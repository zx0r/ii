//! CSV, TSV, and Markdown pipe-table → SVG table renderer.

#![expect(
    clippy::cast_precision_loss,
    reason = "bounded table grid coordinates fit in f64"
)]

use core::fmt::Write as _;

use crate::theme::{Idx, Palette};

const MAX_COLS: usize = 16;
const MAX_ROWS: usize = 64;

#[inline]
fn trim(mut s: &[u8]) -> &[u8] {
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

fn write_xml_escaped(out: &mut String, bytes: &[u8]) {
    let s = core::str::from_utf8(bytes).unwrap_or("");
    for ch in s.chars() {
        match ch {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            _ => out.push(ch),
        }
    }
}

fn emit_table_svg(
    grid: &[[&[u8]; MAX_COLS]; MAX_ROWS],
    n_rows: usize,
    n_cols: usize,
    palette: &Palette,
    width: u16,
    out: &mut String,
) {
    let w = f64::from(width.max(200));
    let row_h = 28.0;
    let rows = n_rows.max(1);
    let cols = n_cols.max(1);
    let h = (rows as f64) * row_h + 8.0;
    let col_w = w / (cols as f64);

    let bg = core::str::from_utf8(palette.get(Idx::Bg).hex()).unwrap_or("#1e1e2e");
    let hdr_bg = core::str::from_utf8(palette.get(Idx::BgSurface).hex()).unwrap_or("#313244");
    let fg = core::str::from_utf8(palette.get(Idx::Fg).hex()).unwrap_or("#cdd6f4");
    let hdr_fg = core::str::from_utf8(palette.get(Idx::Heading).hex()).unwrap_or("#cba6f7");
    let border = core::str::from_utf8(palette.get(Idx::GridLine).hex()).unwrap_or("#45475a");

    let _ = write!(
        out,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="{w:.0}" height="{h:.0}" viewBox="0 0 {w:.0} {h:.0}">"#
    );
    let _ = write!(
        out,
        r#"<rect width="{w:.0}" height="{h:.0}" rx="4" fill="{bg}" stroke="{border}" stroke-width="1"/>"#
    );

    if n_rows > 0 {
        let _ = write!(
            out,
            r#"<rect x="1" y="1" width="{:.0}" height="{row_h:.0}" rx="3" fill="{hdr_bg}"/>"#,
            w - 2.0
        );
    }

    for (r, row) in grid.iter().enumerate().take(n_rows) {
        let y = 4.0 + (r as f64) * row_h;
        if r > 0 {
            let _ = write!(
                out,
                r#"<line x1="0" y1="{y:.1}" x2="{w:.0}" y2="{y:.1}" stroke="{border}" stroke-width="1"/>"#
            );
        }
        let text_col = if r == 0 { hdr_fg } else { fg };
        let weight = if r == 0 { "bold" } else { "normal" };
        for (c, &cell) in row.iter().enumerate().take(n_cols) {
            let x = (c as f64) * col_w + 10.0;
            let ty = y + 18.0;
            let _ = write!(
                out,
                r#"<text x="{x:.1}" y="{ty:.1}" fill="{text_col}" font-family="monospace" font-size="12" font-weight="{weight}">"#
            );
            write_xml_escaped(out, cell);
            out.push_str("</text>");
        }
    }

    out.push_str("</svg>");
}

/// Render CSV or TSV `src` into an SVG table in `out`.
pub fn render_csv(src: &[u8], palette: &Palette, width: u16, out: &mut String) {
    let first_line = src.split(|&b| b == b'\n').next().unwrap_or(src);
    let delim = if first_line.contains(&b'\t') {
        b'\t'
    } else {
        b','
    };

    let mut grid = [[&b""[..]; MAX_COLS]; MAX_ROWS];
    let mut n_rows = 0usize;
    let mut n_cols = 0usize;

    for line in src.split(|&b| b == b'\n') {
        let line = trim(line);
        if line.is_empty() || n_rows >= MAX_ROWS {
            continue;
        }
        let mut c = 0usize;
        for field in line.split(|&b| b == delim) {
            if c >= MAX_COLS {
                break;
            }
            let f = trim(field);
            let f = if f.len() >= 2 && f.starts_with(b"\"") && f.ends_with(b"\"") {
                &f[1..f.len() - 1]
            } else {
                f
            };
            grid[n_rows][c] = f;
            c += 1;
        }
        if c > n_cols {
            n_cols = c;
        }
        n_rows += 1;
    }

    emit_table_svg(&grid, n_rows, n_cols, palette, width, out);
}

/// Render a Markdown pipe table (`| A | B |\n|---|---|\n| 1 | 2 |`) into an SVG table in `out`.
pub fn render_pipe_table(src: &[u8], palette: &Palette, width: u16, out: &mut String) {
    let mut grid = [[&b""[..]; MAX_COLS]; MAX_ROWS];
    let mut n_rows = 0usize;
    let mut n_cols = 0usize;

    for (line_idx, raw_line) in src.split(|&b| b == b'\n').enumerate() {
        let mut line = trim(raw_line);
        if line.is_empty() || n_rows >= MAX_ROWS {
            continue;
        }
        // Skip the delimiter row (line_idx == 1 if composed of `|-:| `)
        if line_idx == 1
            && line
                .iter()
                .all(|&b| matches!(b, b'|' | b'-' | b':' | b' ' | b'\t'))
        {
            continue;
        }
        if let Some(stripped) = line.strip_prefix(b"|") {
            line = stripped;
        }
        if let Some(stripped) = line.strip_suffix(b"|") {
            line = stripped;
        }
        let mut c = 0usize;
        for field in line.split(|&b| b == b'|') {
            if c >= MAX_COLS {
                break;
            }
            grid[n_rows][c] = trim(field);
            c += 1;
        }
        if c > n_cols {
            n_cols = c;
        }
        n_rows += 1;
    }

    emit_table_svg(&grid, n_rows, n_cols, palette, width, out);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theme::DARK;

    #[test]
    fn csv_and_pipe_table_render_svg() {
        let mut svg = String::new();
        render_csv(b"Name,Score\nAlice,99\nBob,87\n", &DARK, 320, &mut svg);
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains(">Alice</text>"));
        assert!(svg.ends_with("</svg>"));

        svg.clear();
        render_pipe_table(b"| Col1 | Col2 |\n|---|---|\n| X | Y |\n", &DARK, 320, &mut svg);
        assert!(svg.contains(">Col1</text>"));
        assert!(svg.contains(">Y</text>"));
    }
}
