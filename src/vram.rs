//! 4-way set-associative L0 VRAM `Table` (4 160 B), L1 RGBA `Cache`, and `wymix` `Tag`.

use core::num::{NonZeroU32, NonZeroU64};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[repr(transparent)]
pub struct Tag(NonZeroU64);

const _: () = assert!(size_of::<Tag>() == 8);
const _: () = assert!(size_of::<Option<Tag>>() == 8);

impl Tag {
    #[inline]
    #[must_use]
    pub const fn new(raw: u64) -> Option<Self> {
        match NonZeroU64::new(raw) {
            Some(nz) => Some(Self(nz)),
            None => None,
        }
    }

    #[inline]
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0.get()
    }

    #[inline]
    #[must_use]
    pub const fn image_id(self) -> NonZeroU32 {
        let raw = self.0.get();
        let b = (raw ^ (raw >> 24) ^ (raw >> 48)).to_le_bytes();
        match NonZeroU32::new(u32::from_le_bytes([b[0], b[1], b[2], 0])) {
            Some(nz) => nz,
            None => NonZeroU32::MIN,
        }
    }
}

#[inline]
const fn plru_touch(bits: u8, way: usize) -> u8 {
    match way & 3 {
        0 => bits | 0b011,
        1 => (bits | 0b001) & !0b010,
        2 => (bits & !0b001) | 0b100,
        _ => bits & !0b101,
    }
}

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
    pub tags: [Option<Tag>; 4],
    pub entries: [Payload; 4],
}
const _: () = assert!(size_of::<Set>() == 64);
const _: () = assert!(align_of::<Set>() == 64);

