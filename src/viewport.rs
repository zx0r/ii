//! Cell-aware pagination of ANSI text. Source offsets never enter this coordinate space.

/// Append the requested visual rows, restoring SGR state inherited from skipped rows.
pub fn append(src: &[u8], cols: u16, skip: u32, max: u32, out: &mut Vec<u8>) {
    let width = usize::from(cols.max(1));
    let end = if max == 0 {
        u64::MAX
    } else {
        u64::from(skip) + u64::from(max)
    };
    let mut row = 0u64;
    let mut col = 0usize;
    let mut pos = 0;
    let mut sgr = Vec::new();
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
                if row >= end {
                    break;
                }
            }
        }
        let byte = src[pos];
        let (len, cells) = if byte == b'\n' {
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
