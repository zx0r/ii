//! IXPC v1 binary wire protocol and zero-copy `writev` client transport.

use std::{io::IoSlice, path::PathBuf};

use rustix::{
    fd::{AsFd, BorrowedFd},
    io::{Errno, FdFlags, fcntl_setfd, read, writev},
    net::{AddressFamily, SocketAddrUnix, SocketType, connect, socket},
    process::getuid,
};

use crate::err::Error;

pub const MAGIC: u32 = 0x4958_5043;
pub const VERSION: u16 = 1;
pub const MAX_PAYLOAD: u32 = 0x0400_0000;
pub const MAX_CLIENTS: usize = 32;
pub const RESP_FLAG: u16 = 0x8000;
pub const ERR_FLAG: u16 = 0x4000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum Cmd {
    Ping = 0x0001,
    ComposeAnsi = 0x0002,
    ComposeExtmarks = 0x0003,
    RenderFile = 0x0004,
    PreviewFile = 0x0005,
    Shutdown = 0x0006,
}

impl Cmd {
    /// Decode raw command code (ignoring response/error flags).
    ///
    /// # Errors
    /// Returns `Error::UnknownCommand` for unknown codes.
    #[inline]
    pub const fn from_u16(v: u16) -> Result<Self, Error> {
        match v & 0x3FFF {
            0x0001 => Ok(Self::Ping),
            0x0002 => Ok(Self::ComposeAnsi),
            0x0003 => Ok(Self::ComposeExtmarks),
            0x0004 => Ok(Self::RenderFile),
            0x0005 => Ok(Self::PreviewFile),
            0x0006 => Ok(Self::Shutdown),
            other => Err(Error::UnknownCommand(other)),
        }
    }
}

pub const FLAG_THEME_LIGHT: u32 = 1 << 0; // 0 = dark, 1 = light
pub const FLAG_TMUX: u32 = 1 << 1; // Client in tmux (wrap in DCS passthrough)
pub const FLAG_REMOTE: u32 = 1 << 2; // Client remote/SSH (enforce Base64, no SHM)

#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Header {
    pub magic: u32,
    pub version: u16,
    pub command: u16,
    pub req_id_lo: u32,
    pub req_id_hi: u32,
    pub flags: u32,
    pub payload_len: u32,
    pub cols: u16,
    pub rows: u16,
    pub cell_w: u16,
    pub cell_h: u16,
}
const _: () = assert!(size_of::<Header>() == 32);

impl Header {
    #[inline]
    #[must_use]
    pub const fn new(cmd: Cmd, payload_len: u32) -> Self {
        Self {
            magic: MAGIC,
            version: VERSION,
            command: cmd as u16,
            req_id_lo: 0,
            req_id_hi: 0,
            flags: 0,
            payload_len,
            cols: 80,
            rows: 24,
            cell_w: 8,
            cell_h: 16,
        }
    }

    #[inline]
    #[must_use]
    pub const fn from_le_bytes(b: &[u8; 32]) -> Self {
        Self {
            magic: u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            version: u16::from_le_bytes([b[4], b[5]]),
            command: u16::from_le_bytes([b[6], b[7]]),
            req_id_lo: u32::from_le_bytes([b[8], b[9], b[10], b[11]]),
            req_id_hi: u32::from_le_bytes([b[12], b[13], b[14], b[15]]),
            flags: u32::from_le_bytes([b[16], b[17], b[18], b[19]]),
            payload_len: u32::from_le_bytes([b[20], b[21], b[22], b[23]]),
            cols: u16::from_le_bytes([b[24], b[25]]),
            rows: u16::from_le_bytes([b[26], b[27]]),
            cell_w: u16::from_le_bytes([b[28], b[29]]),
            cell_h: u16::from_le_bytes([b[30], b[31]]),
        }
    }

    #[inline]
    #[must_use]
    pub const fn to_le_bytes(&self) -> [u8; 32] {
        let [m0, m1, m2, m3] = self.magic.to_le_bytes();
        let [v0, v1] = self.version.to_le_bytes();
        let [c0, c1] = self.command.to_le_bytes();
        let [rlo0, rlo1, rlo2, rlo3] = self.req_id_lo.to_le_bytes();
        let [rhi0, rhi1, rhi2, rhi3] = self.req_id_hi.to_le_bytes();
        let [f0, f1, f2, f3] = self.flags.to_le_bytes();
        let [pl0, pl1, pl2, pl3] = self.payload_len.to_le_bytes();
        let [co0, co1] = self.cols.to_le_bytes();
        let [rw0, rw1] = self.rows.to_le_bytes();
        let [cw0, cw1] = self.cell_w.to_le_bytes();
        let [ch0, ch1] = self.cell_h.to_le_bytes();
        [
            m0, m1, m2, m3, v0, v1, c0, c1, rlo0, rlo1, rlo2, rlo3, rhi0, rhi1, rhi2, rhi3, f0, f1,
            f2, f3, pl0, pl1, pl2, pl3, co0, co1, rw0, rw1, cw0, cw1, ch0, ch1,
        ]
    }

