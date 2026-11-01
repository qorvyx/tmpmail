//! Reading lines of input with a bound on their length: the MCP server's
//! JSON-RPC messages and mail.tm's push stream.

use std::io::{BufRead, Read};

/// One line read by [`read_line`].
pub(crate) enum Line {
    /// The line's bytes, its end (`\n`) included if it had one.
    Text(Vec<u8>),
    /// The line is longer than the limit. What was read of it is dropped;
    /// the rest is still unread ([`skip_line`] reads past it).
    TooLong,
}

/// The next line of `r`, or `None` at the end of the input. At most
/// `max + 1` bytes are held, however long the line: a longer one is
/// [`Line::TooLong`].
pub(crate) fn read_line(r: &mut impl BufRead, max: usize) -> std::io::Result<Option<Line>> {
    let mut buf = Vec::new();
    let n = (&mut *r).take(max as u64 + 1).read_until(b'\n', &mut buf)?;
    if n == 0 {
        return Ok(None);
    }
    if buf.last() != Some(&b'\n') && buf.len() > max {
        return Ok(Some(Line::TooLong));
    }
    Ok(Some(Line::Text(buf)))
}

/// Reads past the end of the current line (its `\n`), holding nothing.
// Only the MCP server (the CLI's) goes on after a long line.
#[cfg_attr(not(feature = "cli"), allow(dead_code))]
pub(crate) fn skip_line(r: &mut impl BufRead) -> std::io::Result<()> {
    loop {
        let chunk = r.fill_buf()?;
        if chunk.is_empty() {
            return Ok(());
        }
        match chunk.iter().position(|&b| b == b'\n') {
            Some(i) => {
                r.consume(i + 1);
                return Ok(());
            }
            None => {
                let len = chunk.len();
                r.consume(len);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_lines_are_reported_and_can_be_skipped() {
        let input = b"short\nmuch too long\nend".to_vec();
        let mut r = std::io::BufReader::with_capacity(4, &input[..]);
        let text = |l: Option<Line>| match l {
            Some(Line::Text(t)) => String::from_utf8(t).unwrap(),
            Some(Line::TooLong) => "long".into(),
            None => "eof".into(),
        };
        assert_eq!(text(read_line(&mut r, 8).unwrap()), "short\n");
        assert_eq!(text(read_line(&mut r, 8).unwrap()), "long");
        skip_line(&mut r).unwrap();
        assert_eq!(text(read_line(&mut r, 8).unwrap()), "end");
        assert_eq!(text(read_line(&mut r, 8).unwrap()), "eof");
        // A line of exactly the limit, with or without its end, is whole.
        let mut r = &b"12345678\n12345678"[..];
        assert_eq!(text(read_line(&mut r, 8).unwrap()), "12345678\n");
        assert_eq!(text(read_line(&mut r, 8).unwrap()), "12345678");
    }
}
