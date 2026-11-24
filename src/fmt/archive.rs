//! Bounded archive inspector for ZIP/JAR/WHL, POSIX TAR, and compressed tarballs.

use crate::{
    err::Error,
    scene::{CellSpan, Scene, TableGrid},
    theme::{ColorToken, Palette, SGR_RESET, SemanticRole},
};

const MAX_ROWS: usize = 128;
const MAX_DEPTH: usize = 64;
const ZIP_EOCD: [u8; 4] = *b"PK\x05\x06";
const ZIP_CDH: [u8; 4] = *b"PK\x01\x02";
const GZIP_MAGIC: [u8; 2] = [0x1F, 0x8B];
const BZIP2_MAGIC: [u8; 3] = *b"BZh";
const XZ_MAGIC: [u8; 6] = [0xFD, 0x37, 0x7A, 0x58, 0x5A, 0x00];
const ZSTD_MAGIC: [u8; 4] = [0x28, 0xB5, 0x2F, 0xFD];

#[inline]
#[must_use]
pub fn looks_like_archive(src: &[u8]) -> bool {
    src.starts_with(b"PK\x03\x04")
        || src.starts_with(&ZIP_EOCD)
        || src.starts_with(&GZIP_MAGIC)
        || src.starts_with(&BZIP2_MAGIC)
        || src.starts_with(&XZ_MAGIC)
        || src.starts_with(&ZSTD_MAGIC)
        || is_tar_header(src)
}

#[inline]
fn is_tar_header(src: &[u8]) -> bool {
    src.len() >= 512
        && (&src[257..262] == b"ustar" || (src[0] != 0 && parse_octal(&src[124..136]).is_some()))
}

#[inline]
fn read_u16_le(src: &[u8], off: usize) -> Option<u16> {
    let b = src.get(off..off + 2)?;
    Some(u16::from_le_bytes([b[0], b[1]]))
}

#[inline]
fn read_u32_le(src: &[u8], off: usize) -> Option<u32> {
    let b = src.get(off..off + 4)?;
    Some(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

#[inline]
fn parse_octal(field: &[u8]) -> Option<u64> {
    let mut val = 0u64;
    let mut saw_digit = false;
    for &b in field {
        if b == 0 || b == b' ' {
            if saw_digit {
                break;
            }
            continue;
        }
        if !(b'0'..=b'7').contains(&b) {
            return None;
        }
        saw_digit = true;
        val = val.checked_mul(8)?.checked_add(u64::from(b - b'0'))?;
    }
    saw_digit.then_some(val)
}

#[inline]
fn fmt_bytes_stack(mut bytes: u64, buf: &mut [u8; 16]) -> &str {
    const UNITS: [u8; 4] = *b"BKMG";
    let mut unit = 0usize;
    while bytes >= 1024 && unit < 3 {
        bytes /= 1024;
        unit += 1;
    }
    let mut idx = 15;
    buf[idx] = UNITS[unit];
    loop {
        idx -= 1;
        buf[idx] = b'0' + u8::try_from(bytes % 10).unwrap_or(0);
        bytes /= 10;
        if bytes == 0 || idx == 0 {
            break;
        }
    }
    core::str::from_utf8(&buf[idx..16]).unwrap_or("0B")
}

fn push_cell(scene: &mut Scene, text: &str, is_header: bool, align: u8) {
    let (text_off, text_len) = scene.intern_text(text).unwrap_or((0, 0));
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
        align,
        flags: u8::from(is_header),
    });
}

fn push_row3(scene: &mut Scene, c0: &str, c1: &str, c2: &str, is_header: bool) {
    push_cell(scene, c0, is_header, 0);
    push_cell(scene, c1, is_header, 2);
    push_cell(scene, c2, is_header, 2);
}

