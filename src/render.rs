//! Turning untrusted mail content into something safe to print or open.

use std::borrow::Cow;
use std::collections::{HashMap, HashSet};

/// Removes what could rewrite the user's terminal or disguise the text:
/// terminal escape sequences (CSI and OSC sequences and the other ESC
/// sequences, whole), control characters (carriage returns, ESC, C1
/// controls), bidi controls (overrides, embeddings, isolates, marks) and
/// the other invisible format characters (zero-width spaces, word joiners,
/// the BOM). Zero-width joiners and non-joiners and Unicode tags stay:
/// emoji sequences (and flags) and some scripts need them. Keeps newlines
/// and tabs.
pub fn clean(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    // Once a string sequence (OSC, DCS...) found no terminator, none after
    // it will: later ones aren't searched for again, so a mail full of
    // them stays linear.
    let mut unterminated = false;
    while let Some(c) = rest.chars().next() {
        let after = &rest[c.len_utf8()..];
        rest = match c {
            '\u{1b}' => skip_escape(after, &mut unterminated),
            '\n' | '\t' => {
                out.push(c);
                after
            }
            // Line and paragraph separators: plain newlines.
            '\u{2028}' | '\u{2029}' => {
                out.push('\n');
                after
            }
            // C1 controls (U+0080–U+009F) go one by one, never as the start
            // of a sequence: headers labelled ISO-8859-1 but written in
            // Windows-1252 decode their quotes and dashes to them.
            c if c.is_control() || is_hidden_format(c) => after,
            c => {
                out.push(c);
                after
            }
        };
    }
    out
}

/// After an ESC: the rest of the text without the sequence it starts. A
/// stray ESC (before a letter, a space, or a string that never ends) goes
/// alone, so it can't take real text with it.
fn skip_escape<'a>(s: &'a str, unterminated: &mut bool) -> &'a str {
    let Some(c) = s.chars().next() else { return s };
    let after = &s[c.len_utf8()..];
    match c {
        '[' => skip_csi(after),
        ']' | 'P' | 'X' | '^' | '_' if !*unterminated => match string_end(after) {
            Some(end) => end,
            None => {
                *unterminated = true;
                s
            }
        },
        // Intermediate bytes, then one final byte (`ESC ( B`, `ESC # 8`).
        '!'..='/' => {
            let t = after.trim_start_matches(|c| matches!(c, ' '..='/'));
            match t.chars().next() {
                Some(f @ '0'..='~') => &t[f.len_utf8()..],
                _ => s,
            }
        }
        // Two-character sequences with a digit or symbol (`ESC 7`, `ESC =`).
        '0'..='?' => after,
        _ => s,
    }
}

/// After a CSI: parameter and intermediate bytes, then the final byte.
fn skip_csi(s: &str) -> &str {
    let t = s.trim_start_matches(|c| matches!(c, ' '..='?'));
    match t.chars().next() {
        Some(f @ '@'..='~') => &t[f.len_utf8()..],
        _ => t,
    }
}

/// After an OSC, DCS, SOS, PM or APC introducer: the text after its
/// terminator (BEL, `ESC \\` or ST), or `None` if it has none.
fn string_end(s: &str) -> Option<&str> {
    let i = s.find(['\u{7}', '\u{1b}', '\u{9c}'])?;
    let t = &s[i..];
    Some(if let Some(rest) = t.strip_prefix("\u{1b}\\") {
        rest
    } else if t.starts_with('\u{1b}') {
        // Another escape starts: this string ends here.
        t
    } else {
        // BEL or ST, one character each.
        &t[t.chars().next().map_or(1, char::len_utf8)..]
    })
}

