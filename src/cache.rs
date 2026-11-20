//! BLAKE3 L2 disk cache (`PX03` + raw LZ4 block), race-free `openat`/`renameat`, zero-alloc hits.

use core::{
    ffi::CStr,
    sync::atomic::{AtomicU32, Ordering},
};

use rustix::{
    fd::{BorrowedFd, OwnedFd},
    fs::{AtFlags, CWD, Mode, OFlags, fstat, mkdirat, openat, renameat, unlinkat},
    io::{Errno, read, write},
    process::getpid,
};

use crate::err::Error;

pub const MAGIC: [u8; 4] = *b"PX03";
pub const HEADER_LEN: usize = 12;
pub const MAX_DECODE_SIZE: usize = 67_108_864;

static TMP_SEQ: AtomicU32 = AtomicU32::new(0);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Header {
    pub magic: [u8; 4],
    pub width: u32,
    pub height: u32,
}
const _: () = assert!(size_of::<Header>() == HEADER_LEN);

impl Header {
    #[inline]
    #[must_use]
    pub const fn to_le_bytes(self) -> [u8; HEADER_LEN] {
        let [w0, w1, w2, w3] = self.width.to_le_bytes();
        let [h0, h1, h2, h3] = self.height.to_le_bytes();
        [
            self.magic[0],
            self.magic[1],
            self.magic[2],
            self.magic[3],
            w0,
            w1,
            w2,
            w3,
            h0,
            h1,
            h2,
            h3,
        ]
    }

    #[inline]
    #[must_use]
    pub const fn from_le_bytes(b: [u8; HEADER_LEN]) -> Self {
        Self {
            magic: [b[0], b[1], b[2], b[3]],
            width: u32::from_le_bytes([b[4], b[5], b[6], b[7]]),
            height: u32::from_le_bytes([b[8], b[9], b[10], b[11]]),
        }
    }
}

#[must_use]
pub fn blake3_key(
    cell_w: u16,
    cell_h: u16,
    max_w: u16,
    kind: u8,
    fg_rgb: [u8; 3],
    is_dark: bool,
    content: &[u8],
) -> [u8; 32] {
    let [cw0, cw1] = cell_w.to_le_bytes();
    let [ch0, ch1] = cell_h.to_le_bytes();
    let [mw0, mw1] = max_w.to_le_bytes();
    let meta = [
        cw0,
        cw1,
        ch0,
        ch1,
        mw0,
        mw1,
        kind,
        fg_rgb[0],
        fg_rgb[1],
        fg_rgb[2],
        u8::from(is_dark),
        3,
    ];
    let mut hasher = blake3::Hasher::new();
    hasher.update(&meta);
    hasher.update(content);
    *hasher.finalize().as_bytes()
}

#[inline]
pub const fn hex_encode_cstr(hash: &[u8; 32], out: &mut [u8; 65]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut i = 0;
    while i < 32 {
        out[i * 2] = HEX[(hash[i] >> 4) as usize];
        out[i * 2 + 1] = HEX[(hash[i] & 0x0F) as usize];
        i += 1;
    }
    out[64] = 0;
}

#[inline]
fn fmt_tmp_name(pid: u32, seq: u32, buf: &mut [u8; 24]) -> &CStr {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    buf[..5].copy_from_slice(b".tmp-");
    for i in 0..8 {
        buf[5 + i] = HEX[((pid >> ((7 - i) * 4)) & 0xF) as usize];
        buf[14 + i] = HEX[((seq >> ((7 - i) * 4)) & 0xF) as usize];
    }
    buf[13] = b'-';
    buf[22] = 0;
    CStr::from_bytes_with_nul(&buf[..=22]).unwrap_or(c".tmp")
}

#[inline]
fn checked_rgba_len(w: u32, h: u32) -> Result<usize, Error> {
    if w == 0 || h == 0 {
        return Err(Error::CacheCorrupt);
    }
    let len = (w as usize)
        .checked_mul(h as usize)
        .and_then(|px| px.checked_mul(4))
        .ok_or(Error::CacheCorrupt)?;
    if len > MAX_DECODE_SIZE {
        return Err(Error::CacheCorrupt);
    }
    Ok(len)
}

