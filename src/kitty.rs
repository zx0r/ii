//! Kitty Graphics Protocol (KGP): POSIX SHM fast-path, Base64 fallback, delete, and `U+10EEEE` grid.

use core::{ffi::CStr, ptr::NonNull};

use rustix::{
    fd::OwnedFd,
    fs::{Mode, ftruncate},
    io::Errno,
    mm::{MapFlags, ProtFlags, mmap, munmap},
    shm::{self, OFlags},
};

use crate::err::Error;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct StageParams {
    pub image_id: u32,
    pub w: u16,
    pub h: u16,
    pub cols: u16,
    pub rows: u16,
    pub is_tmux: bool,
}

/// First 64 canonical combining-class-230 row/column diacritics (UTF-8) for KGP `U=1` placeholders.
#[rustfmt::skip]
const DIACRITICS_UTF8: [[u8; 2]; 64] = [
    [0xCC,0x85],[0xCC,0x8D],[0xCC,0x8E],[0xCC,0x90],[0xCC,0x92],[0xCC,0xBD],[0xCC,0xBE],[0xCC,0xBF],
    [0xCD,0x86],[0xCD,0x8A],[0xCD,0x8B],[0xCD,0x8C],[0xCD,0x90],[0xCD,0x91],[0xCD,0x92],[0xCD,0x97],
    [0xCD,0x9B],[0xCD,0xA3],[0xCD,0xA4],[0xCD,0xA5],[0xCD,0xA6],[0xCD,0xA7],[0xCD,0xA8],[0xCD,0xA9],
    [0xCD,0xAA],[0xCD,0xAB],[0xCD,0xAC],[0xCD,0xAD],[0xCD,0xAE],[0xCD,0xAF],[0xD2,0x83],[0xD2,0x84],
    [0xD2,0x85],[0xD2,0x86],[0xD6,0x92],[0xD6,0x93],[0xD6,0x94],[0xD6,0x95],[0xD6,0x97],[0xD6,0x98],
    [0xD6,0x99],[0xD6,0x9C],[0xD6,0x9D],[0xD6,0x9E],[0xD6,0x9F],[0xD6,0xA0],[0xD6,0xA1],[0xD6,0xA8],
    [0xD6,0xA9],[0xD6,0xAB],[0xD6,0xAC],[0xD6,0xAF],[0xD7,0x84],[0xD8,0x90],[0xD8,0x91],[0xD8,0x92],
    [0xD8,0x93],[0xD8,0x94],[0xD8,0x95],[0xD8,0x96],[0xD8,0x97],[0xD9,0x97],[0xD9,0x98],[0xD9,0x99],
];

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const PLACEHOLDER_UTF8: [u8; 4] = [0xF4, 0x8E, 0xBB, 0xAE];
const SHM_NAME_LEN: usize = 21;

struct ShmGuard<'a> {
    name: &'a CStr,
    armed: bool,
}

impl Drop for ShmGuard<'_> {
    fn drop(&mut self) {
        if self.armed {
            let _ = shm::unlink(self.name);
        }
    }
}

struct ShmMap {
    ptr: NonNull<u8>,
    len: usize,
}

impl ShmMap {
    fn new(fd: &OwnedFd, len: usize) -> Result<Self, Error> {
        // SAFETY: `fd` is an exclusively created POSIX SHM descriptor truncated to `len` bytes.
        let raw = unsafe {
            mmap(
                core::ptr::null_mut(),
                len,
                ProtFlags::READ | ProtFlags::WRITE,
                MapFlags::SHARED,
                fd,
                0,
            )?
        };
        let ptr = NonNull::new(raw.cast::<u8>()).ok_or(Error::Io(Errno::NOMEM))?;
        Ok(Self { ptr, len })
    }

    #[inline]
    fn write_all(&mut self, src: &[u8]) {
        debug_assert_eq!(src.len(), self.len);
        // SAFETY: `self.ptr` is writable for `self.len` bytes and disjoint from `src`.
        unsafe { core::ptr::copy_nonoverlapping(src.as_ptr(), self.ptr.as_ptr(), self.len) };
    }
}

impl Drop for ShmMap {
    fn drop(&mut self) {
        // SAFETY: `self.ptr` and `self.len` come from `mmap` and are unmapped exactly once.
        let _ = unsafe { munmap(self.ptr.as_ptr().cast(), self.len) };
    }
}

#[inline]
const fn write_u8_dec(v: u8, dst: &mut [u8]) -> usize {
    let mut i = 0;
    if v >= 100 {
        dst[i] = b'0' + v / 100;
        i += 1;
        dst[i] = b'0' + (v / 10) % 10;
        i += 1;
    } else if v >= 10 {
        dst[i] = b'0' + v / 10;
        i += 1;
    }
    dst[i] = b'0' + v % 10;
    i + 1
}

