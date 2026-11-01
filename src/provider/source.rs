//! Just enough of a MIME parser to read a message's raw source: its parts'
//! headers (`Content-Type`, `Content-ID`, the file name) and their decoded
//! bodies. Providers' JSON leaves out what is only in the source: mail.tm
//! names an inline image's part by its file name, not its `Content-ID`, and
//! cuts long text at 100 KiB.

use std::collections::HashMap;

use super::mime::{Charset, decode_words};

/// How deep multiparts may nest before the rest is ignored.
const MAX_DEPTH: usize = 12;

/// One leaf part of a message (not a multipart).
#[derive(Debug, PartialEq)]
pub struct Part<'a> {
    /// `type/subtype`, lower case; `text/plain` when missing.
    pub content_type: String,
    pub charset: Option<String>,
    /// Without its angle brackets.
    pub content_id: Option<String>,
    /// From `Content-Disposition`'s `filename`, else `Content-Type`'s `name`.
    pub filename: Option<String>,
    /// `attachment` (or `inline`) when the part says so.
    pub disposition: Option<String>,
    encoding: String,
    body: &'a [u8],
}

impl Part<'_> {
    /// The body without its transfer encoding (base64, quoted-printable).
    pub fn bytes(&self) -> Vec<u8> {
        match self.encoding.as_str() {
            "base64" => base64(self.body),
            "quoted-printable" => quoted_printable(self.body),
            _ => self.body.to_vec(),
        }
    }

    /// The body as text, in its charset (UTF-8 when it names none), with
    /// plain `\n` line ends. `None` for a charset tmpmail can't decode.
    pub fn text(&self) -> Option<String> {
        let charset = match &self.charset {
            Some(name) => Charset::from_name(name)?,
            None => Charset::Utf8,
        };
        Some(charset.decode(&self.bytes()).replace("\r\n", "\n"))
    }

    /// A body part of the message (its text or HTML), not an attachment.
    pub fn is_body(&self) -> bool {
        matches!(self.content_type.as_str(), "text/plain" | "text/html")
            && self.filename.is_none()
            && self.disposition.as_deref() != Some("attachment")
    }
}

/// The leaf parts of a raw message, in order.
pub fn parts(raw: &[u8]) -> Vec<Part<'_>> {
    let mut out = Vec::new();
    walk(raw, 0, &mut out);
    out
}

fn walk<'a>(entity: &'a [u8], depth: usize, out: &mut Vec<Part<'a>>) {
    let (head, body) = split_head(entity);
    let headers = headers(head);
    let get = |name: &str| {
        headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    };
    let (content_type, params) = value_and_params(get("content-type").unwrap_or("text/plain"));
    let content_type = if content_type.contains('/') {
        content_type.to_ascii_lowercase()
    } else {
        "text/plain".to_string()
    };
    let param = |params: &[(String, String)], name: &str| {
        params
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.clone())
    };
    if content_type.starts_with("multipart/") {
        if depth >= MAX_DEPTH {
            return;
        }
        if let Some(boundary) = param(&params, "boundary") {
            for part in split_multipart(body, &boundary) {
                walk(part, depth + 1, out);
            }
        }
        return;
    }
    let (disposition, disp_params) = get("content-disposition")
        .map(value_and_params)
        .unwrap_or_default();
    let filename = param(&disp_params, "filename")
        .or_else(|| param(&params, "name"))
        .map(|f| decode_words(&f))
        .filter(|f| !f.is_empty());
    let content_id = get("content-id")
        .map(|v| {
            v.trim()
                .trim_start_matches('<')
                .trim_end_matches('>')
                .trim()
                .to_string()
        })
        .filter(|v| !v.is_empty());
    out.push(Part {
        content_type,
        charset: param(&params, "charset"),
        content_id,
        filename,
        disposition: Some(disposition.to_ascii_lowercase()).filter(|d| !d.is_empty()),
        encoding: get("content-transfer-encoding")
            .unwrap_or_default()
            .trim()
            .to_ascii_lowercase(),
        body,
    });
}

