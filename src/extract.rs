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
    for (text, bonus) in [(subject, 3), (body, 0)] {
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

            match out.iter_mut().find(|c| c.code == code) {
                Some(existing) => existing.score = existing.score.max(score) + 1,
                None => out.push(Candidate { code, score }),
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

static URL: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"(?i)\bhttps?://[^\s<>"'()\[\]{}]+"#).unwrap());
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
    let mut found: Vec<String> = Vec::new();
    for c in HREF.captures_iter(html) {
        if let Some(url) = link_url(&c[1]) {
            push_new(&mut found, url);
        }
    }
    let linked = found.len();
    let mut loaded = HashSet::new();
    for c in LOADED.captures_iter(html) {
        let srcset = c.get(4).or(c.get(5));
        let value = (1..=8).find_map(|i| c.get(i)).map_or("", |m| m.as_str());
        let urls: Vec<&str> = match srcset {
            Some(set) => srcset_urls(set.as_str()),
            None => vec![value],
        };
        loaded.extend(urls.into_iter().filter_map(link_url));
    }
    loaded.retain(|u| !found[..linked].contains(u));
    for m in URL.find_iter(html).chain(URL.find_iter(text)) {
        if let Some(url) = link_url(m.as_str()).filter(|u| !loaded.contains(u)) {
            push_new(&mut found, url);
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

/// An http(s) URL as written in mail, without the punctuation around it.
fn link_url(raw: &str) -> Option<String> {
    let url = raw
        .trim()
        .replace("&amp;", "&")
        .trim_end_matches(['.', ',', ';', ':', '!', '?', '>'])
        .to_string();
    url.get(..4)
        .is_some_and(|s| s.eq_ignore_ascii_case("http"))
        .then_some(url)
}

/// Adds `url` to `list` unless it has it already.
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
    fn what_html_only_loads_is_not_a_link() {
        // Campaign round 2's "Remote images" mail (B4): no `<a>` at all.
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
}