fn finalize_grid(scene: &mut Scene, cell_start: u32, rows: usize) {
    let Ok(width_start) = u32::try_from(scene.col_widths.len()) else {
        return;
    };
    scene.col_widths.extend_from_slice(&[380.0, 96.0, 96.0]);
    let grid_y = scene.height;
    let rows_u16 = u16::try_from(rows).unwrap_or(u16::MAX);
    scene.grids.push(TableGrid {
        x: 0.0,
        y: grid_y,
        cell_start,
        width_start,
        rows: rows_u16,
        cols: 3,
        border_color: ColorToken::BORDER,
        _pad: [0; 3],
    });
    scene.width = scene.width.max(572.0);
    scene.height = f32::from(rows_u16).mul_add(18.0, grid_y);
}

#[derive(Clone, Copy)]
enum EntryKind {
    Zip(u32),
    Tar(u8),
}

struct Entry<'a> {
    name: &'a str,
    size: u64,
    kind: EntryKind,
}

impl Entry<'_> {
    fn is_dir(&self) -> bool {
        matches!(self.kind, EntryKind::Tar(b'5')) || self.name.ends_with('/')
    }
}

fn zip<'a>(src: &'a [u8], mut entry: impl FnMut(Entry<'a>)) -> bool {
    if src.len() < 22 {
        return false;
    }
    let search_start = src.len().saturating_sub(65_557);
    let Some(eocd_rel) = src[search_start..].windows(4).rposition(|w| w == ZIP_EOCD) else {
        return false;
    };
    let eocd = search_start + eocd_rel;
    let (Some(entries), Some(cd_off)) = (read_u16_le(src, eocd + 10), read_u32_le(src, eocd + 16))
    else {
        return false;
    };

    let mut off = cd_off as usize;
    for _ in 0..usize::from(entries).min(MAX_ROWS - 1) {
        if src.get(off..off + 4) != Some(&ZIP_CDH) {
            break;
        }
        let (Some(packed), Some(unpacked), Some(n_len), Some(x_len), Some(c_len)) = (
            read_u32_le(src, off + 20),
            read_u32_le(src, off + 24),
            read_u16_le(src, off + 28),
            read_u16_le(src, off + 30),
            read_u16_le(src, off + 32),
        ) else {
            break;
        };
        let name_end = off + 46 + usize::from(n_len);
        let Some(name_bytes) = src.get(off + 46..name_end) else {
            break;
        };
        let name = core::str::from_utf8(name_bytes).unwrap_or("<non-utf8>");
        entry(Entry {
            name,
            size: u64::from(unpacked),
            kind: EntryKind::Zip(packed),
        });
        off = name_end + usize::from(x_len) + usize::from(c_len);
    }
    true
}

fn tar<'a>(src: &'a [u8], mut entry: impl FnMut(Entry<'a>)) -> bool {
    if !is_tar_header(src) {
        return false;
    }
    let mut off = 0usize;
    for _ in 1..MAX_ROWS {
        let Some(hdr) = src.get(off..off + 512) else {
            break;
        };
        if hdr[0] == 0 {
            break;
        }
        let end = memchr::memchr(0, &hdr[..100]).unwrap_or(100);
        let name = core::str::from_utf8(&hdr[..end]).unwrap_or("<non-utf8>");
        let Some(size) = parse_octal(&hdr[124..136]) else {
            break;
        };
        entry(Entry {
            name,
            size,
            kind: EntryKind::Tar(hdr[156]),
        });
        let Some(next) = usize::try_from(size)
            .ok()
            .and_then(|size| size.checked_next_multiple_of(512))
            .and_then(|bytes| off.checked_add(512)?.checked_add(bytes))
            .filter(|&next| next <= src.len())
        else {
            break;
        };
        off = next;
    }
    true
}

fn entries<'a>(src: &'a [u8], emit: impl FnMut(Entry<'a>)) -> bool {
    if src.starts_with(b"PK") {
        zip(src, emit)
    } else {
        tar(src, emit)
    }
}