/// The header block and the body, split at the first empty line.
fn split_head(entity: &[u8]) -> (&[u8], &[u8]) {
    let mut i = 0;
    while i < entity.len() {
        let end = entity[i..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(entity.len(), |p| i + p + 1);
        let line = &entity[i..end];
        if line == b"\n" || line == b"\r\n" {
            return (&entity[..i], &entity[end..]);
        }
        i = end;
    }
    (entity, &[])
}

/// Header names and unfolded values.
fn headers(head: &[u8]) -> Vec<(String, String)> {
    let text = String::from_utf8_lossy(head);
    let mut out: Vec<(String, String)> = Vec::new();
    for line in text.lines() {
        if line.starts_with([' ', '\t']) {
            if let Some((_, v)) = out.last_mut() {
                v.push(' ');
                v.push_str(line.trim());
            }
        } else if let Some((k, v)) = line.split_once(':') {
            out.push((k.trim().to_string(), v.trim().to_string()));
        }
    }
    out
}

/// `type/subtype; a=1; b="2"` as the value and its parameters. RFC 2231
/// parameters (`filename*=utf-8''r%C3%A9sum%C3%A9.txt`, or split as
/// `filename*0*=...; filename*1*=...`) are put back together and decoded.
fn value_and_params(header: &str) -> (String, Vec<(String, String)>) {
    let mut pieces = split_params(header).into_iter();
    let value = pieces.next().unwrap_or_default().trim().to_string();
    let mut plain: Vec<(String, String)> = Vec::new();
    let mut extended: Vec<(String, u32, bool, String)> = Vec::new();
    for piece in pieces {
        let Some((k, v)) = piece.split_once('=') else {
            continue;
        };
        let k = k.trim().to_ascii_lowercase();
        let v = unquote(v.trim());
        match k.split_once('*') {
            None => plain.push((k, v)),
            Some((name, rest)) => {
                let encoded = rest.ends_with('*') || rest.is_empty();
                let n = rest.trim_end_matches('*').parse().unwrap_or(0);
                extended.push((name.to_string(), n, encoded, v));
            }
        }
    }
    extended.sort_by_key(|(name, n, ..)| (name.clone(), *n));
    let mut joined: Vec<(String, String)> = Vec::new();
    for (name, n, encoded, v) in extended {
        let mut text = v;
        if encoded {
            // The first piece carries `charset'language'`.
            if n == 0 {
                let mut it = text.splitn(3, '\'');
                if let (Some(_), Some(_), Some(rest)) = (it.next(), it.next(), it.next()) {
                    text = rest.to_string();
                }
            }
            text = String::from_utf8_lossy(&percent_decode(&text)).into_owned();
        }
        match joined.iter_mut().find(|(k, _)| *k == name) {
            Some((_, v)) => v.push_str(&text),
            None => joined.push((name, text)),
        }
    }
    // An RFC 2231 value wins over a plain one of the same name.
    for (k, v) in joined {
        plain.retain(|(p, _)| *p != k);
        plain.push((k, v));
    }
    (value, plain)
}

/// Splits at the semicolons outside quotes.
fn split_params(s: &str) -> Vec<&str> {
    let (mut out, mut start, mut quoted, mut escaped) = (Vec::new(), 0, false, false);
    for (i, c) in s.char_indices() {
        match c {
            _ if escaped => escaped = false,
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            ';' if !quoted => {
                out.push(&s[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&s[start..]);
    out
}

fn unquote(v: &str) -> String {
    match v.strip_prefix('"').and_then(|v| v.strip_suffix('"')) {
        Some(inner) => {
            let mut out = String::with_capacity(inner.len());
            let mut chars = inner.chars();
            while let Some(c) = chars.next() {
                if c == '\\' {
                    if let Some(n) = chars.next() {
                        out.push(n);
                    }
                } else {
                    out.push(c);
                }
            }
            out
        }
        None => v.to_string(),
    }
}

pub(crate) fn percent_decode(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        let pair = (b[i] == b'%')
            .then(|| Some((hex(*b.get(i + 1)?)?, hex(*b.get(i + 2)?)?)))
            .flatten();
        match pair {
            Some((h, l)) => {
                out.push(h << 4 | l);
                i += 3;
            }
            None => {
                out.push(b[i]);
                i += 1;
            }
        }
    }
    out
}

fn hex(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|d| d as u8)
}

/// The parts of a multipart body between its `--boundary` lines.
fn split_multipart<'a>(body: &'a [u8], boundary: &str) -> Vec<&'a [u8]> {
    let delim = format!("--{boundary}");
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    let mut i = 0;
    while i < body.len() {
        let end = body[i..]
            .iter()
            .position(|&b| b == b'\n')
            .map_or(body.len(), |p| i + p + 1);
        let line = trim_eol(&body[i..end]);
        if let Some(rest) = line.strip_prefix(delim.as_bytes()) {
            let rest = trim_space(rest);
            if rest.is_empty() || rest == b"--" {
                if let Some(s) = start {
                    // The line break before the delimiter belongs to it.
                    out.push(trim_eol(&body[s..i]));
                }
                if rest == b"--" {
                    return out;
                }
                start = Some(end);
            }
        }
        i = end;
    }
    // No closing delimiter: keep what came.
    if let Some(s) = start {
        out.push(&body[s..]);
    }
    out
}

fn trim_eol(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line)
}

