//! Flat, register-sized error (`<= 12` B). Zero heap payloads.

use core::fmt;

use rustix::io::Errno;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Io(Errno),
    TtyUnavailable,
    SocketSymlink,
    SocketUidMismatch,
    DaemonLocked,
    BadMagic,
    BadVersion(u16),
    PayloadTooLarge(u32),
    UnknownCommand(u16),
    SvgParse,
    LatexCompile,
    MermaidCompile,
    PngDecode,
    Lz4Decompress,
    CacheCorrupt,
    ImageTooLarge { w: u32, h: u32 },
    JsonNav,
    ShmExhausted,
    TooManyClients,
}

const _: () = assert!(size_of::<Error>() <= 12);

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match *self {
            Self::Io(e) => write!(f, "io: {e}"),
            Self::TtyUnavailable => f.write_str("/dev/tty unavailable"),
            Self::SocketSymlink => f.write_str("socket path is symlink"),
            Self::SocketUidMismatch => f.write_str("socket owned by other uid"),
            Self::DaemonLocked => f.write_str("daemon already running"),
            Self::BadMagic => f.write_str("bad IXPC magic"),
            Self::BadVersion(v) => write!(f, "unsupported IXPC version: {v}"),
            Self::PayloadTooLarge(n) => write!(f, "payload {n} exceeds 64 MiB"),
            Self::UnknownCommand(c) => write!(f, "unknown command 0x{c:04x}"),
            Self::SvgParse => f.write_str("SVG parse failed"),
            Self::LatexCompile => f.write_str("LaTeX compilation failed"),
            Self::MermaidCompile => f.write_str("Mermaid compilation failed"),
            Self::PngDecode => f.write_str("PNG decode failed"),
            Self::Lz4Decompress => f.write_str("LZ4 decompression failed"),
            Self::CacheCorrupt => f.write_str("cache blob corrupt"),
            Self::ImageTooLarge { w, h } => write!(f, "image {w}x{h} exceeds limit"),
            Self::JsonNav => f.write_str("JSON navigation failed"),
            Self::ShmExhausted => f.write_str("SHM allocation failed"),
            Self::TooManyClients => f.write_str("max 32 concurrent clients"),
        }
    }
}

impl core::error::Error for Error {}

impl From<Errno> for Error {
    #[inline]
    fn from(e: Errno) -> Self {
        Self::Io(e)
    }
}