fn parse_compressed_header(src: &[u8], scene: &mut Scene) {
    let Ok(cell_start) = u32::try_from(scene.cells.len()) else {
        return;
    };
    push_row3(scene, "Container", "Packed", "Unpacked", true);

    let mut pk_buf = [0u8; 16];
    let mut un_buf = [0u8; 16];
    let packed = fmt_bytes_stack(src.len() as u64, &mut pk_buf);

    let (label, unpacked) = if src.starts_with(&GZIP_MAGIC) {
        let isize_val = src
            .len()
            .checked_sub(4)
            .and_then(|off| read_u32_le(src, off))
            .map_or("unknown", |n| fmt_bytes_stack(u64::from(n), &mut un_buf));
        ("tar.gz (gzip)", isize_val)
    } else if src.starts_with(&BZIP2_MAGIC) {
        ("tar.bz2 (bzip2)", "stream")
    } else if src.starts_with(&XZ_MAGIC) {
        ("tar.xz (xz)", "stream")
    } else if src.starts_with(&ZSTD_MAGIC) {
        ("tar.zst (zstd)", "stream")
    } else {
        ("archive", "unknown")
    };

    push_row3(scene, label, packed, unpacked, false);
    finalize_grid(scene, cell_start, 2);
}

/// Inspect ZIP/JAR/WHL, TAR, or compressed tar container into `Scene` (`TableGrid`).
///
/// # Errors
/// Never fails on unknown containers; degrades honestly to container header metadata.
pub fn parse_into_scene(src: &[u8], scene: &mut Scene) -> Result<(), Error> {
    let Ok(cell_start) = u32::try_from(scene.cells.len()) else {
        return Ok(());
    };
    let packed = src.starts_with(b"PK");
    if !packed && !is_tar_header(src) {
        parse_compressed_header(src, scene);
        return Ok(());
    }
    push_row3(
        scene,
        "Entry",
        "Size",
        if packed { "Packed" } else { "Type" },
        true,
    );
    let mut rows = 1;
    let entry = |entry: Entry<'_>| {
        let mut size = [0; 16];
        let mut packed = [0; 16];
        let label = match entry.kind {
            EntryKind::Zip(n) => fmt_bytes_stack(u64::from(n), &mut packed),
            EntryKind::Tar(b'5') => "dir",
            EntryKind::Tar(b'2') => "symlink",
            EntryKind::Tar(b'1') => "link",
            EntryKind::Tar(_) => "file",
        };
        push_row3(
            scene,
            entry.name,
            fmt_bytes_stack(entry.size, &mut size),
            label,
            false,
        );
        rows += 1;
    };
    if entries(src, entry) {
        finalize_grid(scene, cell_start, rows);
    }
    Ok(())
}

const MAX_EXPANDED: u64 = 64 * 1024 * 1024;

fn decompress(src: &[u8]) -> Result<Vec<u8>, Error> {
    use std::io::Read;
    fn append(reader: impl Read, out: &mut Vec<u8>) -> Result<(), Error> {
        let remaining = MAX_EXPANDED.saturating_sub(out.len() as u64);
        reader
            .take(remaining + 1)
            .read_to_end(out)
            .map_err(|_| Error::ArchiveDecode)?;
        if out.len() as u64 > MAX_EXPANDED {
            return Err(Error::ResourceLimit);
        }
        Ok(())
    }
    let mut out = Vec::new();
    if src.starts_with(&GZIP_MAGIC) {
        append(flate2::read::MultiGzDecoder::new(src), &mut out)?;
    } else if src.starts_with(&BZIP2_MAGIC) {
        append(bzip2_rs::DecoderReader::new(src), &mut out)?;
    } else if src.starts_with(&XZ_MAGIC) {
        append(
            lzma_rust2::XzReader::new_mem_limit(src, true, 65536),
            &mut out,
        )?;
    } else if src.starts_with(&ZSTD_MAGIC) {
        let mut cursor = std::io::Cursor::new(src);
        while cursor.position() < src.len() as u64 {
            let decoder = ruzstd::decoding::StreamingDecoder::new_with_max_window_size(
                &mut cursor,
                MAX_EXPANDED,
            )
            .map_err(|_| Error::ArchiveDecode)?;
            append(decoder, &mut out)?;
        }
    } else {
        return Err(Error::ArchiveDecode);
    }
    Ok(out)
}