fn trim_space(s: &[u8]) -> &[u8] {
    let end = s
        .iter()
        .rposition(|b| !b.is_ascii_whitespace())
        .map_or(0, |p| p + 1);
    &s[..end]
}

/// Base64 that ignores line breaks and anything else outside the alphabet,
/// and stops at the padding.
fn base64(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for &b in data {
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => break,
            _ => continue,
        };
        acc = (acc << 6 | u32::from(v)) & 0xFFFFFF;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    out
}

fn quoted_printable(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        if data[i] == b'=' {
            // A soft line break.
            if data[i + 1..].starts_with(b"\r\n") {
                i += 3;
                continue;
            }
            if data[i + 1..].starts_with(b"\n") {
                i += 2;
                continue;
            }
            let pair = data
                .get(i + 1)
                .and_then(|&h| hex(h))
                .zip(data.get(i + 2).and_then(|&l| hex(l)));
            if let Some((h, l)) = pair {
                out.push(h << 4 | l);
                i += 3;
                continue;
            }
        }
        out.push(data[i]);
        i += 1;
    }
    out
}

/// The raster image type an inline part may be shown as: its own, or for
/// `application/octet-stream` one known from the file name's extension.
/// Never SVG (it can carry script) and never anything that isn't an image.
pub fn image_type(content_type: &str, filename: &str) -> Option<String> {
    let ct = content_type.split(';').next().unwrap_or_default();
    let ct = ct.trim().to_ascii_lowercase();
    if ct.starts_with("image/") {
        return (!ct.contains("svg")).then_some(ct);
    }
    if ct != "application/octet-stream" {
        return None;
    }
    let ext = filename.rsplit_once('.')?.1.to_ascii_lowercase();
    let mime = match ext.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        "avif" => "image/avif",
        _ => return None,
    };
    Some(mime.to_string())
}

/// The largest inline image shown (in the desktop app, and in
/// `read --html`'s copy).
pub const MAX_INLINE_IMAGE: usize = 10 << 20;

/// The inline images of a message's raw source, by `Content-ID` (without
/// its angle brackets): every part with one that may be shown as an image
/// ([`image_type`]), as its type and bytes, up to [`MAX_INLINE_IMAGE`]. The
/// source is read once for all of them.
pub fn inline_images(raw: &[u8]) -> HashMap<String, (String, Vec<u8>)> {
    let mut out = HashMap::new();
    for part in parts(raw) {
        let Some(cid) = &part.content_id else {
            continue;
        };
        let Some(mime) = image_type(
            &part.content_type,
            part.filename.as_deref().unwrap_or_default(),
        ) else {
            continue;
        };
        let bytes = part.bytes();
        if bytes.len() <= MAX_INLINE_IMAGE && !out.contains_key(cid) {
            out.insert(cid.clone(), (mime, bytes));
        }
    }
    out
}

/// A `cid:` URL's id as a `Content-ID` has it: percent-decoded (RFC 2392,
/// so `a%40b` is `a@b`), without angle brackets.
pub fn cid_of_url(id: &str) -> String {
    let id = String::from_utf8_lossy(&percent_decode(id.trim())).into_owned();
    id.trim_start_matches('<').trim_end_matches('>').to_string()
}

