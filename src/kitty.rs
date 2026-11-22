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

/// 297 canonical combining-class-230 row/column diacritics (UTF-8) for KGP  placeholders.
#[rustfmt::skip]
pub static DIACRITICS_UTF8: [&[u8]; 297] = [
    b"\xCC\x85", // 0: U+0305
    b"\xCC\x8D", // 1: U+030D
    b"\xCC\x8E", // 2: U+030E
    b"\xCC\x90", // 3: U+0310
    b"\xCC\x92", // 4: U+0312
    b"\xCC\xBD", // 5: U+033D
    b"\xCC\xBE", // 6: U+033E
    b"\xCC\xBF", // 7: U+033F
    b"\xCD\x86", // 8: U+0346
    b"\xCD\x8A", // 9: U+034A
    b"\xCD\x8B", // 10: U+034B
    b"\xCD\x8C", // 11: U+034C
    b"\xCD\x90", // 12: U+0350
    b"\xCD\x91", // 13: U+0351
    b"\xCD\x92", // 14: U+0352
    b"\xCD\x97", // 15: U+0357
    b"\xCD\x9B", // 16: U+035B
    b"\xCD\xA3", // 17: U+0363
    b"\xCD\xA4", // 18: U+0364
    b"\xCD\xA5", // 19: U+0365
    b"\xCD\xA6", // 20: U+0366
    b"\xCD\xA7", // 21: U+0367
    b"\xCD\xA8", // 22: U+0368
    b"\xCD\xA9", // 23: U+0369
    b"\xCD\xAA", // 24: U+036A
    b"\xCD\xAB", // 25: U+036B
    b"\xCD\xAC", // 26: U+036C
    b"\xCD\xAD", // 27: U+036D
    b"\xCD\xAE", // 28: U+036E
    b"\xCD\xAF", // 29: U+036F
    b"\xD2\x83", // 30: U+0483
    b"\xD2\x84", // 31: U+0484
    b"\xD2\x85", // 32: U+0485
    b"\xD2\x86", // 33: U+0486
    b"\xD2\x87", // 34: U+0487
    b"\xD6\x92", // 35: U+0592
    b"\xD6\x93", // 36: U+0593
    b"\xD6\x94", // 37: U+0594
    b"\xD6\x95", // 38: U+0595
    b"\xD6\x97", // 39: U+0597
    b"\xD6\x98", // 40: U+0598
    b"\xD6\x99", // 41: U+0599
    b"\xD6\x9C", // 42: U+059C
    b"\xD6\x9D", // 43: U+059D
    b"\xD6\x9E", // 44: U+059E
    b"\xD6\x9F", // 45: U+059F
    b"\xD6\xA0", // 46: U+05A0
    b"\xD6\xA1", // 47: U+05A1
    b"\xD6\xA8", // 48: U+05A8
    b"\xD6\xA9", // 49: U+05A9
    b"\xD6\xAB", // 50: U+05AB
    b"\xD6\xAC", // 51: U+05AC
    b"\xD6\xAF", // 52: U+05AF
    b"\xD7\x84", // 53: U+05C4
    b"\xD8\x90", // 54: U+0610
    b"\xD8\x91", // 55: U+0611
    b"\xD8\x92", // 56: U+0612
    b"\xD8\x93", // 57: U+0613
    b"\xD8\x94", // 58: U+0614
    b"\xD8\x95", // 59: U+0615
    b"\xD8\x96", // 60: U+0616
    b"\xD8\x97", // 61: U+0617
    b"\xD9\x97", // 62: U+0657
    b"\xD9\x98", // 63: U+0658
    b"\xD9\x99", // 64: U+0659
    b"\xD9\x9A", // 65: U+065A
    b"\xD9\x9B", // 66: U+065B
    b"\xD9\x9D", // 67: U+065D
    b"\xD9\x9E", // 68: U+065E
    b"\xDB\x96", // 69: U+06D6
    b"\xDB\x97", // 70: U+06D7
    b"\xDB\x98", // 71: U+06D8
    b"\xDB\x99", // 72: U+06D9
    b"\xDB\x9A", // 73: U+06DA
    b"\xDB\x9B", // 74: U+06DB
    b"\xDB\x9C", // 75: U+06DC
    b"\xDB\x9F", // 76: U+06DF
    b"\xDB\xA0", // 77: U+06E0
    b"\xDB\xA1", // 78: U+06E1
    b"\xDB\xA2", // 79: U+06E2
    b"\xDB\xA4", // 80: U+06E4
    b"\xDB\xA7", // 81: U+06E7
    b"\xDB\xA8", // 82: U+06E8
    b"\xDB\xAB", // 83: U+06EB
    b"\xDB\xAC", // 84: U+06EC
    b"\xDC\xB0", // 85: U+0730
    b"\xDC\xB2", // 86: U+0732
    b"\xDC\xB3", // 87: U+0733
    b"\xDC\xB5", // 88: U+0735
    b"\xDC\xB6", // 89: U+0736
    b"\xDC\xBA", // 90: U+073A
    b"\xDC\xBD", // 91: U+073D
    b"\xDC\xBF", // 92: U+073F
    b"\xDD\x80", // 93: U+0740
    b"\xDD\x81", // 94: U+0741
    b"\xDD\x83", // 95: U+0743
    b"\xDD\x85", // 96: U+0745
    b"\xDD\x87", // 97: U+0747
    b"\xDD\x89", // 98: U+0749
    b"\xDD\x8A", // 99: U+074A
    b"\xDF\xAB", // 100: U+07EB
    b"\xDF\xAC", // 101: U+07EC
    b"\xDF\xAD", // 102: U+07ED
    b"\xDF\xAE", // 103: U+07EE
    b"\xDF\xAF", // 104: U+07EF
    b"\xDF\xB0", // 105: U+07F0
    b"\xDF\xB1", // 106: U+07F1
    b"\xDF\xB3", // 107: U+07F3
    b"\xE0\xA0\x96", // 108: U+0816
    b"\xE0\xA0\x97", // 109: U+0817
    b"\xE0\xA0\x98", // 110: U+0818
    b"\xE0\xA0\x99", // 111: U+0819
    b"\xE0\xA0\x9B", // 112: U+081B
    b"\xE0\xA0\x9C", // 113: U+081C
    b"\xE0\xA0\x9D", // 114: U+081D
    b"\xE0\xA0\x9E", // 115: U+081E
    b"\xE0\xA0\x9F", // 116: U+081F
    b"\xE0\xA0\xA0", // 117: U+0820
    b"\xE0\xA0\xA1", // 118: U+0821
    b"\xE0\xA0\xA2", // 119: U+0822
    b"\xE0\xA0\xA3", // 120: U+0823
    b"\xE0\xA0\xA5", // 121: U+0825
    b"\xE0\xA0\xA6", // 122: U+0826
    b"\xE0\xA0\xA7", // 123: U+0827
    b"\xE0\xA0\xA9", // 124: U+0829
    b"\xE0\xA0\xAA", // 125: U+082A
    b"\xE0\xA0\xAB", // 126: U+082B
    b"\xE0\xA0\xAC", // 127: U+082C
    b"\xE0\xA0\xAD", // 128: U+082D
    b"\xE0\xA5\x91", // 129: U+0951
    b"\xE0\xA5\x93", // 130: U+0953
    b"\xE0\xA5\x94", // 131: U+0954
    b"\xE0\xBE\x82", // 132: U+0F82
    b"\xE0\xBE\x83", // 133: U+0F83
    b"\xE0\xBE\x86", // 134: U+0F86
    b"\xE0\xBE\x87", // 135: U+0F87
    b"\xE1\x8D\x9D", // 136: U+135D
    b"\xE1\x8D\x9E", // 137: U+135E
    b"\xE1\x8D\x9F", // 138: U+135F
    b"\xE1\x9F\x9D", // 139: U+17DD
    b"\xE1\xA4\xBA", // 140: U+193A
    b"\xE1\xA8\x97", // 141: U+1A17
    b"\xE1\xA9\xB5", // 142: U+1A75
    b"\xE1\xA9\xB6", // 143: U+1A76
    b"\xE1\xA9\xB7", // 144: U+1A77
    b"\xE1\xA9\xB8", // 145: U+1A78
    b"\xE1\xA9\xB9", // 146: U+1A79
    b"\xE1\xA9\xBA", // 147: U+1A7A
    b"\xE1\xA9\xBB", // 148: U+1A7B
    b"\xE1\xA9\xBC", // 149: U+1A7C
    b"\xE1\xAD\xAB", // 150: U+1B6B
    b"\xE1\xAD\xAD", // 151: U+1B6D
    b"\xE1\xAD\xAE", // 152: U+1B6E
    b"\xE1\xAD\xAF", // 153: U+1B6F
    b"\xE1\xAD\xB0", // 154: U+1B70
    b"\xE1\xAD\xB1", // 155: U+1B71
    b"\xE1\xAD\xB2", // 156: U+1B72
    b"\xE1\xAD\xB3", // 157: U+1B73
    b"\xE1\xB3\x90", // 158: U+1CD0
    b"\xE1\xB3\x91", // 159: U+1CD1
    b"\xE1\xB3\x92", // 160: U+1CD2
    b"\xE1\xB3\x9A", // 161: U+1CDA
    b"\xE1\xB3\x9B", // 162: U+1CDB
    b"\xE1\xB3\xA0", // 163: U+1CE0
    b"\xE1\xB7\x80", // 164: U+1DC0
    b"\xE1\xB7\x81", // 165: U+1DC1
    b"\xE1\xB7\x83", // 166: U+1DC3
    b"\xE1\xB7\x84", // 167: U+1DC4
    b"\xE1\xB7\x85", // 168: U+1DC5
    b"\xE1\xB7\x86", // 169: U+1DC6
    b"\xE1\xB7\x87", // 170: U+1DC7
    b"\xE1\xB7\x88", // 171: U+1DC8
    b"\xE1\xB7\x89", // 172: U+1DC9
    b"\xE1\xB7\x8B", // 173: U+1DCB
    b"\xE1\xB7\x8C", // 174: U+1DCC
    b"\xE1\xB7\x91", // 175: U+1DD1
    b"\xE1\xB7\x92", // 176: U+1DD2
    b"\xE1\xB7\x93", // 177: U+1DD3
    b"\xE1\xB7\x94", // 178: U+1DD4
    b"\xE1\xB7\x95", // 179: U+1DD5
    b"\xE1\xB7\x96", // 180: U+1DD6
    b"\xE1\xB7\x97", // 181: U+1DD7
    b"\xE1\xB7\x98", // 182: U+1DD8
    b"\xE1\xB7\x99", // 183: U+1DD9
    b"\xE1\xB7\x9A", // 184: U+1DDA
    b"\xE1\xB7\x9B", // 185: U+1DDB
    b"\xE1\xB7\x9C", // 186: U+1DDC
    b"\xE1\xB7\x9D", // 187: U+1DDD
    b"\xE1\xB7\x9E", // 188: U+1DDE
    b"\xE1\xB7\x9F", // 189: U+1DDF
    b"\xE1\xB7\xA0", // 190: U+1DE0
    b"\xE1\xB7\xA1", // 191: U+1DE1
    b"\xE1\xB7\xA2", // 192: U+1DE2
    b"\xE1\xB7\xA3", // 193: U+1DE3
    b"\xE1\xB7\xA4", // 194: U+1DE4
    b"\xE1\xB7\xA5", // 195: U+1DE5
    b"\xE1\xB7\xA6", // 196: U+1DE6
    b"\xE1\xB7\xBE", // 197: U+1DFE
    b"\xE2\x83\x90", // 198: U+20D0
    b"\xE2\x83\x91", // 199: U+20D1
    b"\xE2\x83\x94", // 200: U+20D4
    b"\xE2\x83\x95", // 201: U+20D5
    b"\xE2\x83\x96", // 202: U+20D6
    b"\xE2\x83\x97", // 203: U+20D7
    b"\xE2\x83\x9B", // 204: U+20DB
    b"\xE2\x83\x9C", // 205: U+20DC
    b"\xE2\x83\xA1", // 206: U+20E1
    b"\xE2\x83\xA7", // 207: U+20E7
    b"\xE2\x83\xA9", // 208: U+20E9
    b"\xE2\x83\xB0", // 209: U+20F0
    b"\xE2\xB3\xAF", // 210: U+2CEF
    b"\xE2\xB3\xB0", // 211: U+2CF0
    b"\xE2\xB3\xB1", // 212: U+2CF1
    b"\xE2\xB7\xA0", // 213: U+2DE0
    b"\xE2\xB7\xA1", // 214: U+2DE1
    b"\xE2\xB7\xA2", // 215: U+2DE2
    b"\xE2\xB7\xA3", // 216: U+2DE3
    b"\xE2\xB7\xA4", // 217: U+2DE4
    b"\xE2\xB7\xA5", // 218: U+2DE5
    b"\xE2\xB7\xA6", // 219: U+2DE6
    b"\xE2\xB7\xA7", // 220: U+2DE7
    b"\xE2\xB7\xA8", // 221: U+2DE8
    b"\xE2\xB7\xA9", // 222: U+2DE9
    b"\xE2\xB7\xAA", // 223: U+2DEA
    b"\xE2\xB7\xAB", // 224: U+2DEB
    b"\xE2\xB7\xAC", // 225: U+2DEC
    b"\xE2\xB7\xAD", // 226: U+2DED
    b"\xE2\xB7\xAE", // 227: U+2DEE
    b"\xE2\xB7\xAF", // 228: U+2DEF
    b"\xE2\xB7\xB0", // 229: U+2DF0
    b"\xE2\xB7\xB1", // 230: U+2DF1
    b"\xE2\xB7\xB2", // 231: U+2DF2
    b"\xE2\xB7\xB3", // 232: U+2DF3
    b"\xE2\xB7\xB4", // 233: U+2DF4
    b"\xE2\xB7\xB5", // 234: U+2DF5
    b"\xE2\xB7\xB6", // 235: U+2DF6
    b"\xE2\xB7\xB7", // 236: U+2DF7
    b"\xE2\xB7\xB8", // 237: U+2DF8
    b"\xE2\xB7\xB9", // 238: U+2DF9
    b"\xE2\xB7\xBA", // 239: U+2DFA
    b"\xE2\xB7\xBB", // 240: U+2DFB
    b"\xE2\xB7\xBC", // 241: U+2DFC
    b"\xE2\xB7\xBD", // 242: U+2DFD
    b"\xE2\xB7\xBE", // 243: U+2DFE
    b"\xE2\xB7\xBF", // 244: U+2DFF
    b"\xEA\x99\xAF", // 245: U+A66F
    b"\xEA\x99\xBC", // 246: U+A67C
    b"\xEA\x99\xBD", // 247: U+A67D
    b"\xEA\x9B\xB0", // 248: U+A6F0
    b"\xEA\x9B\xB1", // 249: U+A6F1
    b"\xEA\xA3\xA0", // 250: U+A8E0
    b"\xEA\xA3\xA1", // 251: U+A8E1
    b"\xEA\xA3\xA2", // 252: U+A8E2
    b"\xEA\xA3\xA3", // 253: U+A8E3
    b"\xEA\xA3\xA4", // 254: U+A8E4
    b"\xEA\xA3\xA5", // 255: U+A8E5
    b"\xEA\xA3\xA6", // 256: U+A8E6
    b"\xEA\xA3\xA7", // 257: U+A8E7
    b"\xEA\xA3\xA8", // 258: U+A8E8
    b"\xEA\xA3\xA9", // 259: U+A8E9
    b"\xEA\xA3\xAA", // 260: U+A8EA
    b"\xEA\xA3\xAB", // 261: U+A8EB
    b"\xEA\xA3\xAC", // 262: U+A8EC
    b"\xEA\xA3\xAD", // 263: U+A8ED
    b"\xEA\xA3\xAE", // 264: U+A8EE
    b"\xEA\xA3\xAF", // 265: U+A8EF
    b"\xEA\xA3\xB0", // 266: U+A8F0
    b"\xEA\xA3\xB1", // 267: U+A8F1
    b"\xEA\xAA\xB0", // 268: U+AAB0
    b"\xEA\xAA\xB2", // 269: U+AAB2
    b"\xEA\xAA\xB3", // 270: U+AAB3
    b"\xEA\xAA\xB7", // 271: U+AAB7
    b"\xEA\xAA\xB8", // 272: U+AAB8
    b"\xEA\xAA\xBE", // 273: U+AABE
    b"\xEA\xAA\xBF", // 274: U+AABF
    b"\xEA\xAB\x81", // 275: U+AAC1
    b"\xEF\xB8\xA0", // 276: U+FE20
    b"\xEF\xB8\xA1", // 277: U+FE21
    b"\xEF\xB8\xA2", // 278: U+FE22
    b"\xEF\xB8\xA3", // 279: U+FE23
    b"\xEF\xB8\xA4", // 280: U+FE24
    b"\xEF\xB8\xA5", // 281: U+FE25
    b"\xEF\xB8\xA6", // 282: U+FE26
    b"\xF0\x90\xA8\x8F", // 283: U+10A0F
    b"\xF0\x90\xA8\xB8", // 284: U+10A38
    b"\xF0\x9D\x86\x85", // 285: U+1D185
    b"\xF0\x9D\x86\x86", // 286: U+1D186
    b"\xF0\x9D\x86\x87", // 287: U+1D187
    b"\xF0\x9D\x86\x88", // 288: U+1D188
    b"\xF0\x9D\x86\x89", // 289: U+1D189
    b"\xF0\x9D\x86\xAA", // 290: U+1D1AA
    b"\xF0\x9D\x86\xAB", // 291: U+1D1AB
    b"\xF0\x9D\x86\xAC", // 292: U+1D1AC
    b"\xF0\x9D\x86\xAD", // 293: U+1D1AD
    b"\xF0\x9D\x89\x82", // 294: U+1D242
    b"\xF0\x9D\x89\x83", // 295: U+1D243
    b"\xF0\x9D\x89\x84", // 296: U+1D244
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
        text.extend_from_slice(r_diac);
        text.extend_from_slice(first_col);
        for _ in 1..cols {
            text.extend_from_slice(&PLACEHOLDER_UTF8);
        }
        text.extend_from_slice(b"\x1b[39m\n");
    }
}