/// Read and decompress a `PX03` blob relative to `dir_fd` directly into `rgba`.
///
/// # Errors
/// Returns `Error::Io`, `Error::CacheCorrupt`, or `Error::Lz4Decompress`.
pub fn read_blob(
    dir_fd: BorrowedFd<'_>,
    key: &[u8; 32],
    rgba: &mut Vec<u8>,
    lz4: &mut Vec<u8>,
) -> Result<Option<(u32, u32)>, Error> {
    let mut name_buf = [0u8; 65];
    hex_encode_cstr(key, &mut name_buf);
    let name = CStr::from_bytes_with_nul(&name_buf).map_err(|_| Error::CacheCorrupt)?;

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

    let file_len = usize::try_from(fstat(&fd)?.st_size).map_err(|_| Error::CacheCorrupt)?;
    let max_len = HEADER_LEN + lz4_flex::block::get_maximum_output_size(MAX_DECODE_SIZE);
    if file_len < HEADER_LEN || file_len > max_len {
        return Err(Error::CacheCorrupt);
    }

    lz4.clear();
    lz4.resize(file_len, 0);
    let mut pos = 0usize;
    while pos < file_len {
        match read(&fd, &mut lz4[pos..]) {
            Ok(0) => break,
            Ok(n) => pos += n,
            Err(Errno::INTR) => {}
            Err(e) => return Err(Error::Io(e)),
        }
    }
    if pos < HEADER_LEN {
        return Err(Error::CacheCorrupt);
    }
    lz4.truncate(pos);

    let mut hdr_bytes = [0u8; HEADER_LEN];
    hdr_bytes.copy_from_slice(&lz4[..HEADER_LEN]);
    let hdr = Header::from_le_bytes(hdr_bytes);
    if hdr.magic != MAGIC {
        return Err(Error::CacheCorrupt);
    }
    let expected = checked_rgba_len(hdr.width, hdr.height)?;

    rgba.clear();
    rgba.resize(expected, 0);
    let decoded = lz4_flex::block::decompress_into(&lz4[HEADER_LEN..], rgba)
        .map_err(|_| Error::Lz4Decompress)?;
    if decoded != expected {
        return Err(Error::CacheCorrupt);
    }
    Ok(Some((hdr.width, hdr.height)))
}

/// Compress and atomically publish a `PX03` blob via `O_EXCL` + `renameat`.
///
/// # Errors
/// Returns `Error::CacheCorrupt`, `Error::Lz4Decompress`, or `Error::Io`.
pub fn write_blob(
    dir_fd: BorrowedFd<'_>,
    key: &[u8; 32],
    w: u32,
    h: u32,
    rgba: &[u8],
    lz4: &mut Vec<u8>,
) -> Result<(), Error> {
    if rgba.len() != checked_rgba_len(w, h)? {
        return Err(Error::CacheCorrupt);
    }
    let max_c = lz4_flex::block::get_maximum_output_size(rgba.len());
    lz4.clear();
    lz4.resize(HEADER_LEN + max_c, 0);
    lz4[..HEADER_LEN].copy_from_slice(
        &Header {
            magic: MAGIC,
            width: w,
            height: h,
        }
        .to_le_bytes(),
    );
    let written_lz4 = lz4_flex::block::compress_into(rgba, &mut lz4[HEADER_LEN..])
        .map_err(|_| Error::Lz4Decompress)?;
    lz4.truncate(HEADER_LEN + written_lz4);

    let mut hex_buf = [0u8; 65];
    hex_encode_cstr(key, &mut hex_buf);
    let final_name = CStr::from_bytes_with_nul(&hex_buf).map_err(|_| Error::CacheCorrupt)?;

    let pid = getpid().as_raw_nonzero().get().unsigned_abs();
    let seq = TMP_SEQ.fetch_add(1, Ordering::Relaxed);
    let mut tmp_buf = [0u8; 24];
    let tmp_name = fmt_tmp_name(pid, seq, &mut tmp_buf);

    let fd = openat(
        dir_fd,
        tmp_name,
        OFlags::CREATE | OFlags::EXCL | OFlags::WRONLY | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::RUSR | Mode::WUSR,
    )?;

    let mut off = 0usize;
    while off < lz4.len() {
        match write(&fd, &lz4[off..]) {
            Ok(0) => {
                let _ = unlinkat(dir_fd, tmp_name, AtFlags::empty());
                return Err(Error::Io(Errno::IO));
            }
            Ok(n) => off += n,
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

/// Ensure user-owned `0700` cache directory exists and return its `O_DIRECTORY` fd.
///
/// # Errors
/// Returns `Error::Io` on filesystem or ownership failure.
pub fn ensure_dirs() -> Result<OwnedFd, Error> {
    let uid = rustix::process::getuid();
    let path = std::env::var_os("XDG_CACHE_HOME").map_or_else(
        || {
            std::env::var_os("HOME").map_or_else(
                || std::path::PathBuf::from(format!("/tmp/ii-cache-{}", uid.as_raw())),
                |home| {
                    let mut p = std::path::PathBuf::from(home);
                    p.push(".cache");
                    let _ = mkdirat(CWD, p.as_path(), Mode::RWXU);
                    p.push("ii");
                    p
                },
            )
        },
        |xdg| std::path::PathBuf::from(xdg).join("ii"),
    );

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
    if fstat(&dir_fd)?.st_uid != uid.as_raw() {
        return Err(Error::Io(Errno::ACCESS));
    }
    Ok(dir_fd)
}
