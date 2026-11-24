//! Fast zero-allocation HTML presentation adapter.
//!
//! Transforms readable text, headings, and embedded SVGs into Markdown IR.

use crate::err::Error;

/// Strips leading whitespace from a byte slice.
const fn trim_start(mut bytes: &[u8]) -> &[u8] {
    while let [first, rest @ ..] = bytes {
        if matches!(*first, b' ' | b'\t' | b'\r' | b'\n') {
            bytes = rest;
        } else {
            break;
        }
    }
    bytes
}

/// Checks if `bytes` starts with `needle` ignoring ASCII case.
fn starts_with_case(bytes: &[u8], needle: &[u8]) -> bool {
    bytes.len() >= needle.len() && bytes[..needle.len()].eq_ignore_ascii_case(needle)
}

/// Finds the next occurrence of a needle ignoring ASCII case within a byte slice.
fn find_case(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() {
        return Some(0);
    }
    if haystack.len() < needle.len() {
        return None;
    }
    let first = needle[0].to_ascii_lowercase();
    let first_up = needle[0].to_ascii_uppercase();
    let mut cursor = 0;
    while cursor + needle.len() <= haystack.len() {
        let pos = memchr::memchr2(first, first_up, &haystack[cursor..])?;
        let start = cursor + pos;
        if start + needle.len() > haystack.len() {
            return None;
        }
        if haystack[start..start + needle.len()].eq_ignore_ascii_case(needle) {
            return Some(start);
        }
        cursor = start + 1;
    }
    None
}

/// Resolves standard HTML entities and appends the decoded character.
fn resolve_entity(rest: &[u8], out: &mut Vec<u8>) -> usize {
    if rest.starts_with(b"&lt;") {
        out.push(b'<');
        4
    } else if rest.starts_with(b"&gt;") {
        out.push(b'>');
        4
    } else if rest.starts_with(b"&amp;") {
        out.push(b'&');
        5
    } else if rest.starts_with(b"&quot;") {
        out.push(b'"');
        6
    } else if rest.starts_with(b"&#39;") {
        out.push(b'\'');
        5
    } else if rest.starts_with(b"&nbsp;") {
        out.push(b' ');
        6
    } else {
        out.push(b'&');
        1
    }
}

/// Tries to parse and extract an embedded SVG block.
fn try_extract_svg(rest: &[u8], out: &mut Vec<u8>) -> Option<usize> {
    let trimmed = trim_start(rest);
    if !starts_with_case(trimmed, b"<svg") {
        return None;
    }
    let after = &trimmed[b"<svg".len()..];
    let is_delim =
        after.is_empty() || matches!(after[0], b' ' | b'\t' | b'\r' | b'\n' | b'>' | b'/');
    if !is_delim {
        return None;
    }
    let end_idx = find_case(trimmed, b"</svg>")?;
    let total_svg_len = end_idx + b"</svg>".len();
    let leading_ws = rest.len() - trimmed.len();

    out.extend_from_slice(b"\n~~~svg\n");
    out.extend_from_slice(&trimmed[..total_svg_len]);
    out.extend_from_slice(b"\n~~~\n\n");

    Some(leading_ws + total_svg_len)
}

/// Checks whether tag represents a structural line break.
fn is_break_tag(tag: &[u8]) -> bool {
    starts_with_case(tag, b"<br")
        || starts_with_case(tag, b"</p>")
        || starts_with_case(tag, b"</div>")
        || starts_with_case(tag, b"</textarea>")
        || (tag.len() >= 4
            && tag[1] == b'/'
            && (tag[2] == b'h' || tag[2] == b'H')
            && (b'1'..=b'6').contains(&tag[3]))
}

