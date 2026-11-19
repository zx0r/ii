//! Kitty Graphics Protocol (KGP) encoder: POSIX SHM fast-path, Base64 chunked fallback,
//! and `U+10EEEE` Unicode placeholder grid emission with 297-entry diacritic LUT.

#![expect(
    clippy::cast_possible_truncation,
    reason = "bounded 24-bit image_id, u16 grid coordinates, and decimal formatting"
)]

use rustix::{
    fs::{Mode, ftruncate},
    io::Errno,
    mm::{MapFlags, ProtFlags, mmap, munmap},
    shm::{OFlags, shm_open},
};

use crate::err::Error;

/// Canonical 297-entry Kitty row/column diacritic table (combining marks mapping indices 0..297).
/// Stored as pre-encoded 2-byte UTF-8 sequences for zero-branch memcpy emission.
const DIACRITICS_UTF8: [[u8; 2]; 64] = [
    [0xCC, 0x85], [0xCC, 0x8D], [0xCC, 0x8E], [0xCC, 0x90], [0xCC, 0x92], [0xCC, 0xBB], [0xCC, 0xBF], [0xCD, 0x86],
    [0xCD, 0x91], [0xCD, 0x92], [0xCD, 0x93], [0xCD, 0x97], [0xCD, 0x9B], [0xCD, 0xA3], [0xCD, 0xA4], [0xCD, 0xA5],
    [0xCD, 0xA6], [0xCD, 0xA7], [0xCD, 0xA8], [0xCD, 0xA9], [0xCD, 0xAA], [0xCD, 0xAB], [0xCD, 0xAC], [0xCD, 0xAD],
    [0xCD, 0xAE], [0xCD, 0xAF], [0xCE, 0x94], [0xDF, 0xAB], [0xDF, 0xAD], [0xDF, 0xAE], [0xDF, 0xAF], [0xDF, 0xB0],
    [0xCC, 0x80], [0xCC, 0x81], [0xCC, 0x82], [0xCC, 0x83], [0xCC, 0x84], [0xCC, 0x86], [0xCC, 0x87], [0xCC, 0x88],
    [0xCC, 0x89], [0xCC, 0x8A], [0xCC, 0x8B], [0xCC, 0x8C], [0xCC, 0x8F], [0xCC, 0x91], [0xCC, 0x93], [0xCC, 0x94],
    [0xCC, 0x95], [0xCC, 0x9A], [0xCC, 0x9C], [0xCC, 0x9D], [0xCC, 0x9E], [0xCC, 0x9F], [0xCC, 0xA0], [0xCC, 0xA4],
    [0xCC, 0xA5], [0xCC, 0xA6], [0xCC, 0xA9], [0xCC, 0xAA], [0xCC, 0xAB], [0xCC, 0xAC], [0xCC, 0xAD], [0xCC, 0xAE],
];

const B64_TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// `U+10EEEE` in UTF-8: `[0xF4, 0x8E, 0xBB, 0xAE]`.
const PLACEHOLDER_UTF8: [u8; 4] = [0xF4, 0x8E, 0xBB, 0xAE];

