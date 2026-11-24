//! Cell-aware pagination of ANSI text. Source offsets never enter this coordinate space.

/// Append the requested visual rows, restoring SGR state inherited from skipped rows.
pub fn append(src: &[u8], cols: u16, skip: u32, max: u32, out: &mut Vec<u8>) {
    scan(src, cols, skip, max, out, Vec::new(), |_, _, _| {});
}

/// Sparse visual-row checkpoints for a single immutable source and width.
/// A page can resume inside a wrapped line with its original foreground/style.
#[derive(Debug, Default)]
pub(crate) struct RowIndex(Vec<(usize, Box<[u8]>)>);

impl RowIndex {
    const STRIDE: u32 = 32;
    const MAX_ENTRIES: usize = 1024;

    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }

    pub(crate) fn page(&mut self, src: &[u8], cols: u16, skip: u32, rows: u32, out: &mut Vec<u8>) {
        let block = ((skip / Self::STRIDE) as usize).min(self.0.len());
        let (offset, sgr) = block.checked_sub(1).map_or((0, Vec::new()), |i| {
            let (offset, sgr) = &self.0[i];
            (*offset, sgr.to_vec())
        });
        let base = u32::try_from(block).unwrap_or(0) * Self::STRIDE;
        let mut next = (self.0.len() as u64 + 1) * u64::from(Self::STRIDE) - u64::from(base);
        scan(
            &src[offset..],
            cols,
            skip - base,
            rows,
            out,
            sgr,
            |row, pos, state| {
                if row == next && self.0.len() < Self::MAX_ENTRIES {
                    self.0.push((offset + pos, state.into()));
                    next += u64::from(Self::STRIDE);
                }
            },
        );
    }
}

fn scan(
    src: &[u8],
    cols: u16,
    skip: u32,
    max: u32,
    out: &mut Vec<u8>,
    mut sgr: Vec<u8>,
    mut checkpoint: impl FnMut(u64, usize, &[u8]),
) {
    let width = usize::from(cols.max(1));
    let end = if max == 0 {
        u64::MAX
    } else {
        u64::from(skip) + u64::from(max)
    };
    let mut row = 0u64;
    let mut col = 0usize;
    let mut pos = 0;
    let mut started = false;
    let mut graphic_left = 0;
    while pos < src.len() && row < end {
        if src[pos..].starts_with(b"\x1b[") {
            let tail = &src[pos + 2..];
            if let Some(len) = tail.iter().position(|b| (0x40..=0x7e).contains(b)) {
                let seq = &src[pos..pos + len + 3];
                if seq.ends_with(b"m") {
                    if seq == b"\x1b[0m" || seq == b"\x1b[m" {
                        sgr.clear();
                    }
                    if sgr.len() + seq.len() <= 4096 {
                        sgr.extend_from_slice(seq);
                    }
                }
                if row >= u64::from(skip) {
                    out.extend_from_slice(seq);
                }
                pos += seq.len();
                continue;
            }
        }
        let graphic = src[pos..].starts_with(&crate::kitty::PLACEHOLDER_UTF8);
        if graphic && graphic_left == 0 {
            let tail = &src[pos..];
            let stop = memchr::memchr2(0x1b, b'\n', tail).unwrap_or(tail.len());
            graphic_left =
                memchr::memmem::find_iter(&tail[..stop], &crate::kitty::PLACEHOLDER_UTF8).count();
            if col > 0 && graphic_left <= width && col + graphic_left > width {
                if row >= u64::from(skip) {
                    out.push(b'\n');
                }
                row += 1;
                col = 0;
                checkpoint(row, pos, &sgr);
                if row >= end {
                    break;
                }
            }
        }
        let byte = src[pos];
        let (len, mut cells) = if byte == b'\n' {
            (1, 0)
        } else if byte == b'\t' {
            (1, 4 - col % 4)
        } else if byte < 128 {
            (1, 1)
        } else {
            let len = if byte < 0xe0 {
                2
            } else if byte < 0xf0 {
                3
            } else {
                4
            };
            let ch = src
                .get(pos..pos + len)
                .and_then(|b| std::str::from_utf8(b).ok())
                .and_then(|s| s.chars().next());
            ch.map_or((1, 1), |ch| (len, cells(ch)))
        };
        if byte != b'\n' && cells > 0 && col > 0 && col + cells > width {
            if row >= u64::from(skip) {
                out.push(b'\n');
            }
            row += 1;
            col = 0;
            checkpoint(row, pos, &sgr);
            if byte == b'\t' {
                cells = 4;
            }
            if row >= end {
                break;
            }
        }
        if row >= u64::from(skip) {
            if !started {
                out.extend_from_slice(&sgr);
                started = true;
            }
            if byte == b'\t' {
                out.resize(out.len() + cells.min(width), b' ');
            } else {
                out.extend_from_slice(&src[pos..pos + len]);
            }
        }
        if byte == b'\n' {
            row += 1;
            col = 0;
            checkpoint(row, pos + len, &sgr);
        } else {
            col += cells;
        }
        if graphic {
            graphic_left = graphic_left.saturating_sub(1);
        }
        pos += len;
    }
    if started && !sgr.is_empty() {
        out.extend_from_slice(b"\x1b[0m");
    }
}

