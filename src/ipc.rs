//! IXPC v1 binary wire protocol: 32-byte Header, 16-byte Extmark, command codec,
//! and zero-allocation client transport. Decoupled from server poll reactor.

use std::path::PathBuf;

use rustix::{
    fd::{AsFd, BorrowedFd},
    io::{Errno, FdFlags, fcntl_setfd, read, write},
    net::{AddressFamily, SocketAddrUnix, SocketType, connect, socket},
    process::getuid,
};

use crate::err::Error;

pub const MAGIC: u32 = 0x4958_5043; // "IXPC"
pub const VERSION: u16 = 1;
pub const MAX_PAYLOAD: u32 = 0x0400_0000; // 64 MiB
pub const RESP_FLAG: u16 = 0x8000;
pub const ERR_FLAG: u16 = 0x4000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum Cmd {
    Ping = 0x0001,
    ComposeAnsi = 0x0002,
    ComposeExtmarks = 0x0003,
    RenderFile = 0x0004,
    Shutdown = 0x0006,
}

impl Cmd {
    #[inline]
    pub const fn from_u16(v: u16) -> Result<Self, Error> {
        match v & 0x3FFF {
            0x0001 => Ok(Self::Ping),
            0x0002 => Ok(Self::ComposeAnsi),
            0x0003 => Ok(Self::ComposeExtmarks),
            0x0004 => Ok(Self::RenderFile),
            0x0006 => Ok(Self::Shutdown),
            other => Err(Error::UnknownCommand(other)),
        }
    }
}

/// 32-byte packed binary header.
/// Descending alignment: 8 (reserved) -> 4 (magic, flags, payload_len) -> 2 (version, command, cols, rows, cell_w, cell_h).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Header {
    pub magic: u32,
    pub version: u16,
    pub command: u16,
    pub flags: u32,
    pub cols: u16,
    pub rows: u16,
    pub cell_w: u16,
    pub cell_h: u16,
    pub payload_len: u32,
    pub reserved: u64,
}

const _: () = assert!(size_of::<Header>() == 32);
const _: () = assert!(align_of::<Header>() == 8);

impl Header {
    #[inline]
    #[must_use]
    pub const fn new(cmd: Cmd, payload_len: u32) -> Self {
        Self {
            magic: MAGIC,
            version: VERSION,
            command: cmd as u16,
            flags: 0,
            cols: 80,
            rows: 24,
            cell_w: 8,
            cell_h: 16,
            payload_len,
            reserved: 0,
        }
    }

    #[inline]
    #[must_use]
    pub const fn from_le_bytes(b: &[u8; 32]) -> Self {
        Self {
            magic: u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            version: u16::from_le_bytes([b[4], b[5]]),
            command: u16::from_le_bytes([b[6], b[7]]),
            flags: u32::from_le_bytes([b[8], b[9], b[10], b[11]]),
            cols: u16::from_le_bytes([b[12], b[13]]),
            rows: u16::from_le_bytes([b[14], b[15]]),
            cell_w: u16::from_le_bytes([b[16], b[17]]),
            cell_h: u16::from_le_bytes([b[18], b[19]]),
            payload_len: u32::from_le_bytes([b[20], b[21], b[22], b[23]]),
            reserved: u64::from_le_bytes([b[24], b[25], b[26], b[27], b[28], b[29], b[30], b[31]]),
        }
    }

    #[inline]
    #[must_use]
    pub const fn to_le_bytes(&self) -> [u8; 32] {
        let m = self.magic.to_le_bytes();
        let v = self.version.to_le_bytes();
        let c = self.command.to_le_bytes();
        let f = self.flags.to_le_bytes();
        let co = self.cols.to_le_bytes();
        let ro = self.rows.to_le_bytes();
        let cw = self.cell_w.to_le_bytes();
        let ch = self.cell_h.to_le_bytes();
        let pl = self.payload_len.to_le_bytes();
        let rs = self.reserved.to_le_bytes();
        [
            m[0], m[1], m[2], m[3], v[0], v[1], c[0], c[1], f[0], f[1], f[2], f[3], co[0], co[1],
            ro[0], ro[1], cw[0], cw[1], ch[0], ch[1], pl[0], pl[1], pl[2], pl[3], rs[0], rs[1],
            rs[2], rs[3], rs[4], rs[5], rs[6], rs[7],
        ]
    }