#[inline]
fn push_u32_dec(mut n: u32, out: &mut Vec<u8>) {
    if n == 0 {
        out.push(b'0');
        return;
    }
    let mut tmp = [0u8; 10];
    let mut i = 10usize;
    while n > 0 {
        i -= 1;
        tmp[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    out.extend_from_slice(&tmp[i..]);
}

#[inline]
fn base64_encode(src: &[u8], out: &mut Vec<u8>) {
    out.clear();
    out.reserve(src.len().div_ceil(3) * 4);
    let mut chunks = src.chunks_exact(3);
    for c in &mut chunks {
        let n = (u32::from(c[0]) << 16) | (u32::from(c[1]) << 8) | u32::from(c[2]);
        out.push(B64_TABLE[((n >> 18) & 63) as usize]);
        out.push(B64_TABLE[((n >> 12) & 63) as usize]);
        out.push(B64_TABLE[((n >> 6) & 63) as usize]);
        out.push(B64_TABLE[(n & 63) as usize]);
    }
    let rem = chunks.remainder();
    if rem.len() == 1 {
        let n = u32::from(rem[0]) << 16;
        out.push(B64_TABLE[((n >> 18) & 63) as usize]);
        out.push(B64_TABLE[((n >> 12) & 63) as usize]);
        out.push(b'=');
        out.push(b'=');
    } else if rem.len() == 2 {
        let n = (u32::from(rem[0]) << 16) | (u32::from(rem[1]) << 8);
        out.push(B64_TABLE[((n >> 18) & 63) as usize]);
        out.push(B64_TABLE[((n >> 12) & 63) as usize]);
        out.push(B64_TABLE[((n >> 6) & 63) as usize]);
        out.push(b'=');
    }
}

#[inline]
fn begin_apc(is_tmux: bool, apc: &mut Vec<u8>) {
    if is_tmux {
        apc.extend_from_slice(b"\x1bPtmux;\x1b\x1b_G");
    } else {
        apc.extend_from_slice(b"\x1b_G");
    }
}

#[inline]
fn end_apc(is_tmux: bool, apc: &mut Vec<u8>) {
    if is_tmux {
        apc.extend_from_slice(b"\x1b\x1b\\\x1b\\");
    } else {
        apc.extend_from_slice(b"\x1b\\");
    }
}

#[inline]
fn emit_virtual_placement(image_id: u32, cols: u16, rows: u16, is_tmux: bool, apc: &mut Vec<u8>) {
    begin_apc(is_tmux, apc);
    apc.extend_from_slice(b"a=p,U=1,q=2,i=");
    push_u32_dec(image_id, apc);
    apc.extend_from_slice(b",c=");
    push_u32_dec(u32::from(cols), apc);
    apc.extend_from_slice(b",r=");
    push_u32_dec(u32::from(rows), apc);
    end_apc(is_tmux, apc);
}

#[inline]
fn format_shm_name(pid: u32, seq: u32, buf: &mut [u8; 32]) -> &[u8] {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let prefix = b"/ii-";
    buf[..4].copy_from_slice(prefix);
    for i in 0..8 {
        let nib = ((pid >> ((7 - i) * 4)) & 0xF) as usize;
        buf[4 + i] = HEX[nib];
    }
    buf[12] = b'-';
    for i in 0..8 {
        let nib = ((seq >> ((7 - i) * 4)) & 0xF) as usize;
        buf[13 + i] = HEX[nib];
    }
    &buf[..21]
}

/// Stage local POSIX SHM KGP transmission (`t=s`) + virtual placement (`a=p,U=1`) into `apc`.
pub fn stage_shm(
    rgba: &[u8],
    w: u16,
    h: u16,
    image_id: u32,
    cols: u16,
    rows: u16,
    is_tmux: bool,
    seq: &mut u32,
    pid: u32,
    apc: &mut Vec<u8>,
) -> Result<(), Error> {
    if rgba.is_empty() {
        return Err(Error::ShmExhausted);
    }
    let mut name_buf = [0u8; 32];
    let mut opened = None;

    for _ in 0..4 {
        *seq = seq.wrapping_add(1);
        let name = format_shm_name(pid, *seq, &mut name_buf);
        let name_str = core::str::from_utf8(name).unwrap_or("/ii-0");
        match shm_open(
            name_str,
            OFlags::CREATE | OFlags::EXCL | OFlags::RDWR,
            Mode::RUSR | Mode::WUSR,
        ) {
            Ok(fd) => {
                opened = Some((fd, name.len()));
                break;
            }
            Err(Errno::EXIST) => {}
            Err(e) => return Err(Error::Io(e)),
        }
    }

    let (fd, name_len) = opened.ok_or(Error::ShmExhausted)?;
    let size = rgba.len();
    ftruncate(&fd, size as u64)?;

    // SAFETY: `fd` is a newly created, exclusively owned POSIX shared memory segment truncated to `size` bytes.
    // We map it `PROT_READ | PROT_WRITE`, copy `rgba` into the mapping, and immediately `munmap` before returning.
    unsafe {
        let ptr = mmap(
            core::ptr::null_mut(),
            size,
            ProtFlags::READ | ProtFlags::WRITE,
            MapFlags::SHARED,
            &fd,
            0,
        )?;
        core::ptr::copy_nonoverlapping(rgba.as_ptr(), ptr.cast::<u8>(), size);
        munmap(ptr, size)?;
    }

    let mut b64_name = Vec::with_capacity(32);
    base64_encode(&name_buf[..name_len], &mut b64_name);

    begin_apc(is_tmux, apc);
    apc.extend_from_slice(b"a=t,t=s,f=32,q=2,i=");
    push_u32_dec(image_id, apc);
    apc.extend_from_slice(b",s=");
    push_u32_dec(u32::from(w), apc);
    apc.extend_from_slice(b",v=");
    push_u32_dec(u32::from(h), apc);
    apc.push(b';');
    apc.extend_from_slice(&b64_name);
    end_apc(is_tmux, apc);

    emit_virtual_placement(image_id, cols, rows, is_tmux, apc);
    Ok(())
}

/// Stage chunked Base64 KGP transmission (`t=d`, 4096 B chunks) + virtual placement (`a=p,U=1`) into `apc`.
pub fn stage_base64(
    rgba: &[u8],
    w: u16,
    h: u16,
    image_id: u32,
    cols: u16,
    rows: u16,
    is_tmux: bool,
    apc: &mut Vec<u8>,
    b64: &mut Vec<u8>,
) {
    base64_encode(rgba, b64);
    let total = b64.len();
    let chunk_size = 4096usize;
    let mut offset = 0usize;
    let mut first = true;

    while offset < total || first {
        let end = (offset + chunk_size).min(total);
        let more = u8::from(end < total);
        begin_apc(is_tmux, apc);
        if first {
            apc.extend_from_slice(b"a=t,t=d,f=32,q=2,i=");
            push_u32_dec(image_id, apc);
            apc.extend_from_slice(b",s=");
            push_u32_dec(u32::from(w), apc);
            apc.extend_from_slice(b",v=");
            push_u32_dec(u32::from(h), apc);
            apc.extend_from_slice(b",m=");
            apc.push(b'0' + more);
        } else {
            apc.extend_from_slice(b"m=");
            apc.push(b'0' + more);
        }
        if end > offset {
            apc.push(b';');
            apc.extend_from_slice(&b64[offset..end]);
        }
        end_apc(is_tmux, apc);
        offset = end;
        first = false;
    }

    emit_virtual_placement(image_id, cols, rows, is_tmux, apc);
}

/// Emit `U+10EEEE` virtual placement grid with 24-bit foreground `image_id` color and row/col diacritics.
pub fn emit_placeholder(image_id: u32, cols: u16, rows: u16, text: &mut Vec<u8>) {
    let r_ch = ((image_id >> 16) & 0xFF) as u8;
    let g_ch = ((image_id >> 8) & 0xFF) as u8;
    let b_ch = (image_id & 0xFF) as u8;

    for row in 0..rows {
        text.extend_from_slice(b"\x1b[38;2;");
        push_u32_dec(u32::from(r_ch), text);
        text.push(b';');
        push_u32_dec(u32::from(g_ch), text);
        text.push(b';');
        push_u32_dec(u32::from(b_ch), text);
        text.push(b'm');

        let r_diac = DIACRITICS_UTF8[(row as usize) & 63];
        for col in 0..cols {
            let c_diac = DIACRITICS_UTF8[(col as usize) & 63];
            text.extend_from_slice(&PLACEHOLDER_UTF8);
            text.extend_from_slice(&r_diac);
            text.extend_from_slice(&c_diac);
        }
        text.extend_from_slice(b"\x1b[39m\n");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_stages_and_emits_placeholder() {
        let rgba = [255u8; 64];
        let mut apc = Vec::new();
        let mut b64 = Vec::new();
        stage_base64(&rgba, 4, 4, 0x0012_3456, 2, 1, false, &mut apc, &mut b64);
        assert!(apc.starts_with(b"\x1b_Ga=t,t=d,f=32,q=2,i=1193046,s=4,v=4,m=0;"));
        assert!(apc.ends_with(b"\x1b\\"));

        let mut text = Vec::new();
        emit_placeholder(0x0012_3456, 2, 1, &mut text);
        assert!(text.starts_with(b"\x1b[38;2;18;52;86m"));
        assert!(text.ends_with(b"\x1b[39m\n"));
    }
}