/// Render inspected paths as a native hierarchy; never invent compressed members.
///
/// # Errors
/// Propagates archive inspection failures.
pub fn to_tree(src: &[u8], out: &mut Vec<u8>) -> Result<(), Error> {
    to_tree_with_palette(src, &crate::theme::DARK, out)
}

/// Render a tree using the receiving host's theme.
///
/// # Errors
/// Propagates bounded archive decoding errors.
pub fn to_tree_with_palette(src: &[u8], palette: &Palette, out: &mut Vec<u8>) -> Result<(), Error> {
    let expanded;
    let src = if src.starts_with(&GZIP_MAGIC)
        || src.starts_with(&BZIP2_MAGIC)
        || src.starts_with(&XZ_MAGIC)
        || src.starts_with(&ZSTD_MAGIC)
    {
        expanded = decompress(src)?;
        if !is_tar_header(&expanded) && !expanded.starts_with(b"PK") {
            return Err(Error::ArchiveDecode);
        }
        expanded.as_slice()
    } else {
        src
    };
    let mut paths = std::collections::BTreeMap::<Vec<&str>, bool>::new();
    let mut truncated = false;
    if !entries(src, |entry| {
        let path: Vec<_> = entry
            .name
            .split('/')
            .filter(|part| !part.is_empty())
            .collect();
        if entry.name.len() > 4096 || path.len() > MAX_DEPTH {
            truncated = true;
            return;
        }
        for depth in 1..=path.len() {
            let dir = depth < path.len() || entry.is_dir();
            paths
                .entry(path[..depth].to_vec())
                .and_modify(|prev| *prev |= dir)
                .or_insert(dir);
        }
    }) {
        return Err(Error::ArchiveDecode);
    }
    if truncated || paths.len() > 4096 {
        return Err(Error::ResourceLimit);
    }

    let paths: Vec<_> = paths.into_iter().collect();
    let mut next_at_depth = [None::<usize>; MAX_DEPTH];
    let mut more = vec![false; paths.len()];
    for (i, (path, _)) in paths.iter().enumerate().rev() {
        let depth = path.len() - 1;
        more[i] = next_at_depth[depth].is_some_and(|next| paths[next].0[..depth] == path[..depth]);
        next_at_depth[depth] = Some(i);
    }

    palette.write_sgr_fg(out, SemanticRole::Info);
    out.extend_from_slice("󰉋 archive/".as_bytes());
    out.extend_from_slice(SGR_RESET);
    out.push(b'\n');
    let mut bars = [false; MAX_DEPTH];
    for ((path, directory), more) in paths.into_iter().zip(more) {
        let depth = path.len() - 1;
        palette.write_sgr_fg(out, SemanticRole::Border);
        for &bar in &bars[..depth] {
            out.extend_from_slice(if bar { "│   ".as_bytes() } else { b"    " });
        }
        out.extend_from_slice(if more {
            "├── ".as_bytes()
        } else {
            "└── ".as_bytes()
        });
        bars[depth] = more;
        palette.write_sgr_fg(
            out,
            if directory {
                SemanticRole::Info
            } else {
                SemanticRole::TextPrimary
            },
        );
        out.extend_from_slice(if directory {
            "󰉋 ".as_bytes()
        } else {
            "󰈙 ".as_bytes()
        });
        crate::style::emit_literal(path[depth].as_bytes(), out);
        if directory {
            out.push(b'/');
        }
        out.extend_from_slice(SGR_RESET);
        out.push(b'\n');
    }
    Ok(())
}