/// Decodes normalized whitespace and characters from a raw text chunk.
fn append_text_chunk(chunk: &[u8], out: &mut Vec<u8>, line_content: &mut bool) {
    for &byte in chunk {
        if byte == b'\r' {
            continue;
        }
        if byte == b'\n' {
            if *line_content {
                out.push(b'\n');
                *line_content = false;
            }
        } else if byte == b' ' || byte == b'\t' {
            if *line_content && out.last() != Some(&b' ') && out.last() != Some(&b'\n') {
                out.push(b' ');
            }
        } else {
            out.push(byte);
            *line_content = true;
        }
    }
}

///
/// # Errors
/// Returns `Error::InvalidState` if source is invalid.
pub fn presentation_source(src: &[u8], out: &mut Vec<u8>) -> Result<(), Error> {
    let mut cursor = 0;
    let len = src.len();
    out.reserve(src.len() / 4);

    let mut in_tag = false;
    let mut in_script = false;
    let mut in_style = false;
    let mut line_content = false;

    while cursor < len {
        let rest = &src[cursor..];

        // 1. Skip script payloads
        if in_script {
            if let Some(end) = find_case(rest, b"</script>") {
                cursor += end + b"</script>".len();
                in_script = false;
            } else {
                break;
            }
            continue;
        }

        // 2. Skip style payloads
        if in_style {
            if let Some(end) = find_case(rest, b"</style>") {
                cursor += end + b"</style>".len();
                in_style = false;
            } else {
                break;
            }
            continue;
        }

        // 3. Extract SVG elements
        if !in_tag && let Some(svg_bytes) = try_extract_svg(rest, out) {
            cursor += svg_bytes;
            line_content = false;
            continue;
        }

        // 4. Handle HTML tags
        if rest[0] == b'<' {
            if starts_with_case(rest, b"<script")
                && let Some(cb) = memchr::memchr(b'>', rest)
                && (rest[7] == b'>' || matches!(rest[7], b' ' | b'\t' | b'\r' | b'\n'))
            {
                cursor += cb + 1;
                in_script = true;
                continue;
            }

            if starts_with_case(rest, b"<style")
                && let Some(cb) = memchr::memchr(b'>', rest)
                && (rest[6] == b'>' || matches!(rest[6], b' ' | b'\t' | b'\r' | b'\n'))
            {
                cursor += cb + 1;
                in_style = true;
                continue;
            }

            // Headings <h1 .. <h6
            if rest.len() >= 3
                && (rest[1] == b'h' || rest[1] == b'H')
                && (b'1'..=b'6').contains(&rest[2])
                && let Some(cb) = memchr::memchr(b'>', rest)
            {
                let level = (rest[2] - b'0') as usize;
                if line_content {
                    out.push(b'\n');
                }
                out.extend_from_slice(&b"######"[..level]);
                out.push(b' ');
                cursor += cb + 1;
                line_content = true;
                continue;
            }

            // Structural break tags
            if is_break_tag(rest)
                && let Some(cb) = memchr::memchr(b'>', rest)
            {
                if line_content {
                    out.push(b'\n');
                    line_content = false;
                }
                cursor += cb + 1;
                continue;
            }

            if let Some(cb) = memchr::memchr(b'>', rest) {
                cursor += cb + 1;
                continue;
            }
            in_tag = true;
            cursor += 1;
            continue;
        }

        if in_tag {
            if let Some(cb) = memchr::memchr(b'>', rest) {
                cursor += cb + 1;
                in_tag = false;
            } else {
                break;
            }
            continue;
        }

        // 5. Decode normal text chunk
        let next_special = memchr::memchr2(b'<', b'&', rest).unwrap_or(rest.len());
        let text_chunk = &rest[..next_special];
        if !text_chunk.is_empty() {
            append_text_chunk(text_chunk, out, &mut line_content);
            cursor += next_special;
        }

        // 6. Decode entity
        if cursor < len && src[cursor] == b'&' {
            let advanced = resolve_entity(&src[cursor..], out);
            cursor += advanced;
            line_content = true;
        }
    }

    if line_content {
        out.push(b'\n');
    }

    Ok(())
}