#[inline]
fn encode_base64_into(src: &[u8], dst: &mut [u8]) -> usize {
    let (chunks, rem) = src.as_chunks::<3>();
    let mut off = 0usize;
    for c in chunks {
        let n = (u32::from(c[0]) << 16) | (u32::from(c[1]) << 8) | u32::from(c[2]);
        dst[off..off + 4].copy_from_slice(&[
            B64[((n >> 18) & 63) as usize],
            B64[((n >> 12) & 63) as usize],
            B64[((n >> 6) & 63) as usize],
            B64[(n & 63) as usize],
        ]);
        off += 4;
    }
    match rem {
        [a] => {
            let n = u32::from(*a) << 16;
            dst[off..off + 4].copy_from_slice(&[
                B64[((n >> 18) & 63) as usize],
                B64[((n >> 12) & 63) as usize],
                b'=',
                b'=',
            ]);
            off += 4;
        }
        [a, b] => {
            let n = (u32::from(*a) << 16) | (u32::from(*b) << 8);
            dst[off..off + 4].copy_from_slice(&[
                B64[((n >> 18) & 63) as usize],
                B64[((n >> 12) & 63) as usize],
                B64[((n >> 6) & 63) as usize],
                b'=',
            ]);
            off += 4;
        }
        _ => {}
    }
    off
}

#[inline]
fn begin_apc(is_tmux: bool, apc: &mut Vec<u8>) {
    apc.extend_from_slice(if is_tmux {
        b"\x1bPtmux;\x1b\x1b_G"
    } else {
        b"\x1b_G"
    });
}

#[inline]
fn end_apc(is_tmux: bool, apc: &mut Vec<u8>) {
    apc.extend_from_slice(if is_tmux {
        b"\x1b\x1b\\\x1b\\"
    } else {
        b"\x1b\\"
    });
}

#[inline]
fn emit_virtual_placement(p: StageParams, apc: &mut Vec<u8>) {
    begin_apc(p.is_tmux, apc);
    apc.extend_from_slice(b"a=p,U=1,q=2,i=");
    crate::fmt::push_u32(apc, p.image_id);
    apc.extend_from_slice(b",c=");
    crate::fmt::push_u32(apc, u32::from(p.cols));
    apc.extend_from_slice(b",r=");
    crate::fmt::push_u32(apc, u32::from(p.rows));
    end_apc(p.is_tmux, apc);
}

#[inline]
pub fn emit_delete(image_id: u32, is_tmux: bool, apc: &mut Vec<u8>) {
    begin_apc(is_tmux, apc);
    apc.extend_from_slice(b"a=d,d=I,q=2,i=");
    crate::fmt::push_u32(apc, image_id);
    end_apc(is_tmux, apc);
}

#[inline]
fn format_shm_name(pid: u32, seq: u32, buf: &mut [u8; SHM_NAME_LEN + 1]) -> &CStr {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    buf[..4].copy_from_slice(b"/ii-");
    for i in 0..8 {
        buf[4 + i] = HEX[((pid >> ((7 - i) * 4)) & 0xF) as usize];
        buf[13 + i] = HEX[((seq >> ((7 - i) * 4)) & 0xF) as usize];
    }
    buf[12] = b'-';
    buf[SHM_NAME_LEN] = 0;
    CStr::from_bytes_with_nul(buf).unwrap_or(c"/ii-0")
}

