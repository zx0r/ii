//! Alignment-safe IXPC v1 binary wire codec: 32 B `Header` and 16 B `Extmark`. Zero `transmute`.

#![expect(
    clippy::cast_possible_truncation,
    reason = "payload sizes bounded to <= MAX_PAYLOAD (64 MiB)"
)]

use crate::err::Error;

pub const MAGIC: u32 = 0x4958_5043; // "IXPC"
pub const VERSION: u16 = 1;
pub const MAX_PAYLOAD: u32 = 0x0400_0000; // 64 MiB
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
    pub _reserved: u64,
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
            _reserved: 0,
        }
    }

    /// Explicit byte-slice codec. Zero `transmute`, zero alignment UB.
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
            _reserved: u64::from_le_bytes([b[24], b[25], b[26], b[27], b[28], b[29], b[30], b[31]]),
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
        let rs = self._reserved.to_le_bytes();
        [
            m[0], m[1], m[2], m[3], v[0], v[1], c[0], c[1], f[0], f[1], f[2], f[3], co[0], co[1],
            ro[0], ro[1], cw[0], cw[1], ch[0], ch[1], pl[0], pl[1], pl[2], pl[3], rs[0], rs[1],
            rs[2], rs[3], rs[4], rs[5], rs[6], rs[7],
        ]
    }

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

/// Encode dual-channel response (`[u32 apc_len][apc][text]`) prefixed by a 32-byte IXPC response header.
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

/// Encode an error response frame into `out`.
pub fn encode_error(cmd: Cmd, msg: &str, out: &mut Vec<u8>) {
    let bytes = msg.as_bytes();
    let mut hdr = Header::new(cmd, bytes.len() as u32);
    hdr.command = (cmd as u16) | RESP_FLAG | ERR_FLAG;
    out.clear();
    out.reserve(32 + bytes.len());
    out.extend_from_slice(&hdr.to_le_bytes());
    out.extend_from_slice(bytes);
}

/// Encode `[Extmark]` records directly into a little-endian byte payload.
pub fn encode_extmarks(records: &[Extmark], out: &mut Vec<u8>) {
    out.reserve(records.len() * 16);
    for rec in records {
        out.extend_from_slice(&rec.to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_and_extmark_roundtrip() {
        let h = Header::new(Cmd::ComposeAnsi, 128);
        let raw = h.to_le_bytes();
        let decoded = Header::from_le_bytes(&raw);
        assert_eq!(h, decoded);
        assert_eq!(decoded.validate(), Ok(Cmd::ComposeAnsi));

        let ext = Extmark {
            image_id: 0x0012_3456,
            start_row: 1,
            start_col: 0,
            end_row: 4,
            end_col: 20,
            cols: 20,
            rows: 3,
        };
        let eb = ext.to_le_bytes();
        assert_eq!(Extmark::from_le_bytes(&eb), ext);
    }
}
