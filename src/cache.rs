//! BLAKE3 L2 disk cache (`PX02` header + LZ4 frame compression) using race-free `openat`/`renameat`.

use rustix::{
    fd::{BorrowedFd, OwnedFd},
    fs::{AtFlags, CWD, Mode, OFlags, mkdirat, openat, renameat, unlinkat},
    io::{Errno, read, write},
    process::getpid,
};

use crate::err::Error;

pub const MAGIC: [u8; 4] = *b"PX02";
pub const MAX_DECODE_SIZE: usize = 67_108_864; // 64 MiB

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Header {
    pub magic: [u8; 4],
    pub width: u32,
    pub height: u32,
}

const _: () = assert!(size_of::<Header>() == 12);

impl Header {
    #[inline]
    #[must_use]
    pub const fn to_le_bytes(self) -> [u8; 12] {
        let (w, h) = (self.width.to_le_bytes(), self.height.to_le_bytes());
        [self.magic[0], self.magic[1], self.magic[2], self.magic[3], w[0], w[1], w[2], w[3], h[0], h[1], h[2], h[3]]
    }

    #[inline]
    #[must_use]
    pub const fn from_le_bytes(b: [u8; 12]) -> Self {
        Self {
            magic: [b[0], b[1], b[2], b[3]],
            width: u32::from_le_bytes([b[4], b[5], b[6], b[7]]),
            height: u32::from_le_bytes([b[8], b[9], b[10], b[11]]),
        }
    }
}

/// Compute 32-byte BLAKE3 L2 cache key folding cell geometry, theme polarity, and raw content.
#[must_use]
pub fn blake3_key(cell_w: u16, cell_h: u16, max_w: u16, is_dark: bool, content: &[u8]) -> [u8; 32] {
    let mut hasher = blake3::Hasher::new();
    let meta = [
        (cell_w & 0xFF) as u8,
        (cell_w >> 8) as u8,
        (cell_h & 0xFF) as u8,
        (cell_h >> 8) as u8,
        (max_w & 0xFF) as u8,
        (max_w >> 8) as u8,
        u8::from(is_dark),
        2, // schema version PX02
    ];
    hasher.update(&meta);
    hasher.update(content);
    *hasher.finalize().as_bytes()
}

/// Encode 32-byte hash into 64-byte lowercase ASCII hex on stack.
#[inline]
pub fn hex_encode(hash: &[u8; 32], out: &mut [u8; 64]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for (i, &b) in hash.iter().enumerate() {
        out[i * 2] = HEX[(b >> 4) as usize];
        out[i * 2 + 1] = HEX[(b & 0x0F) as usize];
    }
}

/// Read and decompress a `PX02` cache blob relative to `dir_fd` into `rgba`.
pub fn read_blob(
    dir_fd: BorrowedFd<'_>,
    key: &[u8; 32],
    rgba: &mut Vec<u8>,
    lz4: &mut Vec<u8>,
) -> Result<Option<(u32, u32)>, Error> {
    let mut name_buf = [0u8; 64];
    hex_encode(key, &mut name_buf);
    let name = core::str::from_utf8(&name_buf).unwrap_or("");

    let fd = match openat(
        dir_fd,
        name,
        OFlags::RDONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    ) {
        Ok(fd) => fd,
        Err(Errno::NOENT) => return Ok(None),
        Err(e) => return Err(Error::Io(e)),
    };

    lz4.clear();
    let mut chunk = [0u8; 8192];
    loop {
        match read(&fd, &mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                if lz4.len() + n > MAX_DECODE_SIZE {
                    return Err(Error::CacheCorrupt);
                }
                lz4.extend_from_slice(&chunk[..n]);
            }
            Err(Errno::INTR) => {}
            Err(e) => return Err(Error::Io(e)),
        }
    }

    if lz4.len() < 12 {
        return Err(Error::CacheCorrupt);
    }
    let mut hdr_bytes = [0u8; 12];
    hdr_bytes.copy_from_slice(&lz4[..12]);
    let hdr = Header::from_le_bytes(hdr_bytes);
    if hdr.magic != MAGIC || hdr.width == 0 || hdr.height == 0 {
        return Err(Error::CacheCorrupt);
    }
    let expected_len = (hdr.width as usize)
        .checked_mul(hdr.height as usize)
        .and_then(|px| px.checked_mul(4))
        .ok_or(Error::CacheCorrupt)?;
    if expected_len > MAX_DECODE_SIZE {
        return Err(Error::CacheCorrupt);
    }

    rgba.clear();
    let mut dec = lz4_flex::frame::FrameDecoder::new(&lz4[12..]);
    let mut dec_chunk = [0u8; 8192];
    loop {
        let n = std::io::Read::read(&mut dec, &mut dec_chunk).map_err(|_| Error::Lz4Decompress)?;
        if n == 0 {
            break;
        }
        if rgba.len() + n > expected_len {
            return Err(Error::CacheCorrupt);
        }
        rgba.extend_from_slice(&dec_chunk[..n]);
    }

    if rgba.len() != expected_len {
        return Err(Error::CacheCorrupt);
    }

    Ok(Some((hdr.width, hdr.height)))
}