/// Like [`clean`] but also flattens newlines and tabs, for one-line fields.
pub fn clean_line(s: &str) -> String {
    let flat: String = clean(s)
        .chars()
        .map(|c| if c == '\n' || c == '\t' { ' ' } else { c })
        .collect();
    flat.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Bidi controls and the other invisible format characters (Unicode's Cf
/// category), except the zero-width joiner and non-joiner and the tag
/// characters of emoji flags (U+E0020–U+E007F).
pub(crate) fn is_hidden_format(c: char) -> bool {
    matches!(
        c,
        '\u{AD}'
            | '\u{600}'..='\u{605}'
            | '\u{61C}'
            | '\u{6DD}'
            | '\u{70F}'
            | '\u{890}'..='\u{891}'
            | '\u{8E2}'
            | '\u{180E}'
            | '\u{200B}'
            | '\u{200E}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206F}'
            | '\u{FEFF}'
            | '\u{FFF9}'..='\u{FFFB}'
            | '\u{110BD}'
            | '\u{110CD}'
            | '\u{13430}'..='\u{1343F}'
            | '\u{1BCA0}'..='\u{1BCA3}'
            | '\u{1D173}'..='\u{1D17A}'
            | '\u{E0001}'
    )
}

/// JSON text with every control character (C0, DEL, C1), bidi control and
/// other invisible format character written as a `\uXXXX` escape (a pair
/// for one past U+FFFF), so the document shows nothing a terminal would
/// act on or that would make a line read backwards; the values it holds
/// are the same. Such characters can only be inside strings (the rest of
/// JSON is ASCII), so every one in the text is escaped.
pub fn escape_json_invisible(json: &str) -> String {
    use std::fmt::Write;
    // A raw line break is only ever JSON's own layout (serde writes one in
    // a string as `\n`).
    let hidden = |c: char| {
        (c.is_control() && c != '\n') || matches!(c, '\u{2028}' | '\u{2029}') || is_invisible(c)
    };
    if !json.chars().any(hidden) {
        return json.to_string();
    }
    let mut out = String::with_capacity(json.len() + 16);
    for c in json.chars() {
        if hidden(c) {
            let mut units = [0u16; 2];
            for u in c.encode_utf16(&mut units) {
                let _ = write!(out, "\\u{u:04x}");
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The invisible characters: bidi controls and the other format characters
/// ([`is_hidden_format`]), the joiners and the tag characters included.
pub(crate) fn is_invisible(c: char) -> bool {
    matches!(c, '\u{200C}' | '\u{200D}' | '\u{E0000}'..='\u{E007F}') || is_hidden_format(c)
}

/// The largest HTML body parsed (sanitized or turned into text): far more
/// than real mail, whose providers cut much smaller.
pub const MAX_HTML: usize = 4 << 20;

/// The deepest nesting of elements parsed. The HTML parsers take seconds
/// per few thousand levels past this (a sender's 100 000 nested `<div>`
/// took 40 s), and no real mail nests more than a few dozen.
pub const MAX_HTML_DEPTH: usize = 1000;

/// The note above the plain text shown instead of an HTML body too large or
/// too deeply nested to parse.
pub const HTML_TOO_LARGE: &str = "This message's HTML is too large or too deeply nested to \
     show as it was sent: here is its text, without the layout.";

/// Whether `html` is past [`MAX_HTML`] or nests elements deeper than
/// [`MAX_HTML_DEPTH`]. Counted by a linear scan that reads tags as the HTML
/// parser does where it matters for depth: a tag starts with `<` and a
/// letter; comments, attribute values and the text of `script`, `style`,
/// `textarea` and `title` hold no tags; `/>` doesn't close an HTML element;
/// an end tag closes only an element that is open (and those opened inside
/// it); void elements (`<br>`) and ones the parser closes on its own (`<p>`,
/// `<li>`, `<td>`...) don't count.
pub fn html_too_large(html: &str) -> bool {
    if html.len() > MAX_HTML {
        return true;
    }
    const UNCOUNTED: [&str; 26] = [
        "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param",
        "source", "track", "wbr", "p", "li", "dt", "dd", "tr", "td", "th", "option", "optgroup",
        "thead", "tbody", "tfoot",
    ];
    const RAW_TEXT: [&str; 5] = ["script", "style", "textarea", "title", "xmp"];
    let bytes = html.as_bytes();
    let lower_eq = |a: &[u8], b: &str| a.eq_ignore_ascii_case(b.as_bytes());
    let mut open: Vec<&[u8]> = Vec::new();
    let mut i = 0;
    while let Some(off) = bytes[i..].iter().position(|&b| b == b'<') {
        i += off + 1;
        if bytes[i..].starts_with(b"!--") {
            i = find(bytes, i + 3, b"-->").map_or(bytes.len(), |e| e + 3);
            continue;
        }
        let close = bytes.get(i) == Some(&b'/');
        let start = i + usize::from(close);
        if !bytes.get(start).is_some_and(u8::is_ascii_alphabetic) {
            continue;
        }
        let end = bytes[start..]
            .iter()
            .position(|b| !b.is_ascii_alphanumeric() && *b != b'-')
            .map_or(bytes.len(), |e| start + e);
        let name = &bytes[start..end];
        // The rest of the tag, to its `>` outside quotes.
        let mut j = end;
        let mut quote = None;
        while j < bytes.len() {
            match (quote, bytes[j]) {
                (None, b'>') => break,
                (None, q @ (b'"' | b'\'')) => quote = Some(q),
                (Some(q), c) if c == q => quote = None,
                _ => {}
            }
            j += 1;
        }
        i = j;
        if UNCOUNTED.iter().any(|t| lower_eq(name, t)) {
            continue;
        }
        if close {
            if let Some(k) = open.iter().rposition(|n| n.eq_ignore_ascii_case(name)) {
                open.truncate(k);
            }
            continue;
        }
        if let Some(raw) = RAW_TEXT.iter().find(|t| lower_eq(name, t)) {
            // Its text runs to its own end tag.
            let mut k = i;
            i = loop {
                match find(bytes, k, b"</") {
                    Some(e)
                        if bytes.len() >= e + 2 + raw.len()
                            && lower_eq(&bytes[e + 2..e + 2 + raw.len()], raw) =>
                    {
                        break e;
                    }
                    Some(e) => k = e + 2,
                    None => break bytes.len(),
                }
            };
            continue;
        }
        open.push(name);
        if open.len() > MAX_HTML_DEPTH {
            return true;
        }
    }
    false
}

/// Where `needle` first is in `hay` from `from` on.
fn find(hay: &[u8], from: usize, needle: &[u8]) -> Option<usize> {
    hay.get(from..)?
        .windows(needle.len())
        .position(|w| w == needle)
        .map(|p| from + p)
}

/// The text of an HTML body too large or deep to parse: its tags removed,
/// after the note saying so.
fn too_large_text(html: &str) -> String {
    format!("{HTML_TOO_LARGE}\n\n{}", strip_tags(html))
}

/// Plain-text rendering of an HTML body, without loading anything remote.
/// Past [`html_too_large`], its text without tags after [`HTML_TOO_LARGE`].
pub fn html_to_text(html: &str, width: usize) -> String {
    if html_too_large(html) {
        return too_large_text(html);
    }
    html2text::from_read(html.as_bytes(), width.max(20)).unwrap_or_else(|_| strip_tags(html))
}

fn strip_tags(html: &str) -> String {
    let re = regex::Regex::new(r"(?s)<[^>]*>").unwrap();
    re.replace_all(html, " ").into_owned()
}

/// Best text body for a message: the text part, else the HTML converted.
pub fn body_text(text: Option<&str>, html: Option<&str>, width: usize) -> String {
    match (text, html) {
        (Some(t), _) if !t.trim().is_empty() => t.to_string(),
        (_, Some(h)) => html_to_text(h, width),
        _ => String::new(),
    }
}

/// Sanitized standalone HTML document for viewing a message in a browser:
/// [`sanitize_mail_html`]'s body in a document whose meta CSP blocks scripts
/// and (unless `allow_remote_images`) remote images, with no referrer.
pub fn safe_html_document(subject: &str, html: &str, allow_remote_images: bool) -> String {
    safe_html_document_inline(subject, html, allow_remote_images, &HashMap::new())
}

/// [`safe_html_document`] with the `cid:` images `inline` has a URL for
/// (by [`crate::provider::cid_of_url`]'s id; `data:` URLs, which the
/// document's policy allows) pointing at it. The others stay as they are.
pub fn safe_html_document_inline(
    subject: &str,
    html: &str,
    allow_remote_images: bool,
    inline: &HashMap<String, String>,
) -> String {
    let body = sanitize(html, allow_remote_images, None, inline);
    let img_src = if allow_remote_images {
        "data: cid: https: http:"
    } else {
        "data: cid:"
    };
    format!(
        "<!doctype html>\n<html><head><meta charset=\"utf-8\">\n\
         <meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; img-src {img_src}; style-src 'unsafe-inline'\">\n\
         <meta name=\"referrer\" content=\"no-referrer\">\n\
         <title>{}</title></head><body>\n{body}\n</body></html>\n",
        escape_html(subject)
    )
}

/// How the desktop app shows a mail body, where it differs from a browser
/// file: links open as popups (which the app hands to the system browser),
/// and `cid:` images point at the app's mail scheme.
pub struct Frame<'a> {
    /// Prefix for inline images: `cid:ID` becomes `{cid_base}{ID}` with the
    /// id percent-encoded.
    pub cid_base: &'a str,
}

/// Mail HTML with scripts, forms, frames and event handlers removed, links
/// marked noopener, only http/https/mailto/cid/data URLs, and remote images
/// (tracking pixels, also through `url(...)` in styles) dropped unless
/// `allow_remote_images`. `<style>` blocks are dropped too. With a `frame`,
/// links get `target="_blank"`, `cid:` images are rewritten, and plain
/// `http:` images are dropped even when remote images are allowed (the
/// frame's policy would block them anyway).
pub fn sanitize_mail_html(html: &str, allow_remote_images: bool, frame: Option<&Frame>) -> String {
    sanitize(html, allow_remote_images, frame, &HashMap::new())
}

/// The ids of the `cid:` images (`<img src="cid:ID">`) an HTML body shows,
/// as the sanitizer reads them, each once ([`crate::provider::cid_of_url`]).
pub fn cid_images(html: &str) -> Vec<String> {
    if html_too_large(html) {
        return Vec::new();
    }
    let found = std::sync::Arc::new(std::sync::Mutex::new((Vec::new(), HashSet::new())));
    let list = found.clone();
    let mut builder = ammonia::Builder::default();
    builder
        .url_schemes(HashSet::from(["cid"]))
        .attribute_filter(move |element, attribute, value| {
            if let ("img", "src", Some(id)) = (element, attribute, value.strip_prefix("cid:")) {
                let id = crate::provider::cid_of_url(id);
                let mut list = list.lock().expect("cid list");
                if list.1.insert(id.clone()) {
                    list.0.push(id);
                }
            }
            Some(Cow::Borrowed(value))
        });
    builder.clean(html);
    std::mem::take(&mut found.lock().expect("cid list").0)
}

fn sanitize(
    html: &str,
    allow_remote_images: bool,
    frame: Option<&Frame>,
    inline: &HashMap<String, String>,
) -> String {
    if html_too_large(html) {
        return format!(
            "<pre style=\"white-space:pre-wrap\">{}</pre>",
            escape_html(&too_large_text(html))
        );
    }
    let inline = inline.clone();
    // Each `<img src="cid:...">` gets its own copy of the image's `data:`
    // URL, so one image shown a thousand times would make a copy a thousand
    // times its size: past the per-message limit (as base64), the rest stay
    // `cid:`, which the browser can't load.
    let budget = std::sync::Mutex::new(crate::provider::MAX_INLINE_TOTAL / 3 * 4 + 4096);
    let mut builder = ammonia::Builder::default();
    builder
        .add_generic_attributes(["style", "align", "bgcolor", "width", "height"])
        .add_tag_attributes("td", ["valign", "colspan", "rowspan"])
        .add_tag_attributes("table", ["cellpadding", "cellspacing", "border"])
        .link_rel(Some("noopener noreferrer nofollow"))
        // Mail has no base address, so a relative link or image means
        // nothing, and `//host` or `\\host\share` would resolve against the
        // page: `file://host` (an SMB connection on Windows) for the CLI's
        // copy, the app's own scheme in its frame.
        // In the CLI's copy, in-page links (`#section`) stay; the app's
        // frame opens every link as a popup, so there they go too.
        .url_relative(if frame.is_some() {
            ammonia::UrlRelative::Deny
        } else {
            ammonia::UrlRelative::Custom(Box::new(InPageOnly))
        })
        .url_schemes(HashSet::from(["http", "https", "mailto", "cid", "data"]));
    if frame.is_some() {
        // Image-map areas are links too; without a target they would
        // navigate the frame itself.
        builder
            .set_tag_attribute_value("a", "target", "_blank")
            .set_tag_attribute_value("area", "target", "_blank");
    }
    let cid_base = frame.map(|f| f.cid_base.to_string());
    builder.attribute_filter(move |element, attribute, value| {
        // A quote's source: no reader follows it, and ammonia doesn't check
        // it as a URL.
        if attribute == "cite" {
            return None;
        }
        // `data:` only as an image's own raster picture, never as a link (a
        // page or script of the sender's) or an SVG.
        if is_url_attribute(attribute)
            && is_data_url(value)
            && !(element == "img" && attribute == "src" && is_raster_data(value))
        {
            return None;
        }
        if element == "img" && attribute == "src" {
            if let (Some(base), Some(id)) = (&cid_base, value.strip_prefix("cid:")) {
                return Some(Cow::Owned(format!("{base}{}", percent_encode(id))));
            }
            if let Some(url) = value
                .strip_prefix("cid:")
                .and_then(|id| inline.get(&crate::provider::cid_of_url(id)))
            {
                let mut left = budget.lock().expect("inline budget");
                let fits = *left >= url.len();
                if fits {
                    *left -= url.len();
                }
                return Some(if fits {
                    Cow::Owned(url.clone())
                } else {
                    Cow::Borrowed(value)
                });
            }
            let remote = !(is_data_url(value) || value.starts_with("cid:"));
            if remote && !allow_remote_images {
                return None;
            }
            // The app's frame loads remote images over https only.
            if cid_base.is_some() && is_http(value) {
                return None;
            }
        }
        if attribute == "style" {
            // Nothing may leave its box: a link laid over the whole mail
            // would open the sender's address on any click.
            let value = style_in_its_box(value);
            // Inline styles can fetch remote resources through url(...).
            // The app's frame keeps the rest of the style (colors,
            // spacing); the CLI's file drops the whole attribute, as it
            // always has.
            if !allow_remote_images {
                if cid_base.is_some() {
                    return style_without_resources(&value).map(Cow::Owned);
                }
                if loads_anything(&value) {
                    return None;
                }
            } else {
                // With images on, they load from the web only: a relative
                // `url(...)` would reach the app's own scheme in its frame
                // (which loads over https only), or a file next to the
                // CLI's copy.
                return style_web_only(&value, cid_base.is_none()).map(Cow::Owned);
            }
            return (!value.is_empty()).then_some(value);
        }
        Some(Cow::Borrowed(value))
    });
    builder.clean(html).to_string()
}

/// A `style` attribute without the declarations that could load anything:
/// `url(...)`, image functions, and CSS escapes (which could spell `url(`
/// in a way a plain search misses). `None` when nothing is left.
fn style_without_resources(style: &str) -> Option<String> {
    let kept: Vec<&str> = declarations(style)
        .into_iter()
        .map(str::trim)
        .filter(|d| !d.is_empty() && !d.contains('\\') && !loads_anything(d))
        .collect();
    (!kept.is_empty()).then(|| kept.join(";"))
}

/// The inline-style properties kept: the ones mail uses for its look
/// (colors, fonts, spacing, borders, sizes, table and flex layout), and
/// none that lets an element leave its place and cover the rest of the mail
/// (`position`, `inset`, `z-index`, `transform`, `scale`, `zoom`, ...).
/// Each name also stands for its longhands (`border` for `border-top`).
const STYLE_KEPT: &[&str] = &[
    "color",
    "background",
    "border",
    "padding",
    "margin",
    "font",
    "text-",
    "line-height",
    "letter-spacing",
    "word-",
    "white-space",
    "vertical-align",
    "width",
    "height",
    "max-width",
    "max-height",
    "min-width",
    "min-height",
    "display",
    "table-layout",
    "caption-side",
    "empty-cells",
    "list-style",
    "outline",
    "overflow",
    "visibility",
    "float",
    "clear",
    "box-sizing",
    "box-shadow",
    "cursor",
    "hyphens",
    "direction",
    "flex",
    "gap",
    "row-gap",
    "column-gap",
    "align-",
    "justify-",
    "order",
    "mso-",
    "-webkit-text-size-adjust",
    "-ms-text-size-adjust",
    "quotes",
    "tab-size",
];

/// Whether an inline-style property is one [`STYLE_KEPT`] keeps.
fn style_property_kept(name: &str) -> bool {
    let name = name.trim().to_ascii_lowercase();
    STYLE_KEPT.iter().any(|k| {
        name == *k
            || (k.ends_with('-') && name.starts_with(k))
            || name.strip_prefix(k).is_some_and(|r| r.starts_with('-'))
    })
}

/// `style` with only the declarations of [`STYLE_KEPT`] properties, so
/// nothing may leave its box: a link laid over the whole mail would open
/// the sender's address on any click. Read as CSS reads it: comments go,
/// and so does a declaration with a CSS escape (which could spell any
/// name) or a negative margin. Unchanged when every declaration is kept.
fn style_in_its_box(style: &str) -> Cow<'_, str> {
    let (text, commented) = match style.find("/*") {
        Some(_) => (Cow::Owned(without_css_comments(style)), true),
        None => (Cow::Borrowed(style), false),
    };
    let all = declarations(&text);
    let kept = |d: &str| {
        let d = d.trim();
        d.is_empty()
            || (!d.contains('\\')
                && d.split_once(':').is_some_and(|(name, value)| {
                    style_property_kept(name) && !pulls_out(name, value)
                }))
    };
    if !commented && all.iter().all(|d| kept(d)) {
        return Cow::Borrowed(style);
    }
    let out: Vec<&str> = all
        .into_iter()
        .map(str::trim)
        .filter(|d| !d.is_empty() && kept(d))
        .collect();
    Cow::Owned(out.join(";"))
}

/// Whether a kept property's value would still pull the element over
/// others: a negative margin (with padding it can cover the whole mail).
fn pulls_out(name: &str, value: &str) -> bool {
    let name = name.trim().to_ascii_lowercase();
    (name.starts_with("margin") || name == "text-indent")
        && value
            .split(|c: char| c.is_whitespace() || c == '(' || c == ',')
            .any(|v| v.starts_with('-'))
}

/// CSS text without its `/* ... */` comments (an unclosed one runs to the
/// end, as in CSS).
fn without_css_comments(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("/*") {
        out.push_str(&rest[..i]);
        out.push(' ');
        rest = match rest[i + 2..].find("*/") {
            Some(j) => &rest[i + 2 + j + 2..],
            None => "",
        };
    }
    out.push_str(rest);
    out
}

/// What can load an image or style sheet from inline CSS.
const CSS_LOADERS: [&str; 8] = [
    "url(",
    "image(",
    "image-set(",
    "cross-fade(",
    "element(",
    "src(",
    "@import",
    "expression(",
];

/// Whether a style could load anything, read as CSS reads it (escapes
/// decoded, any case): `url(`, an image function, `@import`, or the old
/// `expression(`.
fn loads_anything(style: &str) -> bool {
    let css = css_unescape(style).to_ascii_lowercase();
    CSS_LOADERS.iter().any(|f| css.contains(f))
}

/// The declarations of `style` that load nothing, or only `https:` URLs
/// (and `http:` ones with `http`) through `url(...)`, written plainly (a
/// CSS escape drops it). `None` when nothing is left.
fn style_web_only(style: &str, http: bool) -> Option<String> {
    let kept: Vec<&str> = declarations(style)
        .into_iter()
        .map(str::trim)
        .filter(|d| {
            if d.is_empty() {
                return false;
            }
            let lower = d.to_ascii_lowercase();
            if !loads_anything(d) {
                return true;
            }
            if lower.contains('\\') || CSS_LOADERS[1..].iter().any(|f| lower.contains(f)) {
                return false;
            }
            lower.split("url(").skip(1).all(|arg| {
                let arg =
                    arg.trim_start_matches(|c: char| c.is_whitespace() || c == '"' || c == '\'');
                arg.starts_with("https://")
                    || (http && arg.starts_with("http://"))
                    || (arg.starts_with("data:image/") && !arg.starts_with("data:image/svg"))
            })
        })
        .collect();
    (!kept.is_empty()).then(|| kept.join(";"))
}

/// CSS text with its escapes decoded: `\` and up to six hex digits (and a
/// space after them), or `\` and any other character, as CSS reads them.
fn css_unescape(s: &str) -> Cow<'_, str> {
    if !s.contains('\\') {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let mut hex = String::new();
        while hex.len() < 6 && chars.peek().is_some_and(char::is_ascii_hexdigit) {
            hex.push(chars.next().expect("peeked"));
        }
        if hex.is_empty() {
            if let Some(next) = chars.next() {
                out.push(next);
            }
            continue;
        }
        if chars.peek().is_some_and(|c| c.is_whitespace()) {
            chars.next();
        }
        let code = u32::from_str_radix(&hex, 16).unwrap_or(0xFFFD);
        out.push(
            char::from_u32(code)
                .filter(|&c| c != '\0')
                .unwrap_or('\u{FFFD}'),
        );
    }
    Cow::Owned(out)
}

/// Keeps a relative URL only if it is an in-page link (`#section`).
struct InPageOnly;

impl<'a> ammonia::UrlRelativeEvaluate<'a> for InPageOnly {
    fn evaluate<'url>(&self, url: &'url str) -> Option<Cow<'url, str>> {
        url.starts_with('#').then_some(Cow::Borrowed(url))
    }
}

/// Whether an attribute holds a URL (where a `data:` one could be a page).
fn is_url_attribute(attribute: &str) -> bool {
    matches!(
        attribute,
        "href"
            | "src"
            | "cite"
            | "longdesc"
            | "background"
            | "poster"
            | "action"
            | "formaction"
            | "srcset"
            | "usemap"
            | "data"
    )
}

/// A URL as a browser reads its scheme: leading spaces and controls
/// ignored, tabs and line breaks dropped anywhere. Only the start is read.
fn url_start(url: &str) -> String {
    url.trim_start_matches(|c: char| c <= ' ')
        .chars()
        .filter(|c| !matches!(c, '\t' | '\n' | '\r'))
        .take(64)
        .collect::<String>()
        .to_ascii_lowercase()
}

/// Whether a URL is a `data:` one, in any spelling a browser accepts.
fn is_data_url(url: &str) -> bool {
    url_start(url).starts_with("data:")
}

/// Whether a `data:` URL holds a raster image ([`crate::provider::image_type`]:
/// never SVG).
fn is_raster_data(url: &str) -> bool {
    let start = url_start(url);
    let Some(rest) = start.strip_prefix("data:") else {
        return false;
    };
    let mime = rest.split([';', ',']).next().unwrap_or_default();
    crate::provider::image_type(mime, "").is_some()
}

/// Splits CSS declarations at the semicolons outside quotes and brackets,
/// so `url(data:image/png;base64,...)` stays one declaration. Read as CSS
/// tokenizes it: a backslash escapes the next character, an unclosed string
/// ends at the line's end, and an unquoted `url(...)` runs to its `)`
/// whatever quotes it holds.
fn declarations(style: &str) -> Vec<&str> {
    let (mut out, mut start, mut depth) = (Vec::new(), 0, 0usize);
    let (mut quote, mut in_url) = (None, false);
    let mut chars = style.char_indices();
    while let Some((i, c)) = chars.next() {
        if in_url {
            match c {
                '\\' => {
                    chars.next();
                }
                ')' => {
                    in_url = false;
                    depth = depth.saturating_sub(1);
                }
                _ => {}
            }
            continue;
        }
        match (quote, c) {
            (_, '\\') => {
                chars.next();
            }
            (Some(q), _) if c == q => quote = None,
            (Some(_), '\n' | '\r' | '\u{C}') => quote = None,
            (Some(_), _) => {}
            (None, '"' | '\'') => quote = Some(c),
            (None, '(') => {
                depth += 1;
                let unquoted = !style[i + 1..].trim_start().starts_with(['"', '\'']);
                let is_url = style
                    .get(i.saturating_sub(3)..i)
                    .is_some_and(|w| w.eq_ignore_ascii_case("url"));
                in_url = is_url && unquoted;
            }
            (None, ')') => depth = depth.saturating_sub(1),
            (None, ';') if depth == 0 => {
                out.push(&style[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&style[start..]);
    out
}

/// Whether sanitized mail HTML sets colors or backgrounds of its own
/// (`bgcolor`, or `color`/`background` in a `style`). The app shows such
/// mail inverted in dark mode, and mail without colors in the theme's own.
pub fn has_own_colors(sanitized: &str) -> bool {
    static STYLED: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| {
        regex::Regex::new(
            r#"(?i)\sbgcolor="|\sstyle="(?:[^"]*[;\s])?(?:background(?:-color)?|color)\s*:"#,
        )
        .unwrap()
    });
    STYLED.is_match(sanitized)
}

/// Whether a URL is plain `http:` as a browser would parse it: leading
/// spaces and control characters are ignored, and tabs and newlines
/// anywhere.
fn is_http(url: &str) -> bool {
    url_start(url).starts_with("http:")
}

/// Percent-encodes everything but unreserved URL characters.
fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'.' | b'_' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

pub fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// The longest name [`safe_filename`] gives, in bytes: under the 255 of
/// most file systems, with room for the ` (9999)` a saver adds to a name
/// that is taken.
const MAX_FILENAME: usize = 240;

/// Safe basename for saving an attachment: the last path component only,
/// no control or invisible characters, none of `: * ? " < > |` (Windows),
/// no leading `.` (a hidden file) or `-` (read as an option by `rm *` or
/// `tar *`), no trailing dot or space (Windows drops them), not a Windows
/// device name (`CON`, `NUL`, `COM1.txt`...: those go to the device), and
/// at most [`MAX_FILENAME`] bytes with the extension kept. `fallback` when
/// nothing is left.
pub fn safe_filename(name: &str, fallback: &str) -> String {
    let base = name.rsplit(['/', '\\']).next().unwrap_or_default();
    let cleaned: String = clean_line(base)
        .chars()
        .map(|c| {
            if matches!(c, ':' | '*' | '?' | '"' | '<' | '>' | '|') {
                '_'
            } else {
                c
            }
        })
        .collect();
    let trimmed = cleaned
        .trim_start_matches(|c: char| c == '.' || c.is_whitespace())
        .trim_end_matches(|c: char| c == '.' || c.is_whitespace());
    if trimmed.is_empty() {
        return fallback.to_string();
    }
    let mut out = match trimmed.strip_prefix('-') {
        Some(rest) => format!("_{rest}"),
        None => trimmed.to_string(),
    };
    let stem = out.split('.').next().unwrap_or_default().trim_end();
    if is_device_name(stem) {
        out.insert(0, '_');
    }
    if out.len() > MAX_FILENAME {
        // Keep a short extension; cut the rest on a character boundary.
        let ext = out
            .rfind('.')
            .filter(|&i| i > 0 && out.len() - i <= 16)
            .map(|i| out[i..].to_string())
            .unwrap_or_default();
        let mut cut = MAX_FILENAME - ext.len();
        while !out.is_char_boundary(cut) {
            cut -= 1;
        }
        out = format!("{}{ext}", out[..cut].trim_end_matches(['.', ' ']));
    }
    out
}

/// Whether `stem` names a Windows device (any case): `CON`, `PRN`, `AUX`,
/// `NUL`, `COM0`–`COM9`, `LPT0`–`LPT9` (and their superscript-digit
/// forms), `CONIN$`, `CONOUT$`.
fn is_device_name(stem: &str) -> bool {
    if ["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"]
        .iter()
        .any(|d| stem.eq_ignore_ascii_case(d))
    {
        return true;
    }
    let (Some(prefix), Some(rest)) = (stem.get(..3), stem.get(3..)) else {
        return false;
    };
    let mut rest = rest.chars();
    ["COM", "LPT"]
        .iter()
        .any(|p| prefix.eq_ignore_ascii_case(p))
        && rest
            .next()
            .is_some_and(|c| c.is_ascii_digit() || matches!(c, '\u{B9}' | '\u{B2}' | '\u{B3}'))
        && rest.next().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strips_terminal_escapes() {
        let evil = "Hi\x1b[2J\x1b]0;pwned\x07 there\r\nnext\u{202E}line";
        assert_eq!(clean(evil), "Hi there\nnextline");
        assert_eq!(clean_line("a\n\tb   c"), "a b c");
        // Every kind of sequence goes whole; a stray ESC alone.
        assert_eq!(clean("\x1b[31mRED\x1b[0m"), "RED");
        assert_eq!(clean("a\x1b]8;;https://x\x1b\\b\x1b]8;;\x1b\\c"), "abc");
        assert_eq!(clean("a\x1b(Bb\x1b7c\x1b]0;t\x1b[1md"), "abcd");
        assert_eq!(clean("a\x1b]never ends"), "a]never ends");
        assert_eq!(clean("a\x1b"), "a");
        assert_eq!(clean("a\x1bé"), "aé");
        assert_eq!(clean("Hi\x1b there"), "Hi there");
        assert_eq!(clean("a\x1bPlease"), "aPlease");
        assert_eq!(clean("a\x1bcool"), "acool");
        // C1 controls go alone: a Windows-1252 subject labelled ISO-8859-1
        // keeps its words.
        assert_eq!(
            clean("\u{9e}zivot a \u{9c}uvre \u{9b}2024 Sale"),
            "zivot a uvre 2024 Sale"
        );
    }

    #[test]
    fn many_unterminated_strings_stay_fast() {
        // Each string ends where the next escape starts; the last one never
        // ends, so only its ESC goes.
        let evil = "\x1b]abc".repeat(200_000);
        let start = std::time::Instant::now();
        assert_eq!(clean(&evil), "]abc");
        assert!(start.elapsed() < std::time::Duration::from_secs(2));
    }

    #[test]
    fn strips_bidi_and_invisible_format_characters() {
        // The hostile sender of campaign round 2 (B1): the override would
        // turn the rest of a list row around.
        assert_eq!(
            clean_line("\"<b>Evil</b> & Co \u{202e}boN\" <evil@x.com>"),
            "\"<b>Evil</b> & Co boN\" <evil@x.com>"
        );
        for c in [
            '\u{202A}',
            '\u{202E}',
            '\u{2066}',
            '\u{2069}',
            '\u{200E}',
            '\u{200F}',
            '\u{61C}',
            '\u{200B}',
            '\u{2060}',
            '\u{FEFF}',
            '\u{AD}',
            '\u{E0001}',
            '\u{85}',
            '\u{9b}',
        ] {
            assert_eq!(clean_line(&format!("a{c}b")), "ab", "U+{:04X}", c as u32);
        }
        assert_eq!(clean_line("a\u{2028}b"), "a b");
        // Joiners and flag tags stay: emoji sequences and some scripts
        // need them.
        assert_eq!(clean("👩\u{200D}💻 \u{200C}"), "👩\u{200D}💻 \u{200C}");
        let england = "\u{1F3F4}\u{E0067}\u{E0062}\u{E0065}\u{E006E}\u{E0067}\u{E007F}";
        assert_eq!(clean(england), england);
        assert_eq!(clean("Zoë 日本 مرحبا"), "Zoë 日本 مرحبا");
    }

    #[test]
    fn safe_html_drops_scripts_and_tracking_pixels() {
        let html = r#"<p onclick="x()">Hi<script>alert(1)</script></p>
            <img src="https://track.example/p.gif"><img src="cid:logo">
            <a href="https://example.com/verify">go</a><form><input></form>
            <div style="background:url(https://t.example/x)">bg</div>"#;
        let doc = safe_html_document("S <x>", html, false);
        assert!(!doc.contains("<script"));
        assert!(!doc.contains("onclick"));
        assert!(!doc.contains("track.example"));
        assert!(!doc.contains("t.example"));
        assert!(doc.contains("cid:logo"));
        assert!(doc.contains("noopener"));
        assert!(!doc.contains("<form"));
        assert!(doc.contains("<title>S &lt;x&gt;</title>"));
        assert!(doc.contains("img-src data: cid:;"));

        let with_images = safe_html_document("S", html, true);
        assert!(with_images.contains("track.example"));
    }

    /// `read --html` output from before the sanitizer was split out of
    /// `safe_html_document`: it must not change by a byte.
    #[test]
    fn safe_html_document_is_unchanged() {
        let html = r#"<p onclick="x()" style="color:red">Hi<script>alert(1)</script></p>
            <style>p{color:blue}</style><img src="https://track.example/p.gif" alt="t">
            <img src="cid:logo@x"><a href="https://example.com/verify" target="_top">go</a>
            <a href="javascript:alert(1)">js</a><form><input></form>
            <table cellpadding="2"><tr><td valign="top" colspan="2">c</td></tr></table>
            <div style="background:url(https://t.example/x)">bg</div>"#;
        let expected = "<!doctype html>\n<html><head><meta charset=\"utf-8\">\n<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; img-src data: cid:; style-src 'unsafe-inline'\">\n<meta name=\"referrer\" content=\"no-referrer\">\n<title>S &lt;x&gt;</title></head><body>\n<p style=\"color:red\">Hi</p>\n            <img alt=\"t\">\n            <img src=\"cid:logo@x\"><a href=\"https://example.com/verify\" rel=\"noopener noreferrer nofollow\">go</a>\n            <a rel=\"noopener noreferrer nofollow\">js</a>\n            <table cellpadding=\"2\"><tbody><tr><td valign=\"top\" colspan=\"2\">c</td></tr></tbody></table>\n            <div>bg</div>\n</body></html>\n";
        assert_eq!(safe_html_document("S <x>", html, false), expected);
        let with_images = "<!doctype html>\n<html><head><meta charset=\"utf-8\">\n<meta http-equiv=\"Content-Security-Policy\" content=\"default-src 'none'; img-src data: cid: https: http:; style-src 'unsafe-inline'\">\n<meta name=\"referrer\" content=\"no-referrer\">\n<title>S &lt;x&gt;</title></head><body>\n<p style=\"color:red\">Hi</p>\n            <img src=\"https://track.example/p.gif\" alt=\"t\">\n            <img src=\"cid:logo@x\"><a href=\"https://example.com/verify\" rel=\"noopener noreferrer nofollow\">go</a>\n            <a rel=\"noopener noreferrer nofollow\">js</a>\n            <table cellpadding=\"2\"><tbody><tr><td valign=\"top\" colspan=\"2\">c</td></tr></tbody></table>\n            <div style=\"background:url(https://t.example/x)\">bg</div>\n</body></html>\n";
        assert_eq!(safe_html_document("S <x>", html, true), with_images);
    }

    #[test]
    fn frame_mode_opens_links_in_popups_and_serves_cid_images() {
        let html = r#"<a href="https://example.com/">go</a><img src="cid:logo@x y">
            <map name="m"><area href="https://example.com/a" shape="rect"></map>"#;
        let frame = Frame {
            cid_base: "tmpmail-mail://localhost/m1/cid/",
        };
        let body = sanitize_mail_html(html, false, Some(&frame));
        assert_eq!(body.matches(r#"target="_blank""#).count(), 2, "{body}");
        assert!(
            body.contains(r#"src="tmpmail-mail://localhost/m1/cid/logo%40x%20y""#),
            "{body}"
        );
        assert!(!sanitize_mail_html(html, false, None).contains("target"));
    }

    #[test]
    fn frame_mode_loads_remote_images_over_https_only() {
        let html = "<img src=\"https://a.example/x.png\"><img src=\"HTTP://b.example/y.png\">\
            <img src=\" ht&#9;tp://b.example/z.png\"><img src=\"\nhttp://b.example/w.png\">";
        let frame = Frame { cid_base: "x:" };
        let on = sanitize_mail_html(html, true, Some(&frame));
        assert!(on.contains("https://a.example/x.png"), "{on}");
        assert!(!on.contains("b.example"), "{on}");
        let off = sanitize_mail_html(html, false, Some(&frame));
        assert!(!off.contains("example"), "{off}");
        // The CLI's file keeps both; its policy allows http too.
        assert!(sanitize_mail_html(html, true, None).contains("b.example"));
    }

    #[test]
    fn frame_mode_strips_only_style_resources() {
        let frame = Frame { cid_base: "x:" };
        let html = r#"<p style="color:red; background:url(https://t.example/x) ; padding:4px">a</p>
            <p style="background-image:url('data:image/png;base64,AA==');margin:0">b</p>
            <p style="b\61ckground:u\72l(https://t.example/y);color:blue">c</p>
            <p style="background:-webkit-image-set('https://t.example/z' 1x)">d</p>"#;
        let off = sanitize_mail_html(html, false, Some(&frame));
        assert!(!off.contains("example"), "{off}");
        assert!(!off.contains("base64"), "{off}");
        assert!(off.contains(r#"style="color:red;padding:4px""#), "{off}");
        assert!(off.contains(r#"style="margin:0""#), "{off}");
        assert!(off.contains(r#"style="color:blue""#), "{off}");
        assert!(off.contains("<p>d</p>"), "{off}");
        // With images on, styles stay as the sender wrote them.
        let on = sanitize_mail_html(html, true, Some(&frame));
        assert!(on.contains("url(https://t.example/x)"), "{on}");
    }

    #[test]
    fn cid_images_are_read_as_the_sanitizer_parses_them() {
        let html = r#"<img src="cid:a&#64;b"><img src=cid:c ><p>src=cid:text</p>
            <img data-src="cid:lazy"><img src="cid:a%40b"><img src="https://x/y.png">"#;
        assert_eq!(cid_images(html), ["a@b", "c"]);
        let inline = HashMap::from([("a@b".to_string(), "data:image/png;base64,AA==".to_string())]);
        let doc = safe_html_document_inline("s", html, false, &inline);
        assert_eq!(
            doc.matches(r#"src="data:image/png;base64,AA==""#).count(),
            2,
            "{doc}"
        );
        assert!(doc.contains(r#"src="cid:c""#), "{doc}");
    }

    #[test]
    fn own_colors_are_detected() {
        assert!(has_own_colors(r##"<td bgcolor="#fff">x</td>"##));
        assert!(has_own_colors(r##"<p style="margin:0; color:#333">x</p>"##));
        assert!(has_own_colors(
            r#"<div style="background-color:red">x</div>"#
        ));
        assert!(!has_own_colors(r#"<p style="border-color:red">x</p>"#));
        assert!(!has_own_colors("<p>color: red</p>"));
        assert!(!has_own_colors("<pre>plain</pre>"));
    }

    #[test]
    fn html_to_text_keeps_content() {
        let t = html_to_text("<p>Your code is <b>123456</b></p>", 80);
        assert!(t.contains("123456"), "{t}");
    }

    #[test]
    fn filenames_cannot_escape_the_target_dir() {
        assert_eq!(safe_filename("../../etc/passwd", "f"), "passwd");
        assert_eq!(safe_filename("..\\..\\boot.ini", "f"), "boot.ini");
        assert_eq!(safe_filename("...", "fallback"), "fallback");
        assert_eq!(safe_filename(".bashrc", "f"), "bashrc");
        assert_eq!(safe_filename("a:b?.txt", "f"), "a_b_.txt");
    }

    #[test]
    fn filenames_are_safe_on_every_system() {
        // Security review B7.
        for (name, safe) in [
            ("CON", "_CON"),
            ("nul.txt", "_nul.txt"),
            ("COM1.pdf", "_COM1.pdf"),
            ("lpt9", "_lpt9"),
            ("com\u{B9}.txt", "_com\u{B9}.txt"),
            ("CONOUT$", "_CONOUT$"),
            ("console.txt", "console.txt"),
            ("COM10.txt", "COM10.txt"),
            ("-rf", "_rf"),
            (
                "--checkpoint-action=exec=sh x.sh",
                "_-checkpoint-action=exec=sh x.sh",
            ),
            ("report.pdf. . ", "report.pdf"),
            ("a.txt:evil", "a.txt_evil"),
            (". .bashrc", "bashrc"),
        ] {
            assert_eq!(safe_filename(name, "f"), safe, "{name:?}");
        }
        let long = format!("{}.pdf", "\u{e9}".repeat(200));
        let cut = safe_filename(&long, "f");
        assert!(cut.len() <= MAX_FILENAME && cut.ends_with(".pdf"), "{cut}");
        assert!(cut.starts_with('\u{e9}'));
    }

    /// Security review A's sanitizer payloads ("Checked, fine" and A6, A8,
    /// C2, C3), and more of the same kind.
    const PAYLOADS: &str = r#"<a href="data:text/html,<script>alert(1)</script>">x</a>
<a href="javascript&colon;alert(1)">x</a>
<a href=" javascript:alert(1)">x</a>
<a href="jav&#x09;ascript:alert(1)">x</a>
<img src="//evil.example/p.gif">
<img src="\\evil.example\share\p.gif">
<a href="//evil.example/x">rel</a>
<a href="\\evil.example\share">unc</a>
<img src="DATA:image/png;base64,AA">
<img src=" data:image/svg+xml,<svg onload=alert(1)>">
<img src="cid:x&quot; onerror=&quot;alert(1)">
<img src="cid:../../etc/passwd">
<svg><a xlink:href="javascript:alert(1)"><text>x</text></a></svg>
<svg><animate attributeName="href" values="javascript:alert(1)"/></svg>
<math><mtext><table><mglyph><style><img src=x onerror=alert(1)>
<noscript><p title="</noscript><img src=x onerror=alert(1)>">
<form action="https://e.x"><button formaction="https://e.x">b</button></form>
<meta http-equiv="refresh" content="0;url=https://e.x">
<base href="https://evil.example/">
<link rel="stylesheet" href="https://e.x/s.css">
<p style="background-image: u\72l(https://t.example/a)">esc</p>
<p style="background: URL(https://t.example/a)">upper</p>
<p style="list-style: url( 'https://t.example/a')">ls</p>
<p style="behavior:url(x.htc)">b</p>
<p style="background:image-set('https://t.example/z' 1x)">is</p>
<p style="cursor: url(https://t.example/c), auto">cur</p>
<table background="https://t.example/bg.png"><tr><td background="https://t.example/td.png">bg</td></tr></table>
<body background="https://t.example/body.png">
<input type="image" src="https://t.example/i.png">
<video poster="https://t.example/v.png"></video>
<object data="https://e.x/o"></object>
<iframe src="https://e.x"></iframe>
<area href="javascript:alert(1)">
<img srcset="https://t.example/s.png 1x">
<a href="https://e.x" ping="https://t.example/ping">p</a>
<img src="https://t.example/a.png" style="x">
<p style="color:red;background:u&#x72;l(https://t.example/ent)">ent</p>
<p style="background:url&#40;https://t.example/ent2)">ent2</p>
<p style="--x:url(https://t.example/var); background:var(--x)">var</p>
<p style="background:ur/**/l(https://t.example/cm)">cm</p>
<img src="data:image/svg+xml;base64,PHN2Zz48L3N2Zz4=">
<a href="mailto:a@b?body=x">m</a>
<a href="cid:foo">c</a>
<img src="http://b.example/w.png">
<p style="background:url(data:image/png;base64,AA)">d</p>
<a href="https://e.x" target="_top">t</a>
<a href="https://e.x" target="_self" rel="opener">t</a>
<a href="https://tracker.example/u1" style="position:fixed;top:0;left:0;width:100%;height:100%;z-index:9">over</a>
<a href="https://tracker.example/u2" style="POSITION : absolute; inset:0; color:red">over2</a>
<a href="https://tracker.example/u3" style="p\6fsition:fixed;inset-block:0;transform:scale(50)">over3</a>
<img src="/mail/1?theme=dark"><img src="/mail/1/source">
<p style="background:url(/mail/1)">rel-css</p>
<p style="background:url('//evil.example/x')">proto-css</p>
<p style="background:IMAGE-SET('https://t.example/z' 1x)">is2</p>
<p style="background:u\72l(https://t.example/esc2)">esc2</p>
<p style="x:expression(alert(1))">expr</p>
<p style="@import 'https://t.example/i.css'">imp</p>
<a href="data:image/png;base64,AA">datalink</a>
<img src="data:text/html;base64,PHNjcmlwdD4=">
<a href="https://tracker.example/c1" style="/**/position:fixed;/**/inset:0;/**/z-index:9">c1</a>
<a href="https://tracker.example/c2" style="position/**/:fixed">c2</a>
<a href="https://tracker.example/c3" style="x:\';position:fixed;inset:0;z-index:9">c3</a>
<a href="https://tracker.example/c4" style="x:'a&#10;;position:fixed;inset:0">c4</a>
<a href="https://tracker.example/c5" style="display:inline-block;scale:200">c5</a>
<a href="https://tracker.example/c6" style="-webkit-transform:scale(200);zoom:200">c6</a>
<a href="https://tracker.example/c7" style="offset-path:path('M0 0');rotate:45deg">c7</a>
<a href="d&#9;ata:text/html,<script>alert(1)</script>">dt</a>
<img src="d&#10;ata:image/svg+xml,<svg onload=alert(1)>">
<a href="https://tracker.example/c8" style="background:url(https://i.example/a'b);position:fixed;inset:0;z-index:9">c8</a>
<a href="https://tracker.example/c9" style="display:block;opacity:.99;margin:-3000px;padding:3000px;background:#fff">c9</a>
<blockquote cite="//evil.example/x">q</blockquote>"#;

    #[test]
    fn hostile_payloads_are_neutralized_in_every_mode() {
        let frame = Frame {
            cid_base: "tmpmail-mail://localhost/mail/7/cid/",
        };
        for line in PAYLOADS.lines() {
            for (mode, images, frame) in [
                ("file", false, None),
                ("file+images", true, None),
                ("frame", false, Some(&frame)),
                ("frame+images", true, Some(&frame)),
            ] {
                let out = sanitize_mail_html(line, images, frame);
                let lower = out.to_ascii_lowercase();
                for bad in [
                    "<script",
                    " onerror=\"",
                    " onload=\"",
                    "javascript",
                    "<svg",
                    "<iframe",
                    "<object",
                    "<form",
                    "<meta",
                    "<base",
                    "<link",
                    "evil.example",
                    "data:text",
                    "data:image/svg",
                    "position",
                    "inset",
                    "z-index",
                    "transform",
                    "scale",
                    "zoom",
                    "rotate",
                    "offset",
                    "d\tata",
                    "opacity",
                    "-3000px",
                    "expression(",
                    "@import",
                    "/mail/1",
                    "ping=",
                ] {
                    assert!(
                        !lower.contains(bad),
                        "{mode}: {bad} in {out:?} from {line:?}"
                    );
                }
                // `ur/**/l(` isn't a URL in CSS (a comment splits the word).
                if !images && !line.contains("ur/**/l") {
                    assert!(!out.contains("t.example"), "{mode}: {out:?} from {line:?}");
                }
                if images && frame.is_some() {
                    assert!(!lower.contains("http://b.example"), "{mode}: {out:?}");
                }
                // A data: URL is only ever a raster image's src (or, with
                // images on, a style's raster background).
                for (i, _) in lower.match_indices("data:") {
                    let before = &lower[..i];
                    assert!(
                        before.ends_with("<img src=\"")
                            || (images
                                && before.ends_with("url(")
                                && lower[i..].starts_with("data:image/png")),
                        "{mode}: {out:?} from {line:?}"
                    );
                }
            }
        }
        // What stays: links, colors, raster data images, https images with
        // images on.
        let kept = sanitize_mail_html(
            r#"<a href="https://e.x/v" style="color:red;top:3px">v</a><img src="data:image/png;base64,AA"><p style="background:url(https://i.example/a.png)">i</p>"#,
            true,
            Some(&frame),
        );
        assert!(kept.contains(r#"href="https://e.x/v""#), "{kept}");
        assert!(kept.contains(r#"style="color:red""#), "{kept}");
        assert!(kept.contains("data:image/png;base64,AA"), "{kept}");
        assert!(kept.contains("url(https://i.example/a.png)"), "{kept}");
        // In-page links, alt text that starts with "Data:", and an inline
        // picture spelled `DATA:` stay.
        let page = sanitize_mail_html(
            r##"<a href="#top">top</a><img alt="Data: Q3" src="DATA:image/png;base64,AA">"##,
            false,
            None,
        );
        assert!(page.contains(r##"href="#top""##), "{page}");
        assert!(page.contains(r#"alt="Data: Q3""#), "{page}");
        assert!(page.contains("DATA:image/png;base64,AA"), "{page}");
        // An escaped font name goes alone; the rest of the style stays.
        let cjk = sanitize_mail_html(
            r#"<p style="font-family:'\5FAE\8F6F';color:#333;padding:20px">x</p>"#,
            false,
            Some(&frame),
        );
        assert!(cjk.contains(r#"style="color:#333;padding:20px""#), "{cjk}");
    }

    #[test]
    fn json_output_escapes_what_a_terminal_would_act_on() {
        // Security review B3: 8-bit CSI/OSC, DEL, bidi and tag characters
        // are escaped; the value read back is the same.
        let subject = "a\u{9b}2J\u{9d}52;c;eA==\u{9c}\u{7f}\u{202E}b\u{E0041}\u{200B} \u{e9}";
        let text =
            serde_json::to_string_pretty(&serde_json::json!({ "subject": subject })).unwrap();
        let out = escape_json_invisible(&text);
        assert!(out.is_ascii() || out.chars().all(|c| !c.is_control() || c == '\n'));
        for raw in [
            '\u{9b}',
            '\u{9d}',
            '\u{9c}',
            '\u{7f}',
            '\u{202E}',
            '\u{E0041}',
            '\u{200B}',
        ] {
            assert!(!out.contains(raw), "{out}");
        }
        assert!(
            out.contains("\\u009b") && out.contains("\\udb40\\udc41"),
            "{out}"
        );
        assert!(out.contains('\u{e9}'), "plain letters stay: {out}");
        let back: serde_json::Value = serde_json::from_str(&out).unwrap();
        assert_eq!(back["subject"], subject);
    }

    #[test]
    fn deeply_nested_html_is_not_parsed() {
        // Security review A4: 100 000 nested `<div>` (500 KB) took 40 s.
        let deep = "<div>".repeat(100_000);
        let t = std::time::Instant::now();
        assert!(html_to_text(&deep, 80).starts_with(HTML_TOO_LARGE));
        assert!(sanitize_mail_html(&deep, false, None).contains("too large"));
        // The shapes that fooled a first, cruder count: `/>` doesn't
        // close a div, an end tag of nothing open closes nothing, and
        // comments and attribute values hold no tags.
        for trick in [
            "<div/>",
            "<div></x>",
            "<div><!--</x>-->",
            "<div title=\"</a>\">",
        ] {
            assert!(html_too_large(&trick.repeat(2000)), "{trick}");
        }
        // Text with `<` that isn't a tag, script text and void elements
        // don't count.
        assert!(!html_too_large(&"a 1<2 b I <3 you ".repeat(5000)));
        assert!(!html_too_large(&format!(
            "<script>{}</script>",
            "<div>".repeat(5000)
        )));
        assert!(cid_images(&deep).is_empty());
        assert!(t.elapsed().as_secs() < 5, "{:?}", t.elapsed());
        // A long ordinary mail is shown.
        let long = "<p>line<br><table><tr><td>a<td>b</table>".repeat(20_000);
        assert!(!html_too_large(&long));
        assert!(!html_too_large(&"<div>x</div>".repeat(100_000)));
        assert!(html_too_large(&"x".repeat(MAX_HTML + 1)));
        // Unclosed tags with no `>` don't make the scan quadratic.
        let t = std::time::Instant::now();
        let _ = html_too_large(&"<a".repeat(1_000_000));
        assert!(t.elapsed().as_secs() < 5, "{:?}", t.elapsed());
    }
}