    #[inline]
    pub const fn validate(&self) -> Result<Cmd, Error> {
        if self.magic != MAGIC {
            return Err(Error::BadMagic);
        }
        if self.version != VERSION {
            return Err(Error::BadVersion(self.version));
        }
        if self.payload_len > MAX_PAYLOAD {
            return Err(Error::PayloadTooLarge(self.payload_len));
        }
        Cmd::from_u16(self.command)
    }
}

/// 16-byte Neovim extmark bounding box and image placement.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Extmark {
    pub image_id: u32,
    pub start_row: u16,
    pub start_col: u16,
    pub end_row: u16,
    pub end_col: u16,
    pub cols: u16,
    pub rows: u16,
}

const _: () = assert!(size_of::<Extmark>() == 16);
const _: () = assert!(align_of::<Extmark>() == 4);

impl Extmark {
    #[inline]
    #[must_use]
    pub const fn to_le_bytes(self) -> [u8; 16] {
        let id = self.image_id.to_le_bytes();
        let sr = self.start_row.to_le_bytes();
        let sc = self.start_col.to_le_bytes();
        let er = self.end_row.to_le_bytes();
        let ec = self.end_col.to_le_bytes();
        let co = self.cols.to_le_bytes();
        let ro = self.rows.to_le_bytes();
        [
            id[0], id[1], id[2], id[3], sr[0], sr[1], sc[0], sc[1], er[0], er[1], ec[0], ec[1],
            co[0], co[1], ro[0], ro[1],
        ]
    }

    #[inline]
    #[must_use]
    pub const fn from_le_bytes(b: &[u8; 16]) -> Self {
        Self {
            image_id: u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            start_row: u16::from_le_bytes([b[4], b[5]]),
            start_col: u16::from_le_bytes([b[6], b[7]]),
            end_row: u16::from_le_bytes([b[8], b[9]]),
            end_col: u16::from_le_bytes([b[10], b[11]]),
            cols: u16::from_le_bytes([b[12], b[13]]),
            rows: u16::from_le_bytes([b[14], b[15]]),
        }
    }
}

/// Encode dual-channel response (`[u32 apc_len][apc][text]`) prefixed by a 32-byte header.
pub fn encode_dual_response(cmd: Cmd, apc: &[u8], text: &[u8], out: &mut Vec<u8>) {
    let payload_len = (4 + apc.len() + text.len()) as u32;
    let mut hdr = Header::new(cmd, payload_len);
    hdr.command = (cmd as u16) | RESP_FLAG;
    out.clear();
    out.reserve(32 + payload_len as usize);
    out.extend_from_slice(&hdr.to_le_bytes());
    out.extend_from_slice(&(apc.len() as u32).to_le_bytes());
    out.extend_from_slice(apc);
    out.extend_from_slice(text);
}

/// Encode error frame into `out`.
pub fn encode_error(cmd: Cmd, msg: &str, out: &mut Vec<u8>) {
    let bytes = msg.as_bytes();
    let mut hdr = Header::new(cmd, bytes.len() as u32);
    hdr.command = (cmd as u16) | RESP_FLAG | ERR_FLAG;
    out.clear();
    out.reserve(32 + bytes.len());
    out.extend_from_slice(&hdr.to_le_bytes());
    out.extend_from_slice(bytes);
}

/// Span coordinates for extmark layout calculation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpanCoord {
    pub outer_start: u32,
    pub outer_end: u32,
    pub image_id: u32,
    pub cols: u16,
    pub rows: u16,
}

