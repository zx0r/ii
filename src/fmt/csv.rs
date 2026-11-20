//! Zero-copy CSV / TSV / Markdown pipe-table parser emitting `TableGrid` directly into `Scene`.

use crate::{
    err::Error,
    fmt::trim_ascii,
    scene::{CellSpan, Scene, TableGrid},
    theme::ColorToken,
};

const MAX_COLS: usize = 64;
const MAX_ROWS: usize = 256;
const CHAR_W: f32 = 8.0;
const CELL_PAD: f32 = 6.0;
const MAX_TABLE_W: f32 = 960.0;
const MIN_COL_W: f32 = 32.0;

#[inline]
fn detect_delim(first_line: &[u8]) -> u8 {
    let trimmed = trim_ascii(first_line);
    if trimmed.starts_with(b"|") && trimmed.ends_with(b"|") {
        return b'|';
    }
    let tabs = memchr::memchr_iter(b'\t', first_line).count();
    let commas = memchr::memchr_iter(b',', first_line).count();
    if tabs > commas { b'\t' } else { b',' }
}

#[inline]
fn is_pipe_separator(line: &[u8]) -> bool {
    let trimmed = trim_ascii(line);
    !trimmed.is_empty()
        && trimmed
            .iter()
            .all(|&byte| matches!(byte, b'|' | b'-' | b':' | b' ' | b'\t'))
}

#[inline]
fn next_field<'a>(line: &'a [u8], delim: u8, cursor: &mut usize) -> Option<&'a [u8]> {
    if *cursor > line.len() {
        return None;
    }
    if *cursor == line.len() {
        *cursor += 1;
        return Some(&[]);
    }
    let rest = &line[*cursor..];
    if delim != b'|' && rest.first() == Some(&b'"') {
        let mut pos = *cursor + 1;
        while pos < line.len() {
            if line[pos] == b'"' {
                if line.get(pos + 1) == Some(&b'"') {
                    pos += 2;
                    continue;
                }
                let inner = &line[*cursor + 1..pos];
                *cursor = memchr::memchr(delim, &line[pos + 1..])
                    .map_or(line.len() + 1, |off| pos + 2 + off);
                return Some(inner);
            }
            pos += 1;
        }
        let inner = &line[*cursor + 1..];
        *cursor = line.len() + 1;
        return Some(inner);
    }

    if let Some(off) = memchr::memchr(delim, rest) {
        *cursor += off + 1;
        Some(&rest[..off])
    } else {
        *cursor = line.len() + 1;
        Some(rest)
    }
}

fn push_table_grid(scene: &mut Scene, cell_start: u32, rows: u16, cols: usize, col_max: &[usize]) {
    let mut widths = [0.0_f32; MAX_COLS];
    let col_cap = (MAX_TABLE_W * 0.6).max(MIN_COL_W);
    let mut total = 0.0_f32;
    for (dst, &chars) in widths[..cols].iter_mut().zip(col_max[..cols].iter()) {
        let px = f32::from(u16::try_from(chars).unwrap_or(u16::MAX))
            .mul_add(CHAR_W, CELL_PAD * 2.0)
            .clamp(MIN_COL_W, col_cap);
        *dst = px;
        total += px;
    }
    if total > MAX_TABLE_W {
        let scale = MAX_TABLE_W / total;
        total = 0.0;
        for w in &mut widths[..cols] {
            *w = (*w * scale).max(24.0);
            total += *w;
        }
    }

    let Ok(width_start) = u32::try_from(scene.col_widths.len()) else {
        return;
    };
    scene.col_widths.extend_from_slice(&widths[..cols]);
    let grid_y = scene.height;
    let row_h = CHAR_W * 2.2;
    let grid_h = f32::from(rows) * row_h;

    scene.grids.push(TableGrid {
        x: 0.0,
        y: grid_y,
        cell_start,
        width_start,
        rows,
        cols: u16::try_from(cols).unwrap_or(u16::MAX),
        border_color: ColorToken::BORDER,
        _pad: [0; 3],
    });
    scene.width = scene.width.max(total);
    scene.height = grid_y + grid_h;
}

fn parse_delimited(src: &[u8], forced_delim: Option<u8>, scene: &mut Scene) {
    let trimmed = trim_ascii(src);
    if trimmed.is_empty() {
        return;
    }
    let first_nl = memchr::memchr(b'\n', trimmed).unwrap_or(trimmed.len());
    let delim = forced_delim.unwrap_or_else(|| detect_delim(&trimmed[..first_nl]));

    let Ok(cell_start) = u32::try_from(scene.cells.len()) else {
        return;
    };
    let mut col_max = [0usize; MAX_COLS];
    let mut max_cols = 0usize;
    let mut row_count = 0usize;
    let mut line_start = 0usize;

    while line_start < trimmed.len() && row_count < MAX_ROWS {
        let nl = memchr::memchr(b'\n', &trimmed[line_start..])
            .map_or(trimmed.len(), |off| line_start + off);
        let mut line = trim_ascii(&trimmed[line_start..nl]);
        line_start = nl + 1;
        if line.is_empty() {
            continue;
        }
        if delim == b'|' {
            if is_pipe_separator(line) {
                continue;
            }
            if let Some(stripped) = line.strip_prefix(b"|") {
                line = stripped;
            }
            if let Some(stripped) = line.strip_suffix(b"|") {
                line = stripped;
            }
        }

        let is_header = row_count == 0;
        let mut cursor = 0usize;
        let mut col = 0usize;
        while col < MAX_COLS {
            let Some(raw) = next_field(line, delim, &mut cursor) else {
                break;
            };
            let cell_bytes = trim_ascii(raw);
            let text = core::str::from_utf8(cell_bytes).unwrap_or("");
            let (text_off, text_len) = scene.intern_text(text).unwrap_or((0, 0));
            col_max[col] = col_max[col].max(text.chars().count());
            scene.cells.push(CellSpan {
                text_off,
                text_len,
                col_span: 1,
                row_span: 1,
                color: if is_header {
                    ColorToken::ACCENT_ORANGE
                } else {
                    ColorToken::FG_PRIMARY
                },
                bg: if is_header {
                    ColorToken::BG_SURFACE
                } else {
                    ColorToken::BG_PRIMARY
                },
                align: 0,
                flags: u8::from(is_header),
            });
            col += 1;
        }
        if col > 0 {
            max_cols = max_cols.max(col);
            row_count += 1;
        }
    }

    if row_count > 0 && max_cols > 0 {
        push_table_grid(
            scene,
            cell_start,
            u16::try_from(row_count).unwrap_or(u16::MAX),
            max_cols,
            &col_max,
        );
    }
}

/// Parse CSV or Markdown pipe-table bytes into `Scene` (`TableGrid`).
///
/// # Errors
/// Returns `Ok(())` on empty or valid delimited inputs.
pub fn parse_into_scene(src: &[u8], scene: &mut Scene) -> Result<(), Error> {
    parse_delimited(src, None, scene);
    Ok(())
}

/// Parse TSV bytes (`\t`-delimited) into `Scene` (`TableGrid`).
///
/// # Errors
/// Returns `Ok(())` on empty or valid TSV inputs.
pub fn parse_tsv_into_scene(src: &[u8], scene: &mut Scene) -> Result<(), Error> {
    parse_delimited(src, Some(b'\t'), scene);
    Ok(())
}
