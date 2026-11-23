//! Non-executing MDX presentation: hide ESM declarations, preserve source offsets.
use crate::err::Error;

/// Mask root import/export declarations without changing bytes, rows or code samples.
///
/// # Errors
/// Rejects invalid UTF-8 and unbalanced module declarations.
pub fn presentation_source(src: &[u8], out: &mut Vec<u8>) -> Result<(), Error> {
    let text = core::str::from_utf8(src).map_err(|_| Error::InvalidState)?;
    let code: Vec<_> = pulldown_cmark::Parser::new(text)
        .into_offset_iter()
        .filter_map(|(event, range)| {
            matches!(
                event,
                pulldown_cmark::Event::Start(pulldown_cmark::Tag::CodeBlock(_))
            )
            .then_some(range)
        })
        .collect();
    let base = out.len();
    out.extend_from_slice(src);
    let mut offset = 0;
    let mut active = false;
    let mut import = false;
    let mut depth = 0i32;
    let mut quote = 0u8;
    let mut escaped = false;
    let mut comment = false;
    for line in src.split_inclusive(|&b| b == b'\n') {
        let clean = line.trim_ascii();
        let index = code.partition_point(|range| range.end <= offset);
        let in_code = code.get(index).is_some_and(|range| range.start <= offset);
        if !active && !in_code {
            import = clean.starts_with(b"import ") || clean.starts_with(b"import\t");
            let export = clean.strip_prefix(b"export ").is_some_and(|tail| {
                [
                    b"const ".as_slice(),
                    b"let ",
                    b"var ",
                    b"function ",
                    b"class ",
                    b"default ",
                    b"{",
                    b"*",
                ]
                .iter()
                .any(|prefix| tail.starts_with(prefix))
            });
            active = import || export;
        }
        if active {
            let mut pos = 0;
            while pos < line.len() {
                let b = line[pos];
                let next = line.get(pos + 1).copied();
                if comment {
                    if b == b'*' && next == Some(b'/') {
                        comment = false;
                        pos += 1;
                    }
                } else if quote != 0 {
                    if b == quote && !escaped {
                        quote = 0;
                    }
                    escaped = b == b'\\' && !escaped;
                } else if b == b'/' && next == Some(b'/') {
                    break;
                } else if b == b'/' && next == Some(b'*') {
                    comment = true;
                    pos += 1;
                } else if matches!(b, b'\'' | b'"' | b'`') {
                    quote = b;
                    escaped = false;
                } else if matches!(b, b'{' | b'(' | b'[') {
                    depth += 1;
                } else if matches!(b, b'}' | b')' | b']') {
                    depth -= 1;
                }
                if !(0..=64).contains(&depth) {
                    return Err(Error::ResourceLimit);
                }
                pos += 1;
            }
            out[base + offset..base + offset + line.len()]
                .iter_mut()
                .for_each(|b| {
                    if !matches!(*b, b'\n' | b'\r') {
                        *b = b' ';
                    }
                });
            let ended = if import {
                clean
                    .last()
                    .is_some_and(|b| matches!(*b, b'\'' | b'"' | b';'))
                    || clean.is_empty()
            } else {
                clean.last().is_some_and(|b| !matches!(*b, b'=' | b','))
            };
            if depth == 0 && quote == 0 && !comment && ended {
                active = false;
            }
        }
        offset += line.len();
    }
    if depth != 0 || quote != 0 || comment {
        return Err(Error::InvalidState);
    }
    Ok(())
}
