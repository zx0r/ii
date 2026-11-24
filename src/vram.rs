//! Byte-bounded, set-associative cache of rendered RGBA images.

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
        let bytes = (raw ^ (raw >> 32)).to_le_bytes();
        match NonZeroU32::new(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]])) {
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
    bytes: usize,
}

impl Cache {
    pub const MAX_RETAINED_SLOT_CAP: usize = 2 * 1024 * 1024;

    #[must_use]
    pub fn new() -> Self {
        Self {
            tags: [[None; 4]; 16],
            plru: [0; 16],
            ways: Box::new([const { [const { Replica::empty() }; 4] }; 16]),
            bytes: 0,
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
        let slot = &mut self.ways[idx][way];
        self.bytes -= slot.rgba.capacity();
        if slot.rgba.capacity() < rgba.len()
            || (slot.rgba.capacity() > Self::MAX_RETAINED_SLOT_CAP
                && rgba.len() <= Self::MAX_RETAINED_SLOT_CAP)
        {
            slot.rgba = rgba.to_vec();
        } else {
            slot.rgba.clear();
            slot.rgba.extend_from_slice(rgba);
        }
        self.bytes += slot.rgba.capacity();
        (slot.cols, slot.rows, slot.w, slot.h) = (cols, rows, w, h);
        self.tags[idx][way] = Some(tag);
        self.plru[idx] = plru_touch(self.plru[idx], way);

        while self.bytes > BUDGET {
            let victim = self
                .ways
                .iter()
                .enumerate()
                .flat_map(|(set, ways)| {
                    ways.iter()
                        .enumerate()
                        .map(move |(way, entry)| (set, way, entry.rgba.capacity()))
                })
                .filter(|&(set, cold, capacity)| capacity > 0 && (set, cold) != (idx, way))
                .max_by_key(|&(set, way, capacity)| (plru_victim(self.plru[set]) == way, capacity));
            let Some((set, evict, capacity)) = victim else {
                break;
            };
            self.bytes -= capacity;
            self.tags[set][evict] = None;
            self.ways[set][evict] = Replica::empty();
        }
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