/// Compute Neovim extmarks using SIMD newline scanning and accurate column offsets.
pub fn compose_extmarks(src: &[u8], spans: &[SpanCoord], out: &mut Vec<Extmark>) {
    out.clear();
    out.reserve(spans.len());

    let mut line_starts = Vec::with_capacity(64);
    line_starts.push(0usize);
    for nl in memchr::memchr_iter(b'\n', src) {
        line_starts.push(nl + 1);
    }

    for sp in spans {
        let start = (sp.outer_start as usize).min(src.len());
        let end = (sp.outer_end as usize).min(src.len());

        let row_start_idx = match line_starts.binary_search(&start) {
            Ok(idx) => idx,
            Err(idx) => idx.saturating_sub(1),
        };
        let line_start_byte = line_starts[row_start_idx];
        let start_col = (start.saturating_sub(line_start_byte)) as u16;

        let row_end_idx = match line_starts.binary_search(&end) {
            Ok(idx) => idx,
            Err(idx) => idx.saturating_sub(1),
        };
        let end_line_byte = line_starts[row_end_idx];
        let end_col = (end.saturating_sub(end_line_byte)) as u16;

        out.push(Extmark {
            image_id: sp.image_id,
            start_row: row_start_idx as u16,
            start_col,
            end_row: row_end_idx as u16,
            end_col,
            cols: sp.cols,
            rows: sp.rows,
        });
    }
}

/// Encode `[Extmark]` records directly into a little-endian byte payload.
pub fn encode_extmarks(records: &[Extmark], out: &mut Vec<u8>) {
    out.reserve(records.len() * 16);
    for rec in records {
        out.extend_from_slice(&rec.to_le_bytes());
    }
}

/// Path to UNIX domain socket in /run/user/$UID/ii or /tmp/ii-$UID.
#[must_use]
pub fn socket_path() -> PathBuf {
    let mut p = if let Some(xdg) = std::env::var_os("XDG_RUNTIME_DIR") {
        let mut d = PathBuf::from(xdg);
        d.push("ii");
        d
    } else {
        PathBuf::from(format!("/tmp/ii-{}", getuid().as_raw()))
    };
    p.push("ii.sock");
    p
}

/// Path to daemon lockfile.
#[must_use]
pub fn lock_path() -> PathBuf {
    let mut p = socket_path();
    p.set_file_name("ii.lock");
    p
}

/// Zero-dep synchronous client transmission.
pub fn client_send(cmd: Cmd, payload: &[u8]) -> Result<Vec<u8>, Error> {
    let sock = socket_path();
    let fd = socket(AddressFamily::UNIX, SocketType::STREAM, None)?;
    fcntl_setfd(&fd, FdFlags::CLOEXEC)?;
    let addr = SocketAddrUnix::new(sock.as_path()).map_err(|_| Error::SocketSymlink)?;
    connect(&fd, &addr)?;

    let hdr = Header::new(cmd, payload.len() as u32);
    let mut out = Vec::with_capacity(32 + payload.len());
    out.extend_from_slice(&hdr.to_le_bytes());
    out.extend_from_slice(payload);

    let mut to_write = &out[..];
    while !to_write.is_empty() {
        match write(&fd, to_write) {
            Ok(n) => to_write = &to_write[n..],
            Err(Errno::INTR) => {}
            Err(e) => return Err(Error::Io(e)),
        }
    }

    let mut resp_hdr = [0u8; 32];
    read_exact(fd.as_fd(), &mut resp_hdr)?;
    let parsed = Header::from_le_bytes(&resp_hdr);
    let mut resp_body = vec![0u8; parsed.payload_len as usize];
    read_exact(fd.as_fd(), &mut resp_body)?;
    Ok(resp_body)
}

fn read_exact(fd: BorrowedFd<'_>, mut buf: &mut [u8]) -> Result<(), Error> {
    while !buf.is_empty() {
        match read(fd, buf) {
            Ok(0) => return Err(Error::BadMagic),
            Ok(n) => buf = &mut buf[n..],
            Err(Errno::INTR) => {}
            Err(e) => return Err(Error::Io(e)),
        }
    }
    Ok(())
}