/// The message's text and HTML bodies, from its raw source: every
/// `text/plain` (and every `text/html`) part that isn't an attachment, in
/// order, joined as mail.tm joins them. `None` for a kind there is none of,
/// or whose charset tmpmail can't decode.
pub fn bodies(raw: &[u8]) -> (Option<String>, Option<String>) {
    let parts = parts(raw);
    let body = |ct: &str| -> Option<String> {
        let texts: Option<Vec<String>> = parts
            .iter()
            .filter(|p| p.is_body() && p.content_type == ct)
            .map(Part::text)
            .collect();
        texts.filter(|t| !t.is_empty()).map(|t| t.concat())
    };
    (body("text/plain"), body("text/html"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const RELATED: &[u8] = b"From: Demo <demo@tmpmail.test>\r\n\
Subject: Inline image\r\n\
MIME-Version: 1.0\r\n\
Content-Type: multipart/alternative; boundary=\"===outer==\"\r\n\
\r\n\
--===outer==\r\n\
Content-Type: text/plain; charset=\"utf-8\"\r\n\
Content-Transfer-Encoding: quoted-printable\r\n\
\r\n\
Caf=C3=A9 r=C3=A9sum=\r\n\
=C3=A9\r\n\
--===outer==\r\n\
Content-Type: multipart/related; boundary=\"inner\"\r\n\
\r\n\
--inner\r\n\
Content-Type: text/html; charset=iso-8859-1\r\n\
\r\n\
<p>caf\xe9 <img src=\"cid:1793.52@tmpmail.test\"></p>\r\n\
--inner\r\n\
Content-Type: image/png\r\n\
Content-Disposition: inline; filename*=utf-8''inline%2Dbox.png\r\n\
Content-ID: <1793.52@tmpmail.test>\r\n\
Content-Transfer-Encoding: base64\r\n\
\r\n\
iVBORw0K\r\n\
GgoAAA==\r\n\
--inner\r\n\
Content-Type: image/svg+xml\r\n\
Content-ID: <evil@x>\r\n\
\r\n\
<svg/>\r\n\
--inner--\r\n\
--===outer==--\r\n";

    #[test]
    fn parts_are_found_through_nested_multiparts() {
        let raw = RELATED;
        let p = parts(raw);
        assert_eq!(p.len(), 4);
        assert_eq!(p[2].content_id.as_deref(), Some("1793.52@tmpmail.test"));
        assert_eq!(p[2].filename.as_deref(), Some("inline-box.png"));
        let (text, html) = bodies(raw);
        assert_eq!(text.as_deref(), Some("Café résumé"));
        // A charset tmpmail can't decode gives nothing, not garbage.
        let cyrillic =
            b"Content-Type: text/plain; charset=windows-1251\r\n\r\n\xcf\xf0\xe8\xe2\xe5\xf2";
        assert_eq!(bodies(cyrillic), (None, None));
        assert_eq!(
            html.as_deref(),
            Some("<p>café <img src=\"cid:1793.52@tmpmail.test\"></p>")
        );
    }

    #[test]
    fn inline_images_come_from_the_part_with_that_content_id() {
        let images = inline_images(RELATED);
        let (mime, bytes) = &images["1793.52@tmpmail.test"];
        assert_eq!(mime, "image/png");
        assert_eq!(bytes, b"\x89PNG\r\n\x1a\n\0\0");
        // SVG is never served as an image.
        assert_eq!(images.len(), 1, "{:?}", images.keys());
        assert_eq!(cid_of_url("1793.52%40tmpmail.test"), "1793.52@tmpmail.test");
        assert_eq!(cid_of_url("<a@b>"), "a@b");
    }

    #[test]
    fn broken_sources_dont_panic() {
        for raw in [
            "",
            "no headers at all",
            "Content-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\nContent-Transfer-Encoding: base64\r\n\r\n%%%",
            "Content-Type: multipart/mixed\r\n\r\nbody",
            "Content-Type: text/plain\r\nContent-Transfer-Encoding: quoted-printable\r\n\r\nend=",
            "Content-Type: text/plain; charset*=''\r\nContent-Disposition: attachment; filename*0*=%\r\n\r\nx",
        ] {
            let _ = bodies(raw.as_bytes());
            let _ = inline_images(raw.as_bytes());
        }
        let deep = "Content-Type: multipart/mixed; boundary=b\r\n\r\n--b\r\n".repeat(50);
        let _ = parts(deep.as_bytes());
    }
}
