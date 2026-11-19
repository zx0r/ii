//! Pure Markdown → ANSI/SGR + OSC 8 hyperlinks + CJK-aware line wrapper.

use crate::{
    ir::Span,
    theme::{Idx, Palette},
};

const SGR_RESET: &[u8] = b"\x1b[0m";
const SGR_BOLD: &[u8] = b"\x1b[1m";
const SGR_ITALIC: &[u8] = b"\x1b[3m";

#[inline]
const fn is_cjk_wide(cp: u32) -> bool {
    matches!(
        cp,
        0x1100..=0x115F
            | 0x2E80..=0x303E
            | 0x3040..=0xA4CF
            | 0xAC00..=0xD7A3
            | 0xF900..=0xFAFF
            | 0xFE10..=0xFE19
            | 0xFE30..=0xFE6F
            | 0xFF00..=0xFF60
            | 0xFFE0..=0xFFE6
            | 0x1F300..=0x1F9FF
            | 0x20000..=0x2FA1F
    )
}

#[inline]
const fn char_width(ch: char) -> u16 {
    if ch.is_control() {
        0
    } else if is_cjk_wide(ch as u32) {
        2
    } else {
        1
    }
}

fn style_line(line: &[u8], cols: u16, palette: &Palette, out: &mut Vec<u8>) {
    let wrap_limit = cols.max(20);
    let mut trimmed = line;
    while trimmed.first() == Some(&b' ') {
        trimmed = &trimmed[1..];
    }

    // ATX Headings: `# `, `## `, `### `
    if trimmed.starts_with(b"#") {
        let mut hashes = 0usize;
        while hashes < trimmed.len() && trimmed[hashes] == b'#' {
            hashes += 1;
        }
        if hashes <= 6 && trimmed.get(hashes) == Some(&b' ') {
            let tok = if hashes == 1 {
                palette.get(Idx::Heading)
            } else {
                palette.get(Idx::HeadingSub)
            };
            out.extend_from_slice(SGR_BOLD);
            out.extend_from_slice(tok.sgr());
            render_inline(&trimmed[hashes + 1..], wrap_limit, palette, out);
            out.extend_from_slice(SGR_RESET);
            return;
        }
    }

    // Bullet lists: `- ` or `* `
    if trimmed.starts_with(b"- ") || trimmed.starts_with(b"* ") {
        out.extend_from_slice(palette.get(Idx::HeadingSub).sgr());
        out.extend_from_slice("• ".as_bytes());
        out.extend_from_slice(palette.get(Idx::Fg).sgr());
        render_inline(&trimmed[2..], wrap_limit.saturating_sub(2), palette, out);
        out.extend_from_slice(SGR_RESET);
        return;
    }

    out.extend_from_slice(palette.get(Idx::Fg).sgr());
    render_inline(line, wrap_limit, palette, out);
    out.extend_from_slice(SGR_RESET);
}

