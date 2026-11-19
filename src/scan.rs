//! Single-pass O(N) SWAR/memchr scanner emitting strictly monotonic, non-overlapping `Span`s.

#![expect(
    clippy::cast_possible_truncation,
    reason = "input offsets fit within 28-bit Span range"
)]

use memchr::{memchr, memchr2, memchr3};

use crate::ir::{Kind, OFFSET_MASK, Span};

const MATH_ENVS: &[&[u8]] = &[
    b"equation*",
    b"equation",
    b"align*",
    b"align",
    b"gather*",
    b"gather",
    b"multline*",
    b"multline",
    b"pmatrix",
    b"bmatrix",
    b"vmatrix",
    b"Vmatrix",
    b"Bmatrix",
    b"matrix",
    b"cases",
    b"split",
    b"CD",
];

#[inline]
const fn is_space(b: u8) -> bool {
    matches!(b, b' ' | b'\t')
}

#[inline]
const fn is_ascii_digit(b: u8) -> bool {
    b.is_ascii_digit()
}

#[inline]
fn line_end(src: &[u8], pos: usize) -> usize {
    memchr(b'\n', &src[pos..]).map_or(src.len(), |off| pos + off)
}

#[inline]
fn trim_ascii(mut s: &[u8]) -> &[u8] {
    while let Some((&b, rest)) = s.split_first() {
        if b.is_ascii_whitespace() {
            s = rest;
        } else {
            break;
        }
    }
    while let Some((&b, rest)) = s.split_last() {
        if b.is_ascii_whitespace() {
            s = rest;
        } else {
            break;
        }
    }
    s
}

#[inline]
fn eq_ignore_ascii_case(a: &[u8], b: &[u8]) -> bool {
    a.eq_ignore_ascii_case(b)
}

#[inline]
fn classify_fence(tag: &[u8]) -> Option<Kind> {
    let tag = trim_ascii(tag);
    if let Some(inner) = tag
        .strip_prefix(b"{")
        .and_then(|s| s.strip_suffix(b"}"))
    {
        let head = inner
            .split(|&b| b.is_ascii_whitespace() || b == b',')
            .next()
            .unwrap_or(&[]);
        if eq_ignore_ascii_case(head, b"mermaid") {
            return Some(Kind::Mermaid);
        }
        if eq_ignore_ascii_case(head, b"dot") || eq_ignore_ascii_case(head, b"graphviz") {
            return Some(Kind::Dot);
        }
        if !head.is_empty() {
            return Some(Kind::Quarto);
        }
    }
    let word = tag
        .split(|&b| b.is_ascii_whitespace())
        .next()
        .unwrap_or(&[]);
    if eq_ignore_ascii_case(word, b"mermaid") {
        Some(Kind::Mermaid)
    } else if eq_ignore_ascii_case(word, b"svg") {
        Some(Kind::RawSvg)
    } else if eq_ignore_ascii_case(word, b"vega-lite") || eq_ignore_ascii_case(word, b"vegalite") {
        Some(Kind::VegaLite)
    } else if eq_ignore_ascii_case(word, b"plotly") {
        Some(Kind::Plotly)
    } else if eq_ignore_ascii_case(word, b"csv") || eq_ignore_ascii_case(word, b"tsv") {
        Some(Kind::CsvTsv)
    } else if eq_ignore_ascii_case(word, b"dot") || eq_ignore_ascii_case(word, b"graphviz") {
        Some(Kind::Dot)
    } else if eq_ignore_ascii_case(word, b"math") || eq_ignore_ascii_case(word, b"latex") {
        Some(Kind::MathDisplay)
    } else if eq_ignore_ascii_case(word, b"chem") || eq_ignore_ascii_case(word, b"mhchem") {
        Some(Kind::Chem)
    } else if eq_ignore_ascii_case(word, b"typst") {
        Some(Kind::TypstMath)
    } else {
        None
    }
}