    /// Validate header magic, version, payload limit, and command.
    ///
    /// # Errors
    /// Returns `Error::BadMagic`, `Error::BadVersion`, `Error::PayloadTooLarge`, or `Error::UnknownCommand`.
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

pub type ExtmarkRecord = Extmark;

impl Extmark {
    #[inline]
    #[must_use]
    pub const fn to_le_bytes(self) -> [u8; 16] {
        let [i0, i1, i2, i3] = self.image_id.to_le_bytes();
        let [sr0, sr1] = self.start_row.to_le_bytes();
        let [sc0, sc1] = self.start_col.to_le_bytes();
        let [er0, er1] = self.end_row.to_le_bytes();
        let [ec0, ec1] = self.end_col.to_le_bytes();
        let [co0, co1] = self.cols.to_le_bytes();
        let [ro0, ro1] = self.rows.to_le_bytes();
        [
            i0, i1, i2, i3, sr0, sr1, sc0, sc1, er0, er1, ec0, ec1, co0, co1, ro0, ro1,
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

pub fn encode_dual_response(cmd: Cmd, apc: &[u8], text: &[u8], out: &mut Vec<u8>) {
    let total = apc.len().saturating_add(text.len());
    let (Ok(payload_len), Ok(apc_len)) = (u32::try_from(total), u32::try_from(apc.len())) else {
        encode_error(cmd as u16, b"response too large", out);
        return;
    };
    if payload_len > MAX_PAYLOAD {
        encode_error(cmd as u16, b"response exceeds limit", out);
        return;
    }
    let mut hdr = Header::new(cmd, payload_len);
    hdr.command = (cmd as u16) | RESP_FLAG;
    hdr.flags = apc_len;
    out.clear();
    out.reserve(32 + total);
    out.extend_from_slice(&hdr.to_le_bytes());
    out.extend_from_slice(apc);
    out.extend_from_slice(text);
}

pub fn encode_error(cmd: u16, msg: &[u8], out: &mut Vec<u8>) {
    let clamped = &msg[..msg.len().min(MAX_PAYLOAD as usize)];
    let mut hdr = Header::new(
        Cmd::Ping,
        u32::try_from(clamped.len()).unwrap_or(MAX_PAYLOAD),
    );
    hdr.command = (cmd & 0x3FFF) | RESP_FLAG | ERR_FLAG;
    out.clear();
    out.reserve(32 + clamped.len());
    out.extend_from_slice(&hdr.to_le_bytes());
    out.extend_from_slice(clamped);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct SpanCoord {
    pub outer_start: u32,
    pub outer_end: u32,
    pub image_id: u32,
    pub cols: u16,
    pub rows: u16,
}
const _: () = assert!(size_of::<SpanCoord>() == 16);

pub fn compose_extmarks(
    src: &[u8],
    spans: &[SpanCoord],
    line_starts: &mut Vec<usize>,
    out: &mut Vec<Extmark>,
) {
    out.clear();
    out.reserve(spans.len());
    line_starts.clear();
    line_starts.push(0);
    for nl in memchr::memchr_iter(b'\n', src) {
        line_starts.push(nl + 1);
    }
    for sp in spans {
        let start = (sp.outer_start as usize).min(src.len());
        let end = (sp.outer_end as usize).clamp(start, src.len());
        let row_start = line_starts
            .partition_point(|&o| o <= start)
            .saturating_sub(1);
        let row_end = line_starts.partition_point(|&o| o <= end).saturating_sub(1);
        out.push(Extmark {
            image_id: sp.image_id,
            start_row: u16::try_from(row_start).unwrap_or(u16::MAX),
            start_col: u16::try_from(start.saturating_sub(line_starts[row_start]))
                .unwrap_or(u16::MAX),
            end_row: u16::try_from(row_end).unwrap_or(u16::MAX),
            end_col: u16::try_from(end.saturating_sub(line_starts[row_end])).unwrap_or(u16::MAX),
            cols: sp.cols,
            rows: sp.rows,
        });
    }
}

pub fn encode_extmarks(records: &[Extmark], out: &mut Vec<u8>) {
    out.reserve(records.len().saturating_mul(16));
    for rec in records {
        out.extend_from_slice(&rec.to_le_bytes());
    }
}

#[must_use]
pub fn socket_path() -> PathBuf {
    let mut p = std::env::var_os("XDG_RUNTIME_DIR").map_or_else(
        || PathBuf::from(format!("/tmp/ii-{}", getuid().as_raw())),
        |xdg| PathBuf::from(xdg).join("ii"),
    );
    p.push("ii.sock");
    p
}

#[must_use]
pub fn lock_path() -> PathBuf {
    let mut p = socket_path();
    p.set_file_name("ii.lock");
    p
}

/// Zero-copy synchronous client send via `writev(2)`.
///
/// # Errors
/// Returns `Error::PayloadTooLarge`, `Error::BadMagic`, `Error::BadVersion`, or `Error::Io`.
pub fn client_send(cmd: Cmd, payload: &[u8]) -> Result<Vec<u8>, Error> {
    client_send_to(cmd, payload, None)
}

/// Send to the selected socket.
///
/// # Errors
/// Returns transport or protocol errors.
pub fn client_send_to(
    cmd: Cmd,
    payload: &[u8],
    path: Option<&std::path::Path>,
) -> Result<Vec<u8>, Error> {
    let payload_len = u32::try_from(payload.len()).map_err(|_| Error::PayloadTooLarge(u32::MAX))?;
    if payload_len > MAX_PAYLOAD {
        return Err(Error::PayloadTooLarge(payload_len));
    }
    exchange(Header::new(cmd, payload_len), payload, path).map(|(_, body)| body)
}

/// Exchange a correlated request with bounded socket waits.
///
/// # Errors
/// Returns transport, remote-render or malformed-response errors.
pub fn exchange(
    header: Header,
    payload: &[u8],
    path: Option<&std::path::Path>,
) -> Result<(Header, Vec<u8>), Error> {
    header.validate()?;
    if header.payload_len as usize != payload.len() {
        return Err(Error::InvalidState);
    }
    let fd = socket(AddressFamily::UNIX, SocketType::STREAM, None)?;
    fcntl_setfd(&fd, FdFlags::CLOEXEC)?;
    let default_path = socket_path();
    let addr =
        SocketAddrUnix::new(path.unwrap_or(&default_path)).map_err(|_| Error::SocketSymlink)?;
    connect(&fd, &addr)?;

    for timeout in [
        rustix::net::sockopt::Timeout::Recv,
        rustix::net::sockopt::Timeout::Send,
    ] {
        rustix::net::sockopt::set_socket_timeout(
            &fd,
            timeout,
            Some(std::time::Duration::from_secs(5)),
        )?;
    }
    let hdr = header.to_le_bytes();
    write_vectored(fd.as_fd(), &hdr, payload)?;

    let mut resp_hdr = [0u8; 32];
    read_exact(fd.as_fd(), &mut resp_hdr)?;
    let parsed = Header::from_le_bytes(&resp_hdr);
    parsed.validate()?;
    if parsed.command != header.command | RESP_FLAG
        || parsed.req_id_lo != header.req_id_lo
        || parsed.req_id_hi != header.req_id_hi
        || parsed.flags > parsed.payload_len
    {
        return Err(Error::InvalidState);
    }

    let mut body = vec![0u8; parsed.payload_len as usize];
    read_exact(fd.as_fd(), &mut body)?;
    Ok((parsed, body))
}

fn write_vectored(fd: BorrowedFd<'_>, mut a: &[u8], mut b: &[u8]) -> Result<(), Error> {
    while !a.is_empty() || !b.is_empty() {
        let iov = [IoSlice::new(a), IoSlice::new(b)];
        let slice = if a.is_empty() { &iov[1..] } else { &iov[..] };
        match writev(fd, slice) {
            Ok(0) => return Err(Error::Io(Errno::PIPE)),
            Ok(n) if n < a.len() => a = &a[n..],
            Ok(n) => {
                b = &b[n - a.len()..];
                a = &[];
            }
            Err(Errno::INTR) => {}
            Err(e) => return Err(Error::Io(e)),
        }
    }
    Ok(())
}

fn read_exact(fd: BorrowedFd<'_>, mut buf: &mut [u8]) -> Result<(), Error> {
    while !buf.is_empty() {
        match read(fd, &mut *buf) {
            Ok(0) => return Err(Error::Io(Errno::CONNRESET)),
            Ok(n) => buf = &mut buf[n..],
            Err(Errno::INTR) => {}
            Err(e) => return Err(Error::Io(e)),
        }
    }
    Ok(())
}