fn render_inline(src: &[u8], max_cols: u16, palette: &Palette, out: &mut Vec<u8>) {
    let mut col = 0u16;
    let mut i = 0usize;
    let len = src.len();

    while i < len {
        // Inline code: `...`
        if src[i] == b'`' {
            if let Some(end) = src[i + 1..].iter().position(|&b| b == b'`') {
                let code = &src[i + 1..i + 1 + end];
                out.extend_from_slice(palette.get(Idx::Code).sgr());
                out.extend_from_slice(code);
                out.extend_from_slice(palette.get(Idx::Fg).sgr());
                #[expect(
                    clippy::cast_possible_truncation,
                    reason = "saturated column width tracking"
                )]
                {
                    col = col.saturating_add(code.len().min(u16::MAX as usize) as u16);
                }
                i += end + 2;
                continue;
            }
        }

        // Bold: `**...**`
        if i + 1 < len && &src[i..i + 2] == b"**" {
            if let Some(end) = src[i + 2..].windows(2).position(|w| w == b"**") {
                out.extend_from_slice(SGR_BOLD);
                out.extend_from_slice(&src[i + 2..i + 2 + end]);
                out.extend_from_slice(SGR_RESET);
                out.extend_from_slice(palette.get(Idx::Fg).sgr());
                i += end + 4;
                continue;
            }
        }

        // Italic: `*...*`
        if src[i] == b'*' {
            if let Some(end) = src[i + 1..].iter().position(|&b| b == b'*') {
                out.extend_from_slice(SGR_ITALIC);
                out.extend_from_slice(&src[i + 1..i + 1 + end]);
                out.extend_from_slice(SGR_RESET);
                out.extend_from_slice(palette.get(Idx::Fg).sgr());
                i += end + 2;
                continue;
            }
        }

        // Markdown link `[label](url)` → OSC 8 hyperlink
        if src[i] == b'[' {
            if let Some(close_br) = src[i + 1..].iter().position(|&b| b == b']') {
                let p_open = i + 1 + close_br + 1;
                if p_open < len && src[p_open] == b'(' {
                    if let Some(close_par) = src[p_open + 1..].iter().position(|&b| b == b')') {
                        let label = &src[i + 1..i + 1 + close_br];
                        let url = &src[p_open + 1..p_open + 1 + close_par];
                        out.extend_from_slice(b"\x1b]8;;");
                        out.extend_from_slice(url);
                        out.extend_from_slice(b"\x1b\\");
                        out.extend_from_slice(palette.get(Idx::Link).sgr());
                        out.extend_from_slice(label);
                        out.extend_from_slice(palette.get(Idx::Fg).sgr());
                        out.extend_from_slice(b"\x1b]8;;\x1b\\");
                        i = p_open + 1 + close_par + 1;
                        continue;
                    }
                }
            }
        }

        // Decode UTF-8 char for CJK width-aware wrapping
        let rem = &src[i..];
        let (ch, ch_len) = match core::str::from_utf8(rem) {
            Ok(s) => {
                let c = s.chars().next().unwrap_or(' ');
                (c, c.len_utf8())
            }
            Err(e) => {
                let valid = e.valid_up_to();
                if valid > 0 {
                    let s = core::str::from_utf8(&rem[..valid]).unwrap_or(" ");
                    let c = s.chars().next().unwrap_or(' ');
                    (c, c.len_utf8())
                } else {
                    out.push(src[i]);
                    i += 1;
                    col = col.saturating_add(1);
                    continue;
                }
            }
        };

        let w = char_width(ch);
        if col + w > max_cols && col > 0 {
            out.push(b'\n');
            col = 0;
        }
        out.extend_from_slice(&src[i..i + ch_len]);
        col = col.saturating_add(w);
        i += ch_len;
    }
}

/// Render Markdown slice `src` (excluding regions covered by `spans`) into ANSI/SGR `out`.
pub fn render(src: &[u8], spans: &[Span], cols: u16, palette: &Palette, out: &mut Vec<u8>) {
    let mut cursor = 0usize;
    for sp in spans {
        let start = (sp.outer_start as usize).min(src.len());
        if start > cursor {
            render_chunk(&src[cursor..start], cols, palette, out);
        }
        cursor = (sp.outer_end as usize).min(src.len()).max(cursor);
    }
    if cursor < src.len() {
        render_chunk(&src[cursor..], cols, palette, out);
    }
}

fn render_chunk(chunk: &[u8], cols: u16, palette: &Palette, out: &mut Vec<u8>) {
    let mut first = true;
    for line in chunk.split(|&b| b == b'\n') {
        if !first {
            out.push(b'\n');
        }
        first = false;
        if !line.is_empty() {
            style_line(line, cols, palette, out);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::DARK;

    #[test]
    fn renders_headings_links_and_cjk_wrap() {
        let md = "# Heading\nCheck [docs](https://example.com) and `code`.\n你好世界你好世界\n";
        let mut out = Vec::new();
        render(md.as_bytes(), &[], 12, &DARK, &mut out);
        let s = String::from_utf8_lossy(&out);
        assert!(s.contains("\x1b[1m"));
        assert!(s.contains("\x1b]8;;https://example.com\x1b\\"));
        assert!(s.contains("你好世界"));
    }
}