/// Try matching a fenced block (` ``` ` or `~~~`) or Quarto `:::` callout at `line_start`.
/// Returns `(next_pos, Option<Span>)` if a block starts at `line_start`.
fn try_line_block(src: &[u8], line_start: usize) -> Option<(usize, Option<Span>)> {
    let len = src.len();
    let mut p = line_start;
    let mut indent = 0usize;
    while p < len && src[p] == b' ' && indent < 3 {
        p += 1;
        indent += 1;
    }
    if p >= len {
        return None;
    }

    // Fenced code block: ``` or ~~~
    if (src[p] == b'`' || src[p] == b'~') && p + 3 <= len {
        let ch = src[p];
        let mut fence_len = 0usize;
        while p + fence_len < len && src[p + fence_len] == ch {
            fence_len += 1;
        }
        if fence_len >= 3 {
            let hdr_end = line_end(src, p + fence_len);
            let tag = &src[p + fence_len..hdr_end];
            let payload_start = (hdr_end + 1).min(len);
            let mut cur = payload_start;
            while cur < len {
                let le = line_end(src, cur);
                let mut cp = cur;
                let mut ci = 0usize;
                while cp < le && src[cp] == b' ' && ci < 3 {
                    cp += 1;
                    ci += 1;
                }
                let mut c_len = 0usize;
                while cp + c_len < le && src[cp + c_len] == ch {
                    c_len += 1;
                }
                if c_len >= fence_len && trim_ascii(&src[cp + c_len..le]).is_empty() {
                    let outer_end = (le + 1).min(len);
                    let span = classify_fence(tag).map(|kind| {
                        Span::new(
                            line_start as u32,
                            outer_end as u32,
                            payload_start as u32,
                            cur as u32,
                            kind,
                        )
                    });
                    return Some((outer_end, span));
                }
                cur = (le + 1).min(len);
            }
            return Some((len, None));
        }
    }

    // Quarto callout: ::: {.callout-...}
    if p + 3 <= len && &src[p..p + 3] == b":::" {
        let mut colons = 0usize;
        while p + colons < len && src[p + colons] == b':' {
            colons += 1;
        }
        let hdr_end = line_end(src, p + colons);
        let tag = trim_ascii(&src[p + colons..hdr_end]);
        if tag.starts_with(b"{.callout") || tag.starts_with(b"callout") {
            let payload_start = (hdr_end + 1).min(len);
            let mut cur = payload_start;
            while cur < len {
                let le = line_end(src, cur);
                let trimmed = trim_ascii(&src[cur..le]);
                if trimmed.len() >= 3 && trimmed.iter().all(|&b| b == b':') {
                    let outer_end = (le + 1).min(len);
                    return Some((
                        outer_end,
                        Some(Span::new(
                            line_start as u32,
                            outer_end as u32,
                            line_start as u32,
                            outer_end as u32,
                            Kind::QmdCallout,
                        )),
                    ));
                }
                cur = (le + 1).min(len);
            }
        }
    }

    // Markdown pipe table: line starts with `|`, followed by separator line `| --- |`
    if src[p] == b'|' {
        let first_end = line_end(src, p);
        let first_line = trim_ascii(&src[p..first_end]);
        if first_line.len() >= 3 && first_line.ends_with(b"|") && first_end + 1 < len {
            let sec_start = first_end + 1;
            let sec_end = line_end(src, sec_start);
            let sec_line = trim_ascii(&src[sec_start..sec_end]);
            if is_pipe_separator(sec_line) {
                let mut cur = (sec_end + 1).min(len);
                let mut table_end = sec_end;
                while cur < len {
                    let le = line_end(src, cur);
                    let row = trim_ascii(&src[cur..le]);
                    if row.starts_with(b"|") && row.len() >= 2 {
                        table_end = le;
                        cur = (le + 1).min(len);
                    } else {
                        break;
                    }
                }
                let outer_end = table_end.min(len);
                return Some((
                    outer_end,
                    Some(Span::new(
                        line_start as u32,
                        outer_end as u32,
                        line_start as u32,
                        outer_end as u32,
                        Kind::PipeTable,
                    )),
                ));
            }
        }
    }

    None
}

