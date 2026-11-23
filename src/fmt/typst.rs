//! Bounded, non-executing Typst presentation: headings, prose, tables and math.
//! Expressions outside this subset remain visible source; no imports or code run.
use crate::err::Error;

/// Translate Typst math into the shared `TeX` compiler's input.
///
/// # Errors
/// Rejects excessive nesting and invalid UTF-8.
pub fn math(src: &str, out: &mut Vec<u8>) -> Result<(), Error> {
    math_inner(src, out, 0)
}

fn math_inner(src: &str, out: &mut Vec<u8>, depth: usize) -> Result<(), Error> {
    if depth > 64 {
        return Err(Error::ResourceLimit);
    }
    let bytes = src.as_bytes();
    let mut pos = 0;
    while pos < bytes.len() {
        let rest = &src[pos..];
        let operator = [
            ("<=>", "\\iff "),
            ("<->", "\\leftrightarrow "),
            ("=>", "\\implies "),
            ("->", "\\to "),
            ("<-", "\\gets "),
            ("<=", "\\le "),
            (">=", "\\ge "),
            ("!=", "\\ne "),
            ("...", "\\dots "),
        ]
        .into_iter()
        .find(|(from, _)| rest.starts_with(from));
        if let Some((from, to)) = operator {
            out.extend_from_slice(to.as_bytes());
            pos += from.len();
            continue;
        }
        if bytes[pos] == b'"'
            && let Some(end) = rest[1..].find('"')
        {
            out.extend_from_slice(b"\\text{");
            for ch in rest[1..=end].chars() {
                if matches!(ch, '{' | '}' | '%' | '_' | '#' | '&') {
                    out.push(b'\\');
                }
                let mut buf = [0; 4];
                out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
            }
            out.push(b'}');
            pos += end + 2;
            continue;
        }
        if matches!(bytes[pos], b'_' | b'^')
            && bytes.get(pos + 1) == Some(&b'(')
            && let Some(end) = matching(&bytes[pos + 1..], b'(', b')')
        {
            out.push(bytes[pos]);
            out.push(b'{');
            math_inner(&src[pos + 2..pos + 1 + end], out, depth + 1)?;
            out.push(b'}');
            pos += end + 2;
            continue;
        }
        if bytes[pos].is_ascii_alphabetic() {
            let start = pos;
            while pos < bytes.len() && bytes[pos].is_ascii_alphanumeric() {
                pos += 1;
            }
            let name = &src[start..pos];
            if matches!(name, "frac" | "sqrt")
                && bytes.get(pos) == Some(&b'(')
                && let Some(end) = matching(&bytes[pos..], b'(', b')')
            {
                let args = &src[pos + 1..pos + end];
                if name == "sqrt" {
                    out.extend_from_slice(b"\\sqrt{");
                    math_inner(args, out, depth + 1)?;
                    out.push(b'}');
                    pos += end + 1;
                    continue;
                }
                let mut nesting = 0;
                let split = args.bytes().position(|b| {
                    match b {
                        b'(' => nesting += 1,
                        b')' => nesting -= 1,
                        _ => {}
                    }
                    b == b',' && nesting == 0
                });
                if let Some(split) = split {
                    out.extend_from_slice(b"\\frac{");
                    math_inner(args[..split].trim(), out, depth + 1)?;
                    out.extend_from_slice(b"}{");
                    math_inner(args[split + 1..].trim(), out, depth + 1)?;
                    out.push(b'}');
                    pos += end + 1;
                    continue;
                }
            }
            let canonical = match name {
                "infinity" | "inf" => "infty",
                "integral" => "int",
                "arrow" => "to",
                "dot" => "cdot",
                "diff" => "mathrm{d}",
                other => other,
            };
            if "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu nu xi pi rho sigma tau upsilon phi chi psi omega Gamma Delta Theta Lambda Xi Pi Sigma Upsilon Phi Psi Omega sum prod int lim sin cos tan log ln exp infty approx equiv times cdot dots to hbar quad partial nabla".split_whitespace().any(|s|s==canonical) || canonical=="mathrm{d}" {
                out.push(b'\\');out.extend_from_slice(canonical.as_bytes());out.push(b' ');
            } else {out.extend_from_slice(name.as_bytes());}
            continue;
        }
        if bytes[pos] == b'%' {
            out.extend_from_slice(b"\\%");
            pos += 1;
            continue;
        }
        let ch = rest.chars().next().ok_or(Error::InvalidState)?;
        let mut buf = [0; 4];
        out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
        pos += ch.len_utf8();
    }
    Ok(())
}

