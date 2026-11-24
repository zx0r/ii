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

#[derive(Clone, Copy)]
struct Field<'a> {
    value: &'a [u8],
    quoted: bool,
    last: bool,
}

impl<'a> Field<'a> {
    fn bytes(self) -> impl Iterator<Item = u8> + use<'a> {
        let mut src = self.value.iter().copied().peekable();
        core::iter::from_fn(move || {
            let byte = src.next()?;
            if self.quoted && byte == b'"' && src.peek() == Some(&b'"') {
                src.next();
            }
            Some(byte)
        })
    }
}

fn fields(src: &[u8], delim: u8) -> impl Iterator<Item = Result<Field<'_>, Error>> {
    let mut rest = (!src.is_empty()).then_some(src);
    core::iter::from_fn(move || {
        let src = rest.take()?;
        let quoted = delim != b'|' && src.starts_with(b"\"");
        let mut end_quote = 0;
        let mut after_quote = 0;
        if quoted {
            let mut at = 1;
            loop {
                let Some(pos) = memchr::memchr(b'"', &src[at..]).map(|next| at + next) else {
                    return Some(Err(Error::InvalidState));
                };
                if src.get(pos + 1) != Some(&b'"') {
                    end_quote = pos;
                    after_quote = pos + 1;
                    break;
                }
                at = pos + 2;
            }
        }
        let end = memchr::memchr2(delim, b'\n', &src[after_quote..])
            .map_or(src.len(), |off| after_quote + off);
        let last = src.get(end).is_none_or(|&byte| byte == b'\n');
        if end < src.len() && (!last || end + 1 < src.len()) {
            rest = Some(&src[end + 1..]);
        }
        let value = if quoted {
            if !src[after_quote..end].trim_ascii().is_empty() {
                rest = None;
                return Some(Err(Error::InvalidState));
            }
            &src[1..end_quote]
        } else {
            &src[..end]
        };
        Some(Ok(Field {
            value,
            quoted,
            last,
        }))
    })
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

fn parse_delimited(src: &[u8], forced_delim: Option<u8>, scene: &mut Scene) -> Result<(), Error> {
    let first_nl = memchr::memchr(b'\n', src).unwrap_or(src.len());
    let delim = forced_delim.unwrap_or_else(|| detect_delim(&src[..first_nl]));
    if delim == b'|' {
        let fields = src
            .split(|&b| b == b'\n')
            .map(trim_ascii)
            .filter(|line| !is_pipe_separator(line))
            .flat_map(|line| {
                let line = line.strip_prefix(b"|").unwrap_or(line);
                fields(line.strip_suffix(b"|").unwrap_or(line), delim)
            });
        table(fields, scene)
    } else {
        table(fields(src, delim), scene)
    }
}

fn table<'a>(
    fields: impl Iterator<Item = Result<Field<'a>, Error>>,
    scene: &mut Scene,
) -> Result<(), Error> {
    let Ok(cell_start) = u32::try_from(scene.cells.len()) else {
        return Err(Error::ResourceLimit);
    };
    let mut col_max = [0; MAX_COLS];
    let mut ends = [0; MAX_ROWS];
    let mut max_cols = 0;
    let mut row = 0;
    let mut col = 0;
    let mut decoded = Vec::new();

    for field in fields {
        let field = field?;
        if col == 0 && field.last && !field.quoted && field.value.trim_ascii().is_empty() {
            continue;
        }
        if col < MAX_COLS {
            let value = if field.quoted && memchr::memmem::find(field.value, b"\"\"").is_some() {
                decoded.clear();
                decoded.extend(field.bytes());
                decoded.as_slice()
            } else {
                field.value
            };
            let text = core::str::from_utf8(trim_ascii(value)).unwrap_or("");
            let (off, len) = scene.intern_text(text).unwrap_or((0, 0));
            col_max[col] = col_max[col].max(text.chars().count());
            scene.cells.push(CellSpan {
                text_off: off,
                text_len: len,
                col_span: 1,
                row_span: 1,
                color: if row == 0 {
                    ColorToken::ACCENT_ORANGE
                } else {
                    ColorToken::FG_PRIMARY
                },
                bg: if row == 0 {
                    ColorToken::BG_SURFACE
                } else {
                    ColorToken::BG_PRIMARY
                },
                align: 0,
                flags: u8::from(row == 0),
            });
            col += 1;
        }
        if field.last {
            max_cols = max_cols.max(col);
            ends[row] = scene.cells.len();
            row += 1;
            col = 0;
            if row == MAX_ROWS {
                break;
            }
        }
    }

    if row == 0 {
        return Ok(());
    }
    let base = cell_start as usize;
    let empty = CellSpan {
        text_off: 0,
        text_len: 0,
        col_span: 1,
        row_span: 1,
        color: ColorToken::FG_PRIMARY,
        bg: ColorToken::BG_PRIMARY,
        align: 0,
        flags: 0,
    };
    let mut ranges = ends[..row].iter().enumerate().rev().peekable();
    scene.cells.resize(base + row * max_cols, empty);
    while let Some((index, &end)) = ranges.next() {
        let start = ranges.peek().map_or(base, |(_, start)| **start);
        let dst = base + index * max_cols;
        scene.cells.copy_within(start..end, dst);
        scene.cells[dst + end - start..dst + max_cols].fill(empty);
    }
    push_table_grid(
        scene,
        cell_start,
        u16::try_from(row).unwrap_or(u16::MAX),
        max_cols,
        &col_max,
    );
    Ok(())
}

/// Parse CSV or Markdown pipe-table bytes into `Scene` (`TableGrid`).
///
/// # Errors
/// Returns `Ok(())` on empty or valid delimited inputs.
pub fn parse_into_scene(src: &[u8], scene: &mut Scene) -> Result<(), Error> {
    parse_delimited(src, None, scene)
}

/// Parse TSV bytes (`\t`-delimited) into `Scene` (`TableGrid`).
///
/// # Errors
/// Returns `Ok(())` on empty or valid TSV inputs.
pub fn parse_tsv_into_scene(src: &[u8], scene: &mut Scene) -> Result<(), Error> {
    parse_delimited(src, Some(b'\t'), scene)
}

/// Present all delimited records as a native text table, including multiline fields.
///
/// # Errors
/// Rejects an unterminated quoted field.
pub fn to_markdown(src: &[u8], delim: u8, out: &mut Vec<u8>) -> Result<(), Error> {
    let mut cols = 0;
    let mut header = true;
    for field in fields(src, delim) {
        let field = field?;
        if cols == 0 {
            out.extend_from_slice(b"| ");
        }
        let mut bytes = field.bytes().peekable();
        while let Some(byte) = bytes.next() {
            match byte {
                b'|' => out.extend_from_slice("│".as_bytes()),
                b'\n' => out.extend_from_slice("↵".as_bytes()),
                b'\r'
                    if bytes.peek() == Some(&b'\n')
                        || !field.quoted && field.last && bytes.peek().is_none() => {}
                _ => out.push(byte),
            }
        }
        cols += 1;
        if field.last {
            out.extend_from_slice(b" |\n");
            if header {
                out.push(b'|');
                for _ in 0..cols {
                    out.extend_from_slice(b" --- |");
                }
                out.push(b'\n');
                header = false;
            }
            cols = 0;
        } else {
            out.extend_from_slice(b" | ");
        }
    }
    Ok(())
}