#[inline]
fn is_pipe_separator(line: &[u8]) -> bool {
    if !line.starts_with(b"|") || line.len() < 3 {
        return false;
    }
    let mut has_dash = false;
    for &b in line {
        match b {
            b'-' => has_dash = true,
            b'|' | b':' | b' ' | b'\t' => {}
            _ => return false,
        }
    }
    has_dash
}

/// Find matching closing `}` for a balanced brace group starting at `open_brace` (`src[open_brace] == b'{'`).
fn find_balanced_brace(src: &[u8], open_brace: usize) -> Option<usize> {
    if open_brace >= src.len() || src[open_brace] != b'{' {
        return None;
    }
    let mut depth = 1u32;
    let mut i = open_brace + 1;
    while i < src.len() {
        match src[i] {
            b'\\' => i += 2,
            b'{' => {
                depth += 1;
                i += 1;
            }
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
                i += 1;
            }
            _ => i += 1,
        }
    }
    None
}

/// Single-pass O(N) SWAR/memchr scanner.
/// Emits strictly monotonic, non-overlapping `Span`s into `out`.
pub fn scan(src: &[u8], out: &mut Vec<Span>) {
    let len = src.len().min(OFFSET_MASK as usize);
    let src = &src[..len];
    let mut pos = 0usize;
    let mut at_line_start = true;

    while pos < len {
        if at_line_start {
            if let Some((next_pos, maybe_span)) = try_line_block(src, pos) {
                if let Some(sp) = maybe_span {
                    out.push(sp);
                }
                pos = next_pos;
                at_line_start = pos == 0 || (pos <= len && src.get(pos.wrapping_sub(1)) == Some(&b'\n'));
                continue;
            }
            at_line_start = false;
        }

        // Advance to next delimiter of interest: '\n', '$', '\\', '`', '<'
        let rem = &src[pos..];
        let off_a = memchr3(b'\n', b'$', b'\\', rem);
        let off_b = memchr2(b'`', b'<', rem);
        let Some(off) = (match (off_a, off_b) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }) else {
            break;
        };

        pos += off;
        match src[pos] {
            b'\n' => {
                pos += 1;
                at_line_start = true;
            }
            b'`' => {
                // Inline code span: skip balanced backticks so `$` inside code is ignored
                let mut ticks = 0usize;
                while pos + ticks < len && src[pos + ticks] == b'`' {
                    ticks += 1;
                }
                let mut cur = pos + ticks;
                let mut closed = false;
                while cur < len {
                    let Some(next_tick) = memchr2(b'`', b'\n', &src[cur..]) else {
                        cur = len;
                        break;
                    };
                    cur += next_tick;
                    if src[cur] == b'\n' {
                        cur += 1;
                        at_line_start = true;
                        closed = true;
                        break;
                    }
                    let mut run = 0usize;
                    while cur + run < len && src[cur + run] == b'`' {
                        run += 1;
                    }
                    cur += run;
                    if run == ticks {
                        closed = true;
                        break;
                    }
                }
                pos = if closed { cur } else { pos + ticks };
            }
            b'$' => {
                // Escaped dollar `\$` is handled in `\\` arm, so here `pos` is an unescaped `$`.
                if pos + 1 < len && src[pos + 1] == b'$' {
                    // Display math `$$ ... $$`
                    let p_start = pos + 2;
                    let mut cur = p_start;
                    let mut found = None;
                    while cur + 1 < len {
                        let Some(d) = memchr2(b'$', b'\\', &src[cur..]) else {
                            break;
                        };
                        cur += d;
                        if src[cur] == b'\\' {
                            cur += 2;
                            continue;
                        }
                        if cur + 1 < len && src[cur + 1] == b'$' {
                            found = Some(cur);
                            break;
                        }
                        cur += 1;
                    }
                    if let Some(p_end) = found {
                        if !trim_ascii(&src[p_start..p_end]).is_empty() {
                            out.push(Span::new(
                                pos as u32,
                                (p_end + 2) as u32,
                                p_start as u32,
                                p_end as u32,
                                Kind::MathDisplay,
                            ));
                        }
                        pos = p_end + 2;
                    } else {
                        pos += 2;
                    }
                } else {
                    // Inline math `$ ... $` with currency guard:
                    // Opening `$` cannot be followed by space/tab/newline or digit if preceded by digit.
                    let p_start = pos + 1;
                    if p_start >= len || is_space(src[p_start]) || src[p_start] == b'\n' {
                        pos += 1;
                        continue;
                    }
                    let mut cur = p_start;
                    let mut found = None;
                    while cur < len {
                        let Some(d) = memchr3(b'$', b'\\', b'\n', &src[cur..]) else {
                            break;
                        };
                        cur += d;
                        match src[cur] {
                            b'\n' => break,
                            b'\\' => cur += 2,
                            b'$' => {
                                // Closing `$` cannot be preceded by space or followed immediately by digit
                                let prev = src[cur - 1];
                                let next_is_digit = cur + 1 < len && is_ascii_digit(src[cur + 1]);
                                if !is_space(prev) && !next_is_digit {
                                    found = Some(cur);
                                }
                                break;
                            }
                            _ => cur += 1,
                        }
                    }
                    if let Some(p_end) = found {
                        if p_end > p_start {
                            out.push(Span::new(
                                pos as u32,
                                (p_end + 1) as u32,
                                p_start as u32,
                                p_end as u32,
                                Kind::MathInline,
                            ));
                            pos = p_end + 1;
                        } else {
                            pos += 1;
                        }
                    } else {
                        pos += 1;
                    }
                }
            }
            b'\\' => {
                if pos + 1 >= len {
                    break;
                }
                match src[pos + 1] {
                    b'[' => {
                        // Display math `\[ ... \]`
                        let p_start = pos + 2;
                        let mut cur = p_start;
                        let mut found = None;
                        while cur + 1 < len {
                            let Some(d) = memchr(b'\\', &src[cur..]) else {
                                break;
                            };
                            cur += d;
                            if cur + 1 < len && src[cur + 1] == b']' {
                                found = Some(cur);
                                break;
                            }
                            cur += 2;
                        }
                        if let Some(p_end) = found {
                            out.push(Span::new(
                                pos as u32,
                                (p_end + 2) as u32,
                                p_start as u32,
                                p_end as u32,
                                Kind::MathDisplay,
                            ));
                            pos = p_end + 2;
                        } else {
                            pos += 2;
                        }
                    }
                    b'(' => {
                        // Inline math `\( ... \)`
                        let p_start = pos + 2;
                        let mut cur = p_start;
                        let mut found = None;
                        while cur + 1 < len {
                            let Some(d) = memchr2(b'\\', b'\n', &src[cur..]) else {
                                break;
                            };
                            cur += d;
                            if src[cur] == b'\n' {
                                break;
                            }
                            if cur + 1 < len && src[cur + 1] == b')' {
                                found = Some(cur);
                                break;
                            }
                            cur += 2;
                        }
                        if let Some(p_end) = found {
                            out.push(Span::new(
                                pos as u32,
                                (p_end + 2) as u32,
                                p_start as u32,
                                p_end as u32,
                                Kind::MathInline,
                            ));
                            pos = p_end + 2;
                        } else {
                            pos += 2;
                        }
                    }
                    b'c' if pos + 4 <= len && &src[pos..pos + 4] == b"\\ce{" => {
                        if let Some(close) = find_balanced_brace(src, pos + 3) {
                            out.push(Span::new(
                                pos as u32,
                                (close + 1) as u32,
                                (pos + 4) as u32,
                                close as u32,
                                Kind::Chem,
                            ));
                            pos = close + 1;
                        } else {
                            pos += 4;
                        }
                    }
                    b'p' if pos + 4 <= len && &src[pos..pos + 4] == b"\\pu{" => {
                        if let Some(close) = find_balanced_brace(src, pos + 3) {
                            out.push(Span::new(
                                pos as u32,
                                (close + 1) as u32,
                                (pos + 4) as u32,
                                close as u32,
                                Kind::Chem,
                            ));
                            pos = close + 1;
                        } else {
                            pos += 4;
                        }
                    }
                    b'b' if pos + 7 < len && &src[pos..pos + 7] == b"\\begin{" => {
                        if let Some(env_close) = memchr(b'}', &src[pos + 7..len.min(pos + 32)]) {
                            let env_name = &src[pos + 7..pos + 7 + env_close];
                            if MATH_ENVS.contains(&env_name) {
                                // Search for matching `\end{env_name}`
                                let mut cur = pos + 8 + env_close;
                                let mut found_end = None;
                                while cur + 6 + env_name.len() <= len {
                                    let Some(d) = memchr(b'\\', &src[cur..]) else {
                                        break;
                                    };
                                    cur += d;
                                    if cur + 6 + env_name.len() <= len
                                        && &src[cur..cur + 5] == b"\\end{"
                                        && &src[cur + 5..cur + 5 + env_name.len()] == env_name
                                        && src[cur + 5 + env_name.len()] == b'}'
                                    {
                                        found_end = Some(cur + 6 + env_name.len());
                                        break;
                                    }
                                    cur += 1;
                                }
                                if let Some(outer_end) = found_end {
                                    out.push(Span::new(
                                        pos as u32,
                                        outer_end as u32,
                                        pos as u32,
                                        outer_end as u32,
                                        Kind::MathDisplay,
                                    ));
                                    pos = outer_end;
                                    continue;
                                }
                            }
                        }
                        pos += 7;
                    }
                    _ => {
                        // Skip escaped char (e.g. `\$`, `\\`)
                        pos += 2;
                    }
                }
            }
            b'<' => {
                // Inline raw `<svg ...> ... </svg>`
                if pos + 4 <= len
                    && eq_ignore_ascii_case(&src[pos..pos + 4], b"<svg")
                    && (pos + 4 == len
                        || src[pos + 4].is_ascii_whitespace()
                        || src[pos + 4] == b'>')
                {
                    let mut cur = pos + 4;
                    let mut found_end = None;
                    while cur + 6 <= len {
                        let Some(d) = memchr(b'<', &src[cur..]) else {
                            break;
                        };
                        cur += d;
                        if cur + 6 <= len && eq_ignore_ascii_case(&src[cur..cur + 6], b"</svg>") {
                            found_end = Some(cur + 6);
                            break;
                        }
                        cur += 1;
                    }
                    if let Some(outer_end) = found_end {
                        out.push(Span::new(
                            pos as u32,
                            outer_end as u32,
                            pos as u32,
                            outer_end as u32,
                            Kind::RawSvg,
                        ));
                        pos = outer_end;
                        continue;
                    }
                }
                pos += 1;
            }
            _ => pos += 1,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn monotonic_non_overlapping_spans() {
        let doc = b"# Title\nInline $a+b$ and $$x^2$$ plus \\ce{H2O}.\n```mermaid\ngraph TD; A-->B;\n```\n<svg><rect/></svg>\n| A | B |\n|---|---|\n| 1 | 2 |\n";
        let mut spans = Vec::new();
        scan(doc, &mut spans);
        assert_eq!(spans.len(), 6);
        assert_eq!(spans[0].kind(), Kind::MathInline);
        assert_eq!(spans[1].kind(), Kind::MathDisplay);
        assert_eq!(spans[2].kind(), Kind::Chem);
        assert_eq!(spans[3].kind(), Kind::Mermaid);
        assert_eq!(spans[4].kind(), Kind::RawSvg);
        assert_eq!(spans[5].kind(), Kind::PipeTable);
        for pair in spans.windows(2) {
            assert!(pair[0].outer_end <= pair[1].outer_start);
        }
    }
}