/// Terminal width policy for common combining marks and East Asian wide characters.
/// Ambiguous characters occupy one cell. Joined emoji sequences remain host dependent.
#[must_use]
pub fn cells(ch: char) -> usize {
    // Every row selector in the graphics protocol is combining, including marks
    // outside the common text ranges below. Counting one shifts the image grid.
    if ('\u{300}'..='\u{1d244}').contains(&ch) {
        let mut utf8 = [0; 4];
        let bytes = ch.encode_utf8(&mut utf8).as_bytes();
        if crate::kitty::DIACRITICS_UTF8.binary_search(&bytes).is_ok() {
            return 0;
        }
    }
    match ch as u32 {
        0..=31
        | 127..=159
        | 0x300..=0x36f
        | 0x483..=0x489
        | 0x591..=0x5bd
        | 0x5bf
        | 0x5c1..=0x5c2
        | 0x610..=0x61a
        | 0x64b..=0x65f
        | 0x670
        | 0x6d6..=0x6ed
        | 0x1ab0..=0x1aff
        | 0x1dc0..=0x1dff
        | 0x200b..=0x200f
        | 0x202a..=0x202e
        | 0x2060..=0x206f
        | 0x20d0..=0x20ff
        | 0xfe00..=0xfe0f
        | 0xfe20..=0xfe2f
        | 0xe0100..=0xe01ef => 0,
        0x1100..=0x115f
        | 0x2329..=0x232a
        | 0x2e80..=0xa4cf
        | 0xac00..=0xd7a3
        | 0xf900..=0xfaff
        | 0xfe10..=0xfe19
        | 0xfe30..=0xfe6f
        | 0xff01..=0xff60
        | 0xffe0..=0xffe6
        | 0x1f300..=0x1faff
        | 0x20000..=0x3fffd => 2,
        _ => 1,
    }
}

#[cfg(test)]
mod tests {
    use super::{RowIndex, append};

    #[test]
    fn indexed_scroll_matches_full_layout_in_both_directions() {
        let mut src = "abc界\t1234567890\n\x1b[1;32mcontinued αβγ "
            .repeat(63)
            .into_bytes();
        crate::kitty::emit_placeholder(42, 12, 7, &mut src);
        src.extend_from_slice(b"\x1b[0mend");
        for cols in [3, 7, 13] {
            let mut index = RowIndex::default();
            for skip in (0..320).rev().chain(0..320) {
                let mut reference = Vec::new();
                append(&src, cols, skip, 7, &mut reference);
                let mut page = Vec::new();
                index.page(&src, cols, skip, 7, &mut page);
                assert_eq!(page, reference, "width {cols}, skip {skip}");
            }
        }
    }
}