impl Set {
    pub const ZERO: Self = Self {
        tags: [None; 4],
        entries: [Payload::ZERO; 4],
    };
}

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
    pub const fn lookup(&mut self, tag: Tag) -> Option<Payload> {
        let idx = (tag.get().to_le_bytes()[0] & 63) as usize;
        let set = &self.sets[idx];
        let mut way = 0;
        while way < 4 {
            if let Some(cur) = set.tags[way]
                && cur.get() == tag.get()
            {
                self.plru[idx] = plru_touch(self.plru[idx], way);
                return Some(set.entries[way]);
            }
            way += 1;
        }
        None
    }

    #[inline]
    pub const fn insert(&mut self, tag: Tag, entry: Payload) -> Option<Payload> {
        let idx = (tag.get().to_le_bytes()[0] & 63) as usize;
        let set = &mut self.sets[idx];
        let mut empty: Option<usize> = None;
        let mut way = 0;
        while way < 4 {
            match set.tags[way] {
                Some(cur) if cur.get() == tag.get() => {
                    set.entries[way] = entry;
                    self.plru[idx] = plru_touch(self.plru[idx], way);
                    return None;
                }
                None if empty.is_none() => empty = Some(way),
                _ => {}
            }
            way += 1;
        }
        if let Some(slot) = empty {
            set.tags[slot] = Some(tag);
            set.entries[slot] = entry;
            self.plru[idx] = plru_touch(self.plru[idx], slot);
            return None;
        }
        let victim = plru_victim(self.plru[idx]);
        let evicted = set.entries[victim];
        set.tags[victim] = Some(tag);
        set.entries[victim] = entry;
        self.plru[idx] = plru_touch(self.plru[idx], victim);
        Some(evicted)
    }

    #[inline]
    pub const fn invalidate(&mut self) {
        let mut i = 0;
        while i < 64 {
            self.sets[i].tags = [None; 4];
            i += 1;
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

#[derive(Debug)]
#[repr(C, align(64))]
pub struct Cache {
    tags: [[Option<Tag>; 4]; 16],
    plru: [u8; 16],
    ways: Box<[[Replica; 4]; 16]>,
}

impl Cache {
    pub const MAX_RETAINED_SLOT_CAP: usize = 2 * 1024 * 1024;

    #[must_use]
    pub fn new() -> Self {
        Self {
            tags: [[None; 4]; 16],
            plru: [0; 16],
            ways: Box::new([const { [const { Replica::empty() }; 4] }; 16]),
        }
    }

    #[inline]
    pub fn lookup(&mut self, tag: Tag) -> Option<&Replica> {
        let idx = usize::from((tag.get() >> 6).to_le_bytes()[0] & 15);
        for way in 0..4 {
            if self.tags[idx][way] == Some(tag) {
                self.plru[idx] = plru_touch(self.plru[idx], way);
                return Some(&self.ways[idx][way]);
            }
        }
        None
    }

    #[inline]
    pub fn insert(&mut self, tag: Tag, cols: u16, rows: u16, w: u16, h: u16, rgba: &[u8]) {
        const BUDGET: usize = 32 * 1024 * 1024;
        if rgba.len() > BUDGET {
            return;
        }

        let idx = usize::from((tag.get() >> 6).to_le_bytes()[0] & 15);
        let mut chosen = None;
        for (way, &slot_tag) in self.tags[idx].iter().enumerate() {
            if slot_tag == Some(tag) {
                chosen = Some(way);
                break;
            }
            if slot_tag.is_none() && chosen.is_none() {
                chosen = Some(way);
            }
        }
        let way = chosen.unwrap_or_else(|| plru_victim(self.plru[idx]));
        let replacing = self.ways[idx][way].rgba.capacity();
        let mut retained = self
            .ways
            .iter()
            .flatten()
            .map(|entry| entry.rgba.capacity())
            .sum::<usize>()
            .saturating_sub(replacing);

        // Evict only enough cold slots to fit the new image. Clearing the whole
        // table made every subsequent notebook page pay the rasterization cost again.
        while retained.saturating_add(rgba.len()) > BUDGET {
            let mut victim: Option<(usize, usize)> = None;
            for set in 0..self.ways.len() {
                for victim_way in 0..self.ways[set].len() {
                    if set == idx && victim_way == way {
                        continue;
                    }
                    let capacity = self.ways[set][victim_way].rgba.capacity();
                    if capacity > 0
                        && victim.is_none_or(|(best_set, best_way)| {
                            capacity < self.ways[best_set][best_way].rgba.capacity()
                        })
                    {
                        victim = Some((set, victim_way));
                    }
                }
            }
            let Some((victim_set, victim_way)) = victim else {
                break;
            };
            retained = retained.saturating_sub(self.ways[victim_set][victim_way].rgba.capacity());
            self.tags[victim_set][victim_way] = None;
            self.ways[victim_set][victim_way].rgba.clear();
            self.ways[victim_set][victim_way].rgba.shrink_to(0);
        }

        self.tags[idx][way] = Some(tag);
        self.plru[idx] = plru_touch(self.plru[idx], way);
        let slot = &mut self.ways[idx][way];
        (slot.cols, slot.rows, slot.w, slot.h) = (cols, rows, w, h);
        slot.rgba.clear();
        if slot.rgba.capacity() > Self::MAX_RETAINED_SLOT_CAP
            && rgba.len() <= Self::MAX_RETAINED_SLOT_CAP
        {
            slot.rgba.shrink_to(rgba.len().max(4096));
        }
        slot.rgba.extend_from_slice(rgba);
    }

    #[inline]
    pub const fn invalidate(&mut self) {
        self.tags = [[None; 4]; 16];
        self.plru = [0; 16];
    }
}

impl Default for Cache {
    fn default() -> Self {
        Self::new()
    }
}

#[inline]
const fn wymix(a: u64, b: u64) -> u64 {
    let r = (a as u128).wrapping_mul(b as u128);
    #[expect(
        clippy::cast_possible_truncation,
        reason = "split u128 product into 64-bit halves"
    )]
    let mixed = (r as u64) ^ ((r >> 64) as u64);
    mixed
}

#[inline]
#[must_use]
pub fn compute_tag(
    cell_w: u16,
    cell_h: u16,
    cols: u16,
    kind: u8,
    fg_rgb: [u8; 3],
    payload: &[u8],
) -> Tag {
    const P0: u64 = 0xa076_1d64_78bd_642f;
    const P1: u64 = 0xe703_7ed1_a0b4_28db;

    let meta = u64::from(cell_w)
        | (u64::from(cell_h) << 16)
        | (u64::from(cols) << 32)
        | (u64::from(kind) << 48)
        | (u64::from(fg_rgb[0]) << 56);
    let mut seed = wymix(
        meta ^ P0,
        ((u64::from(fg_rgb[1]) << 8) | u64::from(fg_rgb[2])) ^ P1,
    );

    let (chunks, rem) = payload.as_chunks::<8>();
    for c in chunks {
        seed = wymix(seed ^ P0, u64::from_le_bytes(*c) ^ P1);
    }
    if !rem.is_empty() {
        let mut tail = [0u8; 8];
        tail[..rem.len()].copy_from_slice(rem);
        seed = wymix(seed ^ P0, u64::from_le_bytes(tail) ^ P1);
    }

    let raw = wymix(seed ^ (payload.len() as u64), P1);
    Tag::new(raw | u64::from(raw == 0)).unwrap_or(Tag(NonZeroU64::MIN))
}