/// Stage POSIX SHM KGP transmission (`t=s`, explicit `S=<bytes>` for macOS 16 `KiB` page rounding)
/// plus virtual placement (`a=p,U=1`).
///
/// # Errors
/// Returns `Error::ShmExhausted` or `Error::Io`.
pub fn stage_shm(
    rgba: &[u8],
    p: StageParams,
    seq: &mut u32,
    pid: u32,
    apc: &mut Vec<u8>,
) -> Result<(), Error> {
    if rgba.is_empty() {
        return Err(Error::ShmExhausted);
    }
    let byte_len = u64::try_from(rgba.len()).map_err(|_| Error::ShmExhausted)?;
    let mut name_buf = [0u8; SHM_NAME_LEN + 1];
    let mut opened = None;
    for _ in 0..4 {
        *seq = seq.wrapping_add(1);
        let name = format_shm_name(pid, *seq, &mut name_buf);
        match shm::open(
            name,
            OFlags::CREATE | OFlags::EXCL | OFlags::RDWR,
            Mode::RUSR | Mode::WUSR,
        ) {
            Ok(fd) => {
                opened = Some(fd);
                break;
            }
            Err(Errno::EXIST) => {}
            Err(e) => return Err(Error::Io(e)),
        }
    }

    let fd = opened.ok_or(Error::ShmExhausted)?;
    let name_cstr = CStr::from_bytes_with_nul(&name_buf).map_err(|_| Error::ShmExhausted)?;
    let mut guard = ShmGuard {
        name: name_cstr,
        armed: true,
    };

    ftruncate(&fd, byte_len)?;
    {
        let mut map = ShmMap::new(&fd, rgba.len())?;
        map.write_all(rgba);
    }
    drop(fd);

    let mut b64_name = [0u8; 28];
    let b64_len = encode_base64_into(&name_buf[..SHM_NAME_LEN], &mut b64_name);

    begin_apc(p.is_tmux, apc);
    apc.extend_from_slice(b"a=t,t=s,f=32,q=2,i=");
    crate::fmt::push_u32(apc, p.image_id);
    apc.extend_from_slice(b",s=");
    crate::fmt::push_u32(apc, u32::from(p.w));
    apc.extend_from_slice(b",v=");
    crate::fmt::push_u32(apc, u32::from(p.h));
    apc.extend_from_slice(b",S=");
    crate::fmt::push_u64(apc, byte_len);
    apc.push(b';');
    apc.extend_from_slice(&b64_name[..b64_len]);
    end_apc(p.is_tmux, apc);

    emit_virtual_placement(p, apc);
    guard.armed = false;
    Ok(())
}

/// Stage chunked Base64 KGP transmission (`t=d`, 4 096 B chunks) + virtual placement.
pub fn stage_base64(rgba: &[u8], p: StageParams, apc: &mut Vec<u8>, b64: &mut Vec<u8>) {
    if rgba.is_empty() {
        return;
    }
    let out_len = rgba.len().div_ceil(3) * 4;
    b64.clear();
    b64.resize(out_len, 0);
    let total = encode_base64_into(rgba, b64);

    let mut off = 0usize;
    let mut first = true;
    while off < total || first {
        let end = (off + 4096).min(total);
        begin_apc(p.is_tmux, apc);
        if first {
            apc.extend_from_slice(b"a=t,t=d,f=32,q=2,i=");
            crate::fmt::push_u32(apc, p.image_id);
            apc.extend_from_slice(b",s=");
            crate::fmt::push_u32(apc, u32::from(p.w));
            apc.extend_from_slice(b",v=");
            crate::fmt::push_u32(apc, u32::from(p.h));
            apc.extend_from_slice(b",m=");
        } else {
            apc.extend_from_slice(b"m=");
        }
        apc.push(b'0' + u8::from(end < total));
        if end > off {
            apc.push(b';');
            apc.extend_from_slice(&b64[off..end]);
        }
        end_apc(p.is_tmux, apc);
        off = end;
        first = false;
    }
    emit_virtual_placement(p, apc);
}

/// Emit `U+10EEEE` virtual placement grid (explicit row+col0 diacritic, auto-incremented columns).
pub fn emit_placeholder(image_id: u32, cols: u16, rows: u16, text: &mut Vec<u8>) {
    if cols == 0 || rows == 0 {
        return;
    }
    let [_, r, g, b] = image_id.to_be_bytes();
    let mut sgr = [0u8; 20];
    sgr[..7].copy_from_slice(b"\x1b[38;2;");
    let mut pos = 7;
    pos += write_u8_dec(r, &mut sgr[pos..]);
    sgr[pos] = b';';
    pos += 1;
    pos += write_u8_dec(g, &mut sgr[pos..]);
    sgr[pos] = b';';
    pos += 1;
    pos += write_u8_dec(b, &mut sgr[pos..]);
    sgr[pos] = b'm';
    let sgr_slice = &sgr[..=pos];

    let max_idx = DIACRITICS_UTF8.len() - 1;
    let first_col = DIACRITICS_UTF8[0];
    text.reserve(usize::from(rows).saturating_mul(sgr_slice.len() + usize::from(cols) * 4 + 10));
    for row in 0..rows {
        text.extend_from_slice(sgr_slice);
        let r_diac = DIACRITICS_UTF8[usize::from(row).min(max_idx)];
        text.extend_from_slice(&PLACEHOLDER_UTF8);
        text.extend_from_slice(&r_diac);
        text.extend_from_slice(&first_col);
        for _ in 1..cols {
            text.extend_from_slice(&PLACEHOLDER_UTF8);
        }
        text.extend_from_slice(b"\x1b[39m\n");
    }
}
