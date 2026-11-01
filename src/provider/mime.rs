//! RFC 2047 encoded-words (`=?utf-8?q?Zo=C3=AB?=`) in mail headers. Most
//! providers decode them; tempmail.lol hands them over as they are.

/// `s` with its encoded-words decoded. Whitespace between two adjacent
/// encoded-words is dropped, as RFC 2047 says; words in a charset other
/// than UTF-8, US-ASCII, ISO-8859-1, ISO-8859-15 or Windows-1252, or that don't parse,
/// are left as they are.
pub fn decode_words(s: &str) -> String {
    if !s.contains("=?") {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    // Bytes of the encoded-words just read, waiting for their charset's
    // conversion: a character may be split across two words.
    let mut pending: Option<(Charset, Vec<u8>)> = None;
    // Whitespace after an encoded-word, dropped if another one follows.
    let mut gap = "";
    while !rest.is_empty() {
        if let Some((charset, bytes, len)) = word(rest) {
            match &mut pending {
                Some((c, buf)) if *c == charset => buf.extend(bytes),
                _ => {
                    flush(&mut out, pending.take());
                    pending = Some((charset, bytes));
                }
            }
            gap = "";
            rest = &rest[len..];
            continue;
        }
        let ws = rest.len() - rest.trim_start_matches([' ', '\t', '\r', '\n']).len();
        if ws > 0 && pending.is_some() {
            gap = &rest[..ws];
            rest = &rest[ws..];
            continue;
        }
        flush(&mut out, pending.take());
        out.push_str(gap);
        gap = "";
        let c = rest.chars().next().expect("not empty");
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    flush(&mut out, pending);
    out.push_str(gap);
    out
}

#[derive(Clone, Copy, PartialEq)]
enum Charset {
    Utf8,
    Latin1,
    Latin9,
    Windows1252,
}

fn flush(out: &mut String, pending: Option<(Charset, Vec<u8>)>) {
    let Some((charset, bytes)) = pending else {
        return;
    };
    let text: String = match charset {
        Charset::Utf8 => String::from_utf8_lossy(&bytes).into_owned(),
        Charset::Latin1 => bytes.iter().map(|&b| b as char).collect(),
        Charset::Latin9 => bytes.iter().map(|&b| latin9(b)).collect(),
        Charset::Windows1252 => bytes.iter().map(|&b| windows_1252(b)).collect(),
    };
    // A header is one line: an encoded `=0D=0A` must not make it two.
    out.extend(text.chars().filter(|c| !c.is_control()));
}

/// The longest encoded-word RFC 2047 allows.
const MAX_WORD: usize = 75;

/// The encoded-word at the start of `s`: its charset, its decoded bytes and
/// its length in `s`.
fn word(s: &str) -> Option<(Charset, Vec<u8>, usize)> {
    // Only that far, so a long header full of stray `=?` stays fast.
    let end = s
        .char_indices()
        .find(|&(i, c)| i >= MAX_WORD || c.is_whitespace())
        .map_or(s.len(), |(i, _)| i);
    let s = &s[..end];
    let body = s.strip_prefix("=?")?;
    let (charset, body) = body.split_once('?')?;
    let (encoding, body) = body.split_once('?')?;
    let end = body.find("?=")?;
    let text = &body[..end];
    if text.contains(|c: char| c.is_whitespace() || c == '?') {
        return None;
    }
    // RFC 2231 allows a language after the charset: `utf-8*en`.
    let charset = charset.split('*').next()?.to_ascii_lowercase();
    let charset = match charset.as_str() {
        "utf-8" | "utf8" | "us-ascii" => Charset::Utf8,
        "iso-8859-1" | "latin1" => Charset::Latin1,
        "iso-8859-15" | "latin9" => Charset::Latin9,
        "windows-1252" | "cp1252" => Charset::Windows1252,
        _ => return None,
    };
    let bytes = match encoding {
        "b" | "B" => base64(text)?,
        "q" | "Q" => quoted(text)?,
        _ => return None,
    };
    // `body` ends where `s` does.
    let len = s.len() - (body.len() - end - 2);
    Some((charset, bytes, len))
}

/// The `Q` encoding: `_` is a space, `=XX` a byte.
fn quoted(text: &str) -> Option<Vec<u8>> {
    let mut out = Vec::with_capacity(text.len());
    let mut bytes = text.bytes();
    while let Some(b) = bytes.next() {
        match b {
            b'_' => out.push(b' '),
            b'=' => {
                let hi = hex(bytes.next()?)?;
                let lo = hex(bytes.next()?)?;
                out.push(hi << 4 | lo);
            }
            _ => out.push(b),
        }
    }
    Some(out)
}

fn hex(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|d| d as u8)
}

fn base64(text: &str) -> Option<Vec<u8>> {
    let data = text.trim_end_matches('=');
    if data.contains('=') || text.len() - data.len() > 2 {
        return None;
    }
    let mut out = Vec::with_capacity(text.len() * 3 / 4);
    let (mut acc, mut bits) = (0u32, 0);
    for b in data.bytes() {
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => return None,
        };
        acc = acc << 6 | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    // A lone sixth of a byte left over: not base64.
    (bits < 6).then_some(out)
}

/// ISO-8859-15 differs from ISO-8859-1 in eight places.
fn latin9(b: u8) -> char {
    match b {
        0xA4 => '€',
        0xA6 => 'Š',
        0xA8 => 'š',
        0xB4 => 'Ž',
        0xB8 => 'ž',
        0xBC => 'Œ',
        0xBD => 'œ',
        0xBE => 'Ÿ',
        _ => b as char,
    }
}

/// Windows-1252 differs from ISO-8859-1 in 0x80..=0x9F only.
fn windows_1252(b: u8) -> char {
    const HIGH: [char; 32] = [
        '€', '\u{81}', '‚', 'ƒ', '„', '…', '†', '‡', 'ˆ', '‰', 'Š', '‹', 'Œ', '\u{8d}', 'Ž',
        '\u{8f}', '\u{90}', '‘', '’', '“', '”', '•', '–', '—', '˜', '™', 'š', '›', 'œ', '\u{9d}',
        'ž', 'Ÿ',
    ];
    match b {
        0x80..=0x9F => HIGH[(b - 0x80) as usize],
        _ => b as char,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoded_words_are_decoded() {
        assert_eq!(
            decode_words("=?utf-8?q?Zo=C3=AB_=C3=9Cnicode?= <hello@example.org>"),
            "Zoë Ünicode <hello@example.org>"
        );
        assert_eq!(
            decode_words("=?utf-8?b?Q2Fmw6kgcsOpc3Vtw6kg4pyT?= code"),
            "Café résumé ✓ code"
        );
        // Whitespace between two encoded-words goes; a character split
        // across them is put back together.
        assert_eq!(
            decode_words("=?UTF-8?Q?Gr=C3?= \r\n =?UTF-8?Q?=BC=C3=9Fe?="),
            "Grüße"
        );
        assert_eq!(decode_words("=?ISO-8859-1?Q?caf=E9?= ok"), "café ok");
        assert_eq!(decode_words("=?windows-1252?Q?=80_5?="), "€ 5");
        assert_eq!(decode_words("=?utf-8*en?B?aGk=?="), "hi");
        assert_eq!(decode_words("=?iso-8859-15?q?Facture_=A4?="), "Facture €");
        // No line breaks smuggled into a one-line header.
        assert_eq!(decode_words("=?utf-8?q?a=0D=0Ab?="), "ab");
    }

    #[test]
    fn other_text_is_left_alone() {
        for s in [
            "plain subject",
            "a =? b ?= c",
            "=?koi8-r?B?8NLJ18XU?=",
            "=?utf-8?x?abc?=",
            "=?utf-8?q?=ZZ?=",
            "=?utf-8?q?unterminated",
            "=?utf-8?b?a?=",
            "=?utf-8?b?aGk=garbage?=",
            "=?utf-8?q?this_encoded_word_is_far_longer_than_the_seventy_five_characters_allowed?=",
            "",
        ] {
            assert_eq!(decode_words(s), s);
        }
        // Text between words keeps its spaces.
        assert_eq!(decode_words("=?utf-8?q?a?= and =?utf-8?q?b?="), "a and b");
        // Linear, not quadratic, in stray `=?`.
        let long = "=?a?b?x".repeat(20_000);
        let start = std::time::Instant::now();
        assert_eq!(decode_words(&long), long);
        assert!(start.elapsed() < std::time::Duration::from_secs(1));
    }
}
