//! 4-way set-associative L0 VRAM tag `Table` (4 160 B), 4-way L1 RGBA `Cache`, and `wymix` identity hash.


/// Update 3-bit Tree-PLRU state after accessing `way in 0..4`.
#[inline]
const fn plru_touch(bits: u8, way: usize) -> u8 {
    match way & 3 {
        0 => bits | 0b011,
        1 => (bits | 0b001) & !0b010,
        2 => (bits & !0b001) | 0b100,
        _ => bits & !0b101,
    }
}

/// Select victim way from 3-bit Tree-PLRU state. Returns `0..4`.
#[inline]
const fn plru_victim(bits: u8) -> usize {
    let b0 = (bits & 1) as usize;
    let b_sub = ((bits >> (1 + b0)) & 1) as usize;
    (b0 << 1) | b_sub
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct Payload {
    pub image_id: u32,
    pub cols: u16,
    pub rows: u16,
}

const _: () = assert!(size_of::<Payload>() == 8);

impl Payload {
    pub const ZERO: Self = Self {
        image_id: 0,
        cols: 0,
        rows: 0,
    };
}

#[derive(Clone, Copy, Debug)]
#[repr(C, align(64))]
pub struct Set {
    pub tags: [u64; 4],
    pub entries: [Payload; 4],
}

const _: () = assert!(size_of::<Set>() == 64);
const _: () = assert!(align_of::<Set>() == 64);

impl Set {
    pub const ZERO: Self = Self {
        tags: [0; 4],
        entries: [Payload::ZERO; 4],
    };
}

/// L0 VRAM tag table: 64 sets × 4 ways = 256 resident KGP slots (4 160 B total, L1d-resident).
#[derive(Debug)]
#[repr(C, align(64))]
pub struct Table {
    sets: [Set; 64],
    plru: [u8; 64],
}

const _: () = assert!(size_of::<Table>() == 4160);

impl Table {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            sets: [Set::ZERO; 64],
            plru: [0; 64],
        }
    }

    #[inline]
    pub fn lookup(&mut self, tag: u64) -> Option<Payload> {
        let idx = ((tag & 63) as u8) as usize;
        let set = &self.sets[idx];
        for way in 0..4 {
            if set.tags[way] == tag {
                self.plru[idx] = plru_touch(self.plru[idx], way);
                return Some(set.entries[way]);
            }
        }
        None
    }

    #[inline]
    pub fn insert(&mut self, tag: u64, entry: Payload) {
        let idx = ((tag & 63) as u8) as usize;
        let set = &mut self.sets[idx];
        for way in 0..4 {
            if set.tags[way] == tag || set.tags[way] == 0 {
                set.tags[way] = tag;
                set.entries[way] = entry;
                self.plru[idx] = plru_touch(self.plru[idx], way);
                return;
            }
        }
        let victim = plru_victim(self.plru[idx]);
        set.tags[victim] = tag;
        set.entries[victim] = entry;
        self.plru[idx] = plru_touch(self.plru[idx], victim);
    }

    #[inline]
    pub fn invalidate(&mut self) {
        for set in &mut self.sets {
            set.tags = [0; 4];
        }
        self.plru = [0; 64];
    }
}

impl Default for Table {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug)]
pub struct Replica {
    pub cols: u16,
    pub rows: u16,
    pub w: u16,
    pub h: u16,
    pub rgba: Vec<u8>,
}

impl Replica {
    const fn empty() -> Self {
        Self {
            cols: 0,
            rows: 0,
            w: 0,
            h: 0,
            rgba: Vec::new(),
        }
    }
}

/// L1 RAM cache: 16 sets × 4 ways with capacity-preserving `Replica` buffers.
#[derive(Debug)]
pub struct Cache {
    tags: [[u64; 4]; 16],
    plru: [u8; 16],
    ways: Box<[[Replica; 4]; 16]>,
}

impl Cache {
    #[must_use]
    pub fn new() -> Self {
        Self {
            tags: [[0; 4]; 16],
            plru: [0; 16],
            ways: Box::new([const { [const { Replica::empty() }; 4] }; 16]),
        }
    }

    #[inline]
    pub fn lookup(&mut self, tag: u64) -> Option<&Replica> {
        let idx = (((tag >> 6) & 15) as u8) as usize;
        for way in 0..4 {
            if self.tags[idx][way] == tag {
                self.plru[idx] = plru_touch(self.plru[idx], way);
                return Some(&self.ways[idx][way]);
            }
        }
        None
    }

