//! Small, non-executing HTML presentation adapter.
//! It keeps readable text and headings while ignoring tags and scripts.

use crate::err::Error;

///
/// # Errors
/// Returns `Error::InvalidState` for invalid UTF-8.
pub fn presentation_source(src: &[u8], out: &mut Vec<u8>) -> Result<(), Error> {
    let text = core::str::from_utf8(src).map_err(|_| Error::InvalidState)?;
    let mut in_tag = false;
    let mut in_script = false;
    let mut line_start = true;
    let mut remaining = text;
    while !remaining.is_empty() {
        let candidate = remaining.trim_start_matches([' ', '\t']);
        if !in_script
            && !in_tag
            && candidate.starts_with("<svg ")
            && let Some(end) = candidate.find("</svg>")
        {
            let end = end + "</svg>".len();
            out.extend_from_slice(b"\n~~~svg\n");
            out.extend_from_slice(&candidate.as_bytes()[..end]);
            out.extend_from_slice(b"\n~~~\n");
            remaining = &candidate[end..];
            continue;
        }
        let (line, tail) = remaining.split_once('\n').unwrap_or((remaining, ""));
        let line = line.strip_suffix('\r').unwrap_or(line);
        remaining = tail;
        let trimmed = line.trim();
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("<script") {
            in_script = true;
            continue;
        }
        if in_script {
            if lower.contains("</script>") {
                in_script = false;
            }
            continue;
        }
        let mut heading = None;
        for level in 1..=6 {
            let open = format!("<h{level}");
            if lower.starts_with(&open) {
                heading = Some(level);
                break;
            }
        }
        let mut plain = Vec::new();
        for byte in line.bytes() {
            match byte {
                b'<' => in_tag = true,
                b'>' if in_tag => in_tag = false,
                _ if !in_tag => {
                    if line_start && byte.is_ascii_whitespace() {
                        continue;
                    }
                    plain.push(byte);
                    line_start = false;
                }
                _ => {}
            }
        }
        if !plain.is_empty() {
            if let Some(level) = heading {
                out.extend_from_slice("#".repeat(level).as_bytes());
                out.push(b' ');
            }
            out.extend_from_slice(&plain);
            out.push(b'\n');
        }
        line_start = true;
    }
    // The adapter deliberately leaves tags out and never executes embedded code.
    Ok(())
}