fn matching(src: &[u8], open: u8, close: u8) -> Option<usize> {
    let mut depth = 0usize;
    let mut quoted = false;
    let mut escaped = false;
    for (i, &b) in src.iter().enumerate() {
        if b == b'"' && !escaped {
            quoted = !quoted;
        }
        if !quoted {
            if b == open {
                depth += 1;
                if depth > 64 {
                    return None;
                }
            } else if b == close {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(i);
                }
            }
        }
        escaped = b == b'\\' && !escaped;
    }
    None
}

fn table_cell(cell: &str, out: &mut Vec<u8>) {
    // Keep table geometry native: math in cells uses readable source
    // notation rather than inserting a multi-row image between columns.
    let mut pos = 0;
    let raw = cell.as_bytes();
    while pos < raw.len() {
        if raw[pos] == b'$'
            && let Some(end) = memchr::memchr(b'$', &raw[pos + 1..])
        {
            for &b in &raw[pos + 1..pos + 1 + end] {
                if b == b'|' {
                    out.push(b'\\');
                }
                out.push(b);
            }
            pos += end + 2;
        } else {
            if raw[pos] == b'|' {
                out.push(b'\\');
            }
            out.push(raw[pos]);
            pos += 1;
        }
    }
}

fn table(src: &str, out: &mut Vec<u8>) -> Option<usize> {
    let body = src.strip_prefix("#table(")?;
    let end = matching(&src.as_bytes()[6..], b'(', b')')? + 6;
    let cols = body
        .split_once("columns:")?
        .1
        .trim_start()
        .split(|c: char| !c.is_ascii_digit())
        .next()?
        .parse::<usize>()
        .ok()?;
    if !(1..=32).contains(&cols) {
        return None;
    }
    let body = &src[7..end];
    let mut cells = Vec::new();
    let mut pos = 0;
    while let Some(start) = body[pos..].find('[') {
        let start = pos + start;
        let end = matching(&body.as_bytes()[start..], b'[', b']')? + start;
        cells.push(&body[start + 1..end]);
        pos = end + 1;
        if cells.len() > 4096 {
            return None;
        }
    }
    if cells.is_empty() || cells.len() % cols != 0 {
        return None;
    }
    for (row, chunk) in cells.chunks(cols).enumerate() {
        out.push(b'|');
        for cell in chunk {
            table_cell(cell, out);
            out.push(b'|');
        }
        out.push(b'\n');
        if row == 0 {
            out.push(b'|');
            for _ in 0..cols {
                out.extend_from_slice(b"---|");
            }
            out.push(b'\n');
        }
    }
    Some(end + 1)
}

/// Normalize the supported presentation subset before shared document composition.
///
/// # Errors
/// Returns UTF-8 and expression resource errors.
pub fn to_markdown(src: &[u8], out: &mut Vec<u8>) -> Result<(), Error> {
    let src = core::str::from_utf8(src).map_err(|_| Error::LatexCompile)?;
    let mut normalized = Vec::new();
    let mut pos = 0;
    while pos < src.len() {
        if src[pos..].starts_with("#table(")
            && let Some(used) = table(&src[pos..], &mut normalized)
        {
            pos += used;
            continue;
        }
        let end = src[pos..].find('\n').map_or(src.len(), |i| pos + i);
        let line = &src[pos..end];
        let level = line.bytes().take_while(|&b| b == b'=').count();
        if (1..=6).contains(&level) && line.as_bytes().get(level) == Some(&b' ') {
            normalized.resize(normalized.len() + level, b'#');
            normalized.extend_from_slice(&line.as_bytes()[level..]);
        } else {
            normalized.extend_from_slice(line.as_bytes());
        }
        if end < src.len() {
            normalized.push(b'\n');
        }
        pos = (end + 1).min(src.len());
    }
    let mut pos = 0;
    while pos < normalized.len() {
        if normalized[pos] == b'$'
            && (pos == 0 || normalized[pos - 1] != b'\\')
            && let Some(end) = memchr::memchr(b'$', &normalized[pos + 1..])
        {
            let end = pos + 1 + end;
            let expr =
                core::str::from_utf8(&normalized[pos + 1..end]).map_err(|_| Error::LatexCompile)?;
            let display =
                expr.starts_with(char::is_whitespace) && expr.ends_with(char::is_whitespace);
            out.extend_from_slice(if display { b"$$" } else { b"$" });
            math(expr.trim(), out)?;
            out.extend_from_slice(if display { b"$$" } else { b"$" });
            pos = end + 1;
        } else {
            out.push(normalized[pos]);
            pos += 1;
        }
    }
    Ok(())
}