    #[inline]
    pub fn insert(&mut self, tag: u64, cols: u16, rows: u16, w: u16, h: u16, rgba: &[u8]) {
        let idx = (((tag >> 6) & 15) as u8) as usize;
        let way = (0..4)
            .find(|&way_idx| self.tags[idx][way_idx] == tag || self.tags[idx][way_idx] == 0)
            .unwrap_or_else(|| plru_victim(self.plru[idx]));
        self.tags[idx][way] = tag;
        self.plru[idx] = plru_touch(self.plru[idx], way);
        let slot = &mut self.ways[idx][way];
        slot.cols = cols;
        slot.rows = rows;
        slot.w = w;
        slot.h = h;
        slot.rgba.clear();
        slot.rgba.extend_from_slice(rgba);
    }
}

impl Default for Cache {
    fn default() -> Self {
        Self::new()
    }
}

/// 64-bit content+geometry `wymix` hash. Folds `cell_w`, `cell_h`, `cols`, `kind`, `fg_rgb`, `payload`.
/// Guaranteed non-zero (`0` = empty sentinel).
#[inline]
#[must_use]
pub fn compute_tag(
    cell_w: u16,
    cell_h: u16,
    cols: u16,
    kind: u8,
    fg_rgb: [u8; 3],
    payload: &[u8],
) -> u64 {
    const P0: u64 = 0xa076_1d64_78bd_642f;
    const P1: u64 = 0xe703_7ed1_a0b4_28db;

    #[inline]
    const fn wymix(a: u64, b: u64) -> u64 {
        let r = (a as u128).wrapping_mul(b as u128);
        let bytes = r.to_le_bytes();
        let lo = u64::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7]]);
        let hi = u64::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11], bytes[12], bytes[13], bytes[14], bytes[15]]);
        lo ^ hi
    }

    let meta = u64::from(cell_w)
        | (u64::from(cell_h) << 16)
        | (u64::from(cols) << 32)
        | (u64::from(kind) << 48)
        | (u64::from(fg_rgb[0]) << 56);
    let mut seed = wymix(
        meta ^ P0,
        ((u64::from(fg_rgb[1]) << 8) | u64::from(fg_rgb[2])) ^ P1,
    );

    let mut i = 0;
    while i + 8 <= payload.len() {
        let v = u64::from_le_bytes([
            payload[i],
            payload[i + 1],
            payload[i + 2],
            payload[i + 3],
            payload[i + 4],
            payload[i + 5],
            payload[i + 6],
            payload[i + 7],
        ]);
        seed = wymix(seed ^ P0, v ^ P1);
        i += 8;
    }
    if i < payload.len() {
        let mut tail = 0u64;
        let mut shift = 0;
        while i < payload.len() {
            tail |= u64::from(payload[i]) << shift;
            shift += 8;
            i += 1;
        }
        seed = wymix(seed ^ P0, tail ^ P1);
    }
    wymix(seed ^ (payload.len() as u64), P1).max(1)
}

/// Canonical 24-bit non-zero KGP `image_id` from `wymix` tag.
#[inline]
#[must_use]
pub const fn image_id_from_tag(tag: u64) -> u32 {
    let folded = tag ^ (tag >> 24) ^ (tag >> 48);
    let bytes = folded.to_le_bytes();
    let id = u32::from_le_bytes([bytes[0], bytes[1], bytes[2], 0]);
    if id == 0 { 1 } else { id }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_4way_plru_evicts_least_recently_used() {
        let mut tbl = Table::new();
        for i in 0..4u32 {
            tbl.insert(7 + u64::from(i) * 64, Payload { image_id: 10 + i, cols: 1, rows: 1 });
        }
        for &t in &[7, 7 + 128, 7 + 192] {
            assert!(tbl.lookup(t).is_some());
        }
        tbl.insert(7 + 256, Payload { image_id: 14, cols: 1, rows: 1 });
        assert_eq!(tbl.lookup(7 + 64), None);
        assert_eq!(tbl.lookup(7 + 256).map(|p| p.image_id), Some(14));
    }

    #[test]
    fn l1_cache_preserves_rgba_capacity_on_eviction() {
        let mut cache = Cache::new();
        let base_tag = 1u64 << 6;
        for way in 0..4u64 {
            let tag = base_tag + (way << 10) + 1;
            cache.insert(tag, 10, 5, 80, 80, &[0xAA; 256]);
        }
        let victim_tag = base_tag + (4 << 10) + 1;
        cache.insert(victim_tag, 4, 2, 32, 32, &[0xBB; 16]);
        let rep = cache.lookup(victim_tag);
        assert!(rep.is_some());
        if let Some(r) = rep {
            assert_eq!(r.rgba.len(), 16);
            assert!(r.rgba.capacity() >= 256);
        }
    }

    #[test]
    fn compute_tag_and_image_id_never_zero() {
        let t = compute_tag(8, 16, 80, 0, [205, 214, 244], b"E = mc^2");
        assert_ne!(t, 0);
        let id = image_id_from_tag(t);
        assert_ne!(id, 0);
        assert!(id <= 0x00FF_FFFF);
        assert_eq!(image_id_from_tag(0), 1);
    }
}