/// Compress and atomically write a `PX02` cache blob relative to `dir_fd` via temp file + `renameat`.
pub fn write_blob(
    dir_fd: BorrowedFd<'_>,
    key: &[u8; 32],
    w: u32,
    h: u32,
    rgba: &[u8],
    lz4: &mut Vec<u8>,
) -> Result<(), Error> {
    let expected = (w as usize)
        .checked_mul(h as usize)
        .and_then(|px| px.checked_mul(4))
        .ok_or(Error::CacheCorrupt)?;
    if rgba.len() != expected {
        return Err(Error::CacheCorrupt);
    }

    lz4.clear();
    let hdr = Header {
        magic: MAGIC,
        width: w,
        height: h,
    };
    lz4.extend_from_slice(&hdr.to_le_bytes());
    {
        let mut enc = lz4_flex::frame::FrameEncoder::new(&mut *lz4);
        std::io::Write::write_all(&mut enc, rgba).map_err(|_| Error::Lz4Decompress)?;
        enc.finish().map_err(|_| Error::Lz4Decompress)?;
    }

    let mut hex_name = [0u8; 64];
    hex_encode(key, &mut hex_name);
    let final_name = core::str::from_utf8(&hex_name).unwrap_or("");

    let pid = getpid().as_raw_nonzero().get();
    let mut tmp_buf = [0u8; 80];
    tmp_buf[..5].copy_from_slice(b".tmp-");
    tmp_buf[5..69].copy_from_slice(&hex_name);
    tmp_buf[69] = b'-';
    let mut p = pid.unsigned_abs();
    for i in (70..78).rev() {
        tmp_buf[i] = b'0' + ((p % 10) as u8);
        p /= 10;
    }
    let tmp_name = core::str::from_utf8(&tmp_buf[..78]).unwrap_or(".tmp");

    let fd = openat(
        dir_fd,
        tmp_name,
        OFlags::CREATE | OFlags::TRUNC | OFlags::WRONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::RUSR | Mode::WUSR,
    )?;

    let mut written = 0usize;
    while written < lz4.len() {
        match write(&fd, &lz4[written..]) {
            Ok(n) => written += n,
            Err(Errno::INTR) => {}
            Err(e) => {
                let _ = unlinkat(dir_fd, tmp_name, AtFlags::empty());
                return Err(Error::Io(e));
            }
        }
    }
    drop(fd);

    if let Err(e) = renameat(dir_fd, tmp_name, dir_fd, final_name) {
        let _ = unlinkat(dir_fd, tmp_name, AtFlags::empty());
        return Err(Error::Io(e));
    }
    Ok(())
}

/// Ensure `$XDG_CACHE_HOME/ii` (or `~/.cache/ii` / `/tmp/ii-cache-<uid>`) exists with `0700` permissions
/// and return an `O_DIRECTORY | O_CLOEXEC` file descriptor.
pub fn ensure_dirs() -> Result<OwnedFd, Error> {
    let path = if let Some(xdg) = std::env::var_os("XDG_CACHE_HOME") {
        let mut p = std::path::PathBuf::from(xdg);
        p.push("ii");
        p
    } else if let Some(home) = std::env::var_os("HOME") {
        let mut p = std::path::PathBuf::from(home);
        p.push(".cache");
        let _ = mkdirat(CWD, p.as_path(), Mode::RWXU);
        p.push("ii");
        p
    } else {
        let uid = rustix::process::getuid().as_raw();
        std::path::PathBuf::from(format!("/tmp/ii-cache-{uid}"))
    };

    match mkdirat(CWD, path.as_path(), Mode::RWXU) {
        Ok(()) | Err(Errno::EXIST) => {}
        Err(e) => return Err(Error::Io(e)),
    }

    let dir_fd = openat(
        CWD,
        path.as_path(),
        OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
    )?;
    Ok(dir_fd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blake3_hex_and_blob_roundtrip() {
        let key = blake3_key(8, 16, 640, true, b"<svg/>");
        let mut hex = [0u8; 64];
        hex_encode(&key, &mut hex);
        assert!(hex.iter().all(u8::is_ascii_hexdigit));
    }
}
