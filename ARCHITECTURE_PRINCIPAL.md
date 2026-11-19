# ARCHITECTURE.md — `ii`: Terminal Graphical Coprocessor

> Edition 2024 · `resolver = "3"` · Single-crate binary · Rust ≥ 1.85 · ≤ 4 200 SLOC

---

## §0 Design Thesis

**Hourglass IR.** Ten pure frontends produce typed index spans (`ir::Span`) and flat SVG/SGR buffers
inside a pre-allocated frame arena (`ir::Scratch`). One unified backend (`raster` → `kgp` → `tty`)
renders and transports. Zero frontend pixel awareness; zero backend parser awareness.

**Three-tier set-associative cache.**

| Tier    | Structure                    | Size          | Eviction     | Key                               |
| ------- | ---------------------------- | ------------- | ------------ | --------------------------------- |
| L0 VRAM | `vram::Table` 64-set × 4-way | 4 160 B       | Tree-PLRU    | `wymix` 64-bit tag                |
| L1 RAM  | `vram::Cache` 16-set × 4-way | ~512 KiB pool | Tree-PLRU    | same `wymix` tag                  |
| L2 Disk | `cache` PX02 blobs           | unbounded     | LRU-by-atime | BLAKE3 (computed on L1 miss only) |

**Disjoint-borrow trinity.** `cfg::Cfg` (immutable) ⊥ `compose::Session` (mutable state) ⊥
`ir::Scratch` (frame-local arena). Zero `RefCell`. Zero `Arc`. Zero hot-path heap allocation.

**Single identity chain.** `wymix` 64-bit tag → deterministic 24-bit KGP `image_id` via
`vram::image_id_from_tag`. No dual-hash aliasing. BLAKE3 computed only on L1→L2 fallthrough.

---

## §1 File Tree & SLOC Budget

```
ii/
├── Cargo.toml
├── deny.toml
├── ARCHITECTURE.md
├── src/
│   ├── main.rs       ──  120  entry, hand-rolled CLI, SIGPIPE mask
│   ├── err.rs        ──   80  flat Error enum (≤ 12 B), Display, core::error::Error
│   ├── cfg.rs        ──   90  Viewport (8 B), Cfg, const defaults
│   ├── theme.rs      ──  180  Token (32 B), Palette, const SGR/Hex LUTs
│   ├── ir.rs         ──  260  Span (16 B), Kind (u4), Scratch arena
│   ├── scan.rs       ──  340  single-pass SWAR/memchr lexer → Vec<Span>
│   ├── heal.rs       ──  170  streaming LaTeX healer (brace/env/delim stacks)
│   ├── chem.rs       ──  150  \ce{} / \pu{} → LaTeX transpiler
│   ├── json.rs       ──  180  zero-alloc &[u8] JSON navigator
│   ├── chart.rs      ──  380  Vega-Lite + Plotly → SVG
│   ├── dot.rs        ──  340  Graphviz DOT → SVG (Sugiyama layout)
│   ├── csv.rs        ──  130  CSV/TSV + pipe-table → SVG table
│   ├── ipynb.rs      ──  190  .ipynb zero-serde cell extraction
│   ├── font.rs       ──  100  font specimen card → SVG
│   ├── style.rs      ──  240  Markdown → ANSI/SGR + OSC 8 + CJK wrap
│   ├── svg.rs        ──  110  single-pass streaming SVG theme rewriter
│   ├── raster.rs     ──  190  resvg render, cell-snap, Q24 unpremultiply
│   ├── cache.rs      ──  210  BLAKE3 L2 disk cache (PX02), openat/renameat
│   ├── vram.rs       ──  210  4-way L0 Table + 4-way L1 Cache, wymix
│   ├── kgp.rs        ──  210  KGP encoder: SHM/Base64 transport, diacr LUT
│   ├── tty.rs        ──  150  /dev/tty probe, OSC 10/11, winsize, Metrics
│   ├── ipc.rs        ──  180  IXPC v1 wire codec, Header (32 B), Extmark (16 B)
│   ├── daemon.rs     ──  260  non-blocking poll reactor, 32-slot client FSM
│   └── compose.rs    ──  160  frame orchestrator, CompileCtx, FORMAT_TABLE
│                     ─────────
│               TOTAL ≈ 4 200 SLOC
├── tests/
│   ├── scan_test.rs
│   ├── heal_test.rs
│   ├── vram_test.rs
│   ├── ipc_test.rs
│   └── cache_test.rs
└── benches/
    ├── scan_bench.rs
    └── compose_bench.rs
```

**Depth:** max 2. No `domain/`, `service/`, `handler/`, `dto/`.

---

## §2 Core Data Layouts

### 2.1 `err.rs` — Flat Error Enum (≤ 12 bytes)

```rust
use core::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Io(rustix::io::Errno),
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
            Self::Io(e)              => write!(f, "io: {e}"),
            Self::TtyUnavailable     => f.write_str("/dev/tty unavailable"),
            Self::SocketSymlink      => f.write_str("socket path is symlink"),
            Self::SocketUidMismatch  => f.write_str("socket owned by other uid"),
            Self::DaemonLocked       => f.write_str("daemon already running"),
            Self::BadMagic           => f.write_str("bad IXPC magic"),
            Self::BadVersion(v)      => write!(f, "unsupported IXPC version: {v}"),
            Self::PayloadTooLarge(n) => write!(f, "payload {n} exceeds 64 MiB"),
            Self::UnknownCommand(c)  => write!(f, "unknown command 0x{c:04x}"),
            Self::SvgParse           => f.write_str("SVG parse failed"),
            Self::LatexCompile       => f.write_str("LaTeX compilation failed"),
            Self::MermaidCompile     => f.write_str("Mermaid compilation failed"),
            Self::PngDecode          => f.write_str("PNG decode failed"),
            Self::Lz4Decompress      => f.write_str("LZ4 decompression failed"),
            Self::CacheCorrupt       => f.write_str("cache blob corrupt"),
            Self::ImageTooLarge { w, h } => write!(f, "image {w}x{h} exceeds limit"),
            Self::JsonNav            => f.write_str("JSON navigation failed"),
            Self::ShmExhausted       => f.write_str("SHM name collision (4 retries)"),
            Self::TooManyClients     => f.write_str("max 32 concurrent clients"),
        }
    }
}

impl core::error::Error for Error {}

impl From<rustix::io::Errno> for Error {
    #[inline]
    fn from(e: rustix::io::Errno) -> Self { Self::Io(e) }
}
```

> [!IMPORTANT] Compile-time lock: `size_of::<Error>() <= 12`. `ImageTooLarge { w, h }` is the widest
> variant (8 bytes payload + 4 bytes discriminant). No `Box<dyn>`, no `String` payloads.

### 2.2 `ir.rs` — `Span` (16 B), `Kind` (4-bit), `Scratch`

```rust
use crate::ipynb::Cell;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    MathDisplay = 0,  MathInline  = 1,  Mermaid     = 2,  RawSvg      = 3,
    VegaLite    = 4,  Plotly      = 5,  CsvTsv      = 6,  Dot         = 7,
    Quarto      = 8,  QmdCallout  = 9,  NotebookOut = 10, PipeTable   = 11,
    Chem        = 12, TypstMath   = 13,
}

impl Kind {
    #[inline]
    pub const fn from_u4(v: u8) -> Self {
        const TABLE: [Kind; 16] = [
            Kind::MathDisplay, Kind::MathInline, Kind::Mermaid, Kind::RawSvg,
            Kind::VegaLite, Kind::Plotly, Kind::CsvTsv, Kind::Dot,
            Kind::Quarto, Kind::QmdCallout, Kind::NotebookOut, Kind::PipeTable,
            Kind::Chem, Kind::TypstMath,
            Kind::MathDisplay, Kind::MathDisplay, // sentinels (14-15 unreachable)
        ];
        TABLE[(v & 0x0F) as usize]
    }
}

/// 16-byte lexer token. 4 per 64 B x86 cacheline, 8 per 128 B aarch64.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Span {
    pub outer_start:          u32,
    pub outer_end:            u32,
    pub payload_start:        u32,
    pub payload_end_and_kind: u32, // lower 28: offset, upper 4: Kind
}

const _: () = assert!(size_of::<Span>() == 16);
const _: () = assert!(align_of::<Span>() == 4);

pub const OFFSET_MASK: u32 = 0x0FFF_FFFF;
pub const KIND_SHIFT: u32  = 28;

impl Span {
    #[inline]
    pub const fn new(
        outer_start: u32, outer_end: u32,
        payload_start: u32, payload_end: u32, kind: Kind,
    ) -> Self {
        Self {
            outer_start, outer_end, payload_start,
            payload_end_and_kind: (payload_end & OFFSET_MASK) | ((kind as u32) << KIND_SHIFT),
        }
    }

    #[inline] pub const fn payload_end(self) -> u32 { self.payload_end_and_kind & OFFSET_MASK }
    #[inline] pub const fn kind(self) -> Kind { Kind::from_u4((self.payload_end_and_kind >> KIND_SHIFT) as u8) }
    #[inline] pub fn payload<'a>(&self, src: &'a [u8]) -> &'a [u8] { &src[self.payload_start as usize..self.payload_end() as usize] }
    #[inline] pub fn outer<'a>(&self, src: &'a [u8]) -> &'a [u8] { &src[self.outer_start as usize..self.outer_end as usize] }
}
```

**Scratch arena** — disjoint frame-local working buffers, zero steady-state allocation:

```rust
pub struct Scratch {
    pub spans:    Vec<Span>,
    pub apc_buf:  Vec<u8>,     // KGP APC assembly
    pub text_buf: Vec<u8>,     // styled text output
    pub svg_buf:  String,      // SVG generation target
    pub aux_buf:  Vec<u8>,     // JSON unescape / chem LaTeX / SVG rewrite staging
    pub rgba_buf: Vec<u8>,     // rasterization target
    pub lz4_buf:  Vec<u8>,     // LZ4 compress/decompress
    pub b64_buf:  Vec<u8>,     // Base64 encoding
    pub line_buf: Vec<u8>,     // line accumulation (stream mode, NOT cleared per-frame)
    pub cells:    Vec<Cell>,   // notebook cell descriptors
}

impl Scratch {
    pub fn new() -> Self {
        Self {
            spans:    Vec::with_capacity(256),
            apc_buf:  Vec::with_capacity(1 << 19),  // 512 KiB
            text_buf: Vec::with_capacity(1 << 16),  // 64 KiB
            svg_buf:  String::with_capacity(1 << 16),
            aux_buf:  Vec::with_capacity(1 << 16),
            rgba_buf: Vec::with_capacity(1 << 19),
            lz4_buf:  Vec::with_capacity(1 << 16),
            b64_buf:  Vec::with_capacity(1 << 12),  // 4 KiB
            line_buf: Vec::with_capacity(1 << 14),
            cells:    Vec::with_capacity(64),
        }
    }

    /// Reset all cursors. Preserves capacity. `line_buf` persists (stream accumulation).
    #[inline]
    pub fn reset_frame(&mut self) {
        self.spans.clear();
        self.apc_buf.clear();
        self.text_buf.clear();
        self.svg_buf.clear();
        self.aux_buf.clear();
        self.rgba_buf.clear();
        self.lz4_buf.clear();
        self.b64_buf.clear();
        self.cells.clear();
    }

    /// In-place high-watermark reclamation. No double-allocation peak spike.
    pub fn shrink_if_bloated(&mut self) {
        const RGBA_HWM: usize  = 4 << 20;   // 4 MiB
        const TEXT_HWM: usize  = 1 << 20;   // 1 MiB
        const RGBA_BASE: usize = 1 << 19;   // 512 KiB
        const TEXT_BASE: usize = 1 << 16;   // 64 KiB

        if self.rgba_buf.capacity() > RGBA_HWM {
            self.rgba_buf.clear();
            self.rgba_buf.shrink_to(RGBA_BASE);
        }
        if self.svg_buf.capacity() > TEXT_HWM {
            self.svg_buf.clear();
            self.svg_buf.shrink_to(TEXT_BASE);
        }
        if self.aux_buf.capacity() > TEXT_HWM {
            self.aux_buf.clear();
            self.aux_buf.shrink_to(TEXT_BASE);
        }
    }
}
```

> [!NOTE] `shrink_if_bloated` uses `.clear()` + `.shrink_to()` — in-place realloc, not
> `= Vec::with_capacity()` which allocates before freeing (peak = old + new).

### 2.3 `vram.rs` — 4-Way Set-Associative L0 + L1 + wymix

**Tree-PLRU primitives** (shared by L0 and L1):

```rust
/// Update 3-bit Tree-PLRU state after accessing `way ∈ 0..4`.
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
```

**L0 VRAM Tag Table** (4 160 bytes, fits L1d):

```rust
#[derive(Clone, Copy, Default)]
#[repr(C)]
pub struct Payload {
    pub image_id: u32,  // 24-bit non-zero KGP ID
    pub cols:     u16,
    pub rows:     u16,
}
const _: () = assert!(size_of::<Payload>() == 8);

#[derive(Clone, Copy)]
#[repr(C, align(64))]
pub struct Set {
    pub tags:    [u64; 4],
    pub entries: [Payload; 4],
}
const _: () = assert!(size_of::<Set>() == 64);
const _: () = assert!(align_of::<Set>() == 64);

#[repr(C, align(64))]
pub struct Table {
    sets: [Set; 64],
    plru: [u8; 64],
}
const _: () = assert!(size_of::<Table>() == 4160);

impl Table {
    pub const fn new() -> Self { /* zero-init sets + plru */ }

    #[inline]
    pub fn lookup(&mut self, tag: u64) -> Option<Payload> {
        let idx = (tag as usize) & 63;
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
        let idx = (tag as usize) & 63;
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
        for set in &mut self.sets { set.tags = [0; 4]; }
    }
}
```

**L1 RGBA Cache** (16-set × 4-way, eliminates birthday-paradox thrashing):

```rust
pub struct Replica {
    pub cols: u16, pub rows: u16,
    pub w: u16,    pub h: u16,
    pub rgba: Vec<u8>,  // capacity preserved across frames
}

pub struct Cache {
    tags: [[u64; 4]; 16],
    plru: [u8; 16],
    ways: Box<[[Replica; 4]; 16]>,
}

impl Cache {
    #[inline]
    pub fn lookup(&mut self, tag: u64) -> Option<&Replica> {
        let idx = ((tag >> 6) as usize) & 15; // decorrelated from L0 lower-6-bit index
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
        let idx = ((tag >> 6) as usize) & 15;
        let way = (0..4)
            .find(|&w| self.tags[idx][w] == tag || self.tags[idx][w] == 0)
            .unwrap_or_else(|| plru_victim(self.plru[idx]));
        self.tags[idx][way] = tag;
        self.plru[idx] = plru_touch(self.plru[idx], way);
        let slot = &mut self.ways[idx][way];
        slot.cols = cols; slot.rows = rows; slot.w = w; slot.h = h;
        slot.rgba.clear();
        slot.rgba.extend_from_slice(rgba);
    }
}
```

**wymix hash + canonical image_id**:

```rust
/// 64-bit content+geometry hash. Folds cell_w, cell_h, cols, kind, fg_rgb, payload.
/// Guaranteed non-zero (0 = empty sentinel).
#[inline]
pub fn compute_tag(
    cell_w: u16, cell_h: u16, cols: u16, kind: u8,
    fg_rgb: [u8; 3], payload: &[u8],
) -> u64 {
    const P0: u64 = 0xa076_1d64_78bd_642f;
    const P1: u64 = 0xe703_7ed1_a0b4_28db;

    #[inline]
    const fn wymix(a: u64, b: u64) -> u64 {
        let r = (a as u128).wrapping_mul(b as u128);
        (r as u64) ^ ((r >> 64) as u64)
    }

    let meta = (cell_w as u64)
        | ((cell_h as u64) << 16)
        | ((cols as u64) << 32)
        | ((kind as u64) << 48)
        | ((fg_rgb[0] as u64) << 56);
    let mut seed = wymix(meta ^ P0, ((fg_rgb[1] as u64) << 8 | fg_rgb[2] as u64) ^ P1);

    let mut i = 0;
    while i + 8 <= payload.len() {
        let v = u64::from_le_bytes([
            payload[i], payload[i+1], payload[i+2], payload[i+3],
            payload[i+4], payload[i+5], payload[i+6], payload[i+7],
        ]);
        seed = wymix(seed ^ P0, v ^ P1);
        i += 8;
    }
    if i < payload.len() {
        let mut tail = 0u64;
        let mut shift = 0;
        while i < payload.len() { tail |= (payload[i] as u64) << shift; shift += 8; i += 1; }
        seed = wymix(seed ^ P0, tail ^ P1);
    }
    wymix(seed ^ (payload.len() as u64), P1).max(1)
}

/// Canonical 24-bit KGP image_id from wymix tag. Single derivation across all tiers.
#[inline]
pub const fn image_id_from_tag(tag: u64) -> u32 {
    let id = ((tag ^ (tag >> 24) ^ (tag >> 48)) as u32) & 0x00FF_FFFF;
    if id == 0 { 1 } else { id }
}
```

> [!IMPORTANT] **No `image_id_from_key`.** One identity derivation. BLAKE3 is for L2 disk addressing
> only. KGP `image_id` is always `image_id_from_tag(wymix_tag)`.

### 2.4 `tty.rs` + `cfg.rs` — Geometry & Config

```rust
// tty.rs
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Metrics {
    pub cols: u16, pub rows: u16,
    pub cell_w: u16, pub cell_h: u16,
    pub px_w: u16, pub px_h: u16,
    pub is_remote: u8, pub is_tmux: u8,
    pub _pad: [u8; 2],
}
const _: () = assert!(size_of::<Metrics>() == 16);
const _: () = assert!(align_of::<Metrics>() == 2);

impl Metrics {
    pub const FALLBACK: Self = Self {
        cols: 80, rows: 24, cell_w: 8, cell_h: 16,
        px_w: 640, px_h: 384, is_remote: 0, is_tmux: 0, _pad: [0; 2],
    };
}
```

```rust
// cfg.rs
use rustix::fd::OwnedFd;
use crate::{theme::Palette, tty::Metrics};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Viewport {
    pub skip_lines: u16, pub max_lines: u16,
    pub width_cells: u16, pub height_cells: u16,
}
const _: () = assert!(size_of::<Viewport>() == 8);

pub struct Cfg {
    pub tty: Metrics,
    pub viewport: Viewport,
    pub palette: &'static Palette,
    pub cache_dir: Option<OwnedFd>,
    pub is_stdout_tty: bool,
}

impl Cfg {
    #[inline]
    pub const fn max_width_px(&self) -> u16 {
        self.tty.cols.saturating_mul(self.tty.cell_w)
    }
}
```

### 2.5 `theme.rs` — Compile-Time Token (32 B) + Palette

```rust
#[derive(Clone, Copy, Debug)]
#[repr(C)]
pub struct Token {
    pub hex:     [u8; 7],   // "#RRGGBB"
    pub hex_len: u8,        // 7
    pub sgr:     [u8; 20],  // "\x1b[38;2;R;G;Bm"
    pub sgr_len: u8,
    pub rgb:     [u8; 3],   // raw [R, G, B] for wymix tag folding
}
const _: () = assert!(size_of::<Token>() == 32);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Idx {
    Fg = 0, FgMuted, Bg, BgSurface, Heading, HeadingSub,
    Link, Code, Error, GridLine,
    Series0, Series1, Series2, Series3, Series4, Series5, Series6, Series7,
}

pub struct Palette {
    pub tokens: [Token; 18],
    pub is_dark: bool,
}

impl Palette {
    #[inline]
    pub const fn get(&self, idx: Idx) -> &Token { &self.tokens[idx as usize] }
}

pub const fn token_from_rgb(r: u8, g: u8, b: u8) -> Token { /* compile-time SGR + hex */ }

pub static DARK:  Palette = { /* Catppuccin Mocha, const-constructed */ };
pub static LIGHT: Palette = { /* Catppuccin Latte, const-constructed */ };
```

### 2.6 `ipc.rs` — Alignment-Safe Wire Codec (IXPC v1)

```rust
pub const MAGIC: u32         = 0x4958_5043; // "IXPC"
pub const VERSION: u16       = 1;
pub const MAX_PAYLOAD: u32   = 0x0400_0000; // 64 MiB
pub const MAX_CLIENTS: usize = 32;
pub const RESP_FLAG: u16     = 0x8000;
pub const ERR_FLAG: u16      = 0x4000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u16)]
pub enum Cmd {
    Ping = 0x0001, ComposeAnsi = 0x0002,
    ComposeExtmarks = 0x0003, RenderFile = 0x0004, Shutdown = 0x0006,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Header {
    pub magic: u32, pub version: u16, pub command: u16,
    pub flags: u32, pub cols: u16, pub rows: u16,
    pub cell_w: u16, pub cell_h: u16,
    pub payload_len: u32, pub _reserved: u64,
}
const _: () = assert!(size_of::<Header>() == 32);
const _: () = assert!(align_of::<Header>() == 8);

impl Header {
    /// Explicit byte-slice codec. No transmute, no alignment UB.
    #[inline]
    pub const fn from_le_bytes(b: &[u8; 32]) -> Self {
        Self {
            magic:       u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
            version:     u16::from_le_bytes([b[4], b[5]]),
            command:     u16::from_le_bytes([b[6], b[7]]),
            flags:       u32::from_le_bytes([b[8], b[9], b[10], b[11]]),
            cols:        u16::from_le_bytes([b[12], b[13]]),
            rows:        u16::from_le_bytes([b[14], b[15]]),
            cell_w:      u16::from_le_bytes([b[16], b[17]]),
            cell_h:      u16::from_le_bytes([b[18], b[19]]),
            payload_len: u32::from_le_bytes([b[20], b[21], b[22], b[23]]),
            _reserved:   u64::from_le_bytes([b[24],b[25],b[26],b[27],b[28],b[29],b[30],b[31]]),
        }
    }

    #[inline]
    pub const fn to_le_bytes(&self) -> [u8; 32] { /* symmetric encoding */ }

    pub fn validate(&self) -> Result<Cmd, Error> { /* magic, version, payload_len, command */ }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(C)]
pub struct Extmark {
    pub image_id: u32,
    pub start_row: u16, pub start_col: u16,
    pub end_row: u16,   pub end_col: u16,
    pub cols: u16,       pub rows: u16,
}
const _: () = assert!(size_of::<Extmark>() == 16);
const _: () = assert!(align_of::<Extmark>() == 4);
```

### 2.7 `cache.rs` — PX02 Disk Cache Header (12 B)

```rust
pub const MAGIC: [u8; 4] = *b"PX02";

#[derive(Clone, Copy)]
#[repr(C)]
pub struct Header {
    pub magic: [u8; 4],
    pub width: u32,
    pub height: u32,
}
const _: () = assert!(size_of::<Header>() == 12);

pub const MAX_DECODE_SIZE: usize = 67_108_864; // 64 MiB
```

### 2.8 `ipynb.rs` — Cell Descriptor (32 B)

```rust
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum CellKind { Code = 0, Markdown = 1, Raw = 2 }

#[derive(Clone, Copy)]
#[repr(C)]
pub struct Cell {
    pub source_start: u32,  pub source_end: u32,
    pub outputs_start: u32, pub outputs_end: u32,
    pub cell_type: u32,     pub execution_count: u32,
    pub cell_index: u32,    pub output_count: u32,
}
const _: () = assert!(size_of::<Cell>() == 32);
```

---

## §3 Module Boundaries — Public Signatures

Each module: **one primary `pub fn`**, explicit ownership contract.

### 3.1 `scan` — Single-Pass O(N) Lexer

```rust
/// Single-pass SWAR/memchr scanner.
/// Uses memchr3(b'\n', b'$', b'\\') + memchr2(b'`', b'<') dispatch.
/// Emits strictly monotonic, non-overlapping Spans. No dedup. No sorting.
pub fn scan(src: &[u8], out: &mut Vec<Span>);
```

> [!CAUTION] Original ARCHITECTURE.md specified 7 sub-scanners + O(N log N) dedup. This is O(N)
> single-pass.

### 3.2 `json` — Zero-Alloc Navigator (Sound Lifetimes)

```rust
#[derive(Clone, Copy)]
pub struct Nav<'a> { src: &'a [u8], pos: u32, end: u32 }

impl<'a> Nav<'a> {
    pub const fn new(src: &'a [u8]) -> Self;
    pub fn get(&self, key: &[u8]) -> Option<Nav<'a>>;
    pub fn items(&self) -> Items<'a>;

    /// No-escape fast path: returns &'a [u8] directly from source.
    /// Escape path: unescapes into scratch, returns &'b [u8] tied to scratch lifetime.
    pub fn str_of<'b>(&self, scratch: &'b mut Vec<u8>) -> Option<&'b [u8]>
    where 'a: 'b;

    pub fn str_raw(&self) -> Option<&'a [u8]>;
    pub fn num_of(&self) -> Option<f64>;
    pub fn int_of(&self) -> Option<i64>;
}
```

> [!IMPORTANT] Original `str_of` returned `Option<&'a [u8]>` while writing into `scratch` — lifetime
> unsound (E0515). Fixed: returns `&'b` tied to `scratch`.

### 3.3 `heal` — Streaming LaTeX Healer

```rust
pub struct State { brace_depth: u32, env_stack: Vec<u8>, delim_stack: Vec<u8> }

impl State {
    pub const fn new() -> Self;
    pub fn heal(&mut self, fragment: &[u8], out: &mut Vec<u8>);
    pub fn reset(&mut self);
}
```

### 3.4 `chem` — mhchem Transpiler

```rust
/// \ce{H2O} → \text{H}_{2}\text{O}. Pure. No alloc beyond `out`.
pub fn transpile(input: &[u8], out: &mut Vec<u8>);
```

### 3.5 `chart` — Vega-Lite & Plotly → SVG

```rust
pub fn vegalite(nav: Nav<'_>, palette: &Palette, width: u16, out: &mut String);
pub fn plotly(nav: Nav<'_>, palette: &Palette, width: u16, out: &mut String);
```

### 3.6 `dot` — Graphviz DOT → SVG

```rust
pub fn render(src: &[u8], palette: &Palette, width: u16, out: &mut String);
```

### 3.7 `csv` — Tabular → SVG

```rust
pub fn render_csv(src: &[u8], palette: &Palette, width: u16, out: &mut String);
pub fn render_pipe_table(src: &[u8], palette: &Palette, width: u16, out: &mut String);
```

### 3.8 `ipynb` — Notebook Cell Extraction

```rust
pub fn extract_cells<'a>(src: &'a [u8], cells: &mut Vec<Cell>) -> Result<Nav<'a>, Error>;
pub fn resolve_output<'a>(nav: Nav<'a>, src: &'a [u8]) -> Option<(Kind, &'a [u8])>;
```

### 3.9 `svg` — Single-Pass Streaming Color Rewriter

```rust
/// Streams from src → dst. O(N), zero in-place memmove.
/// Uses memchr3(b'#', b'c', b'r') for #000, currentColor, rgb() substitution.
pub fn rewrite_colors(src: &[u8], palette: &Palette, dst: &mut Vec<u8>);
```

> [!CAUTION] Original `rewrite_colors(buf: &mut Vec<u8>)` was in-place with O(N²) memmove for
> length-changing substitutions. This is `src → dst` streaming in O(N).

### 3.10 `raster` — SVG → RGBA

```rust
pub fn rasterize(
    svg: &[u8], cell_w: u16, cell_h: u16, max_width_px: u16,
    rgba: &mut Vec<u8>,
) -> Result<(u16, u16, u16, u16), Error>; // (w_px, h_px, cols, rows)

pub fn decode_png(src: &[u8], rgba: &mut Vec<u8>) -> Result<(u32, u32), Error>;
pub fn unpremultiply(rgba: &mut [u8]); // Q24 fixed-point, in-place
```

### 3.11 `cache` — BLAKE3 L2 Disk Cache

```rust
pub fn blake3_key(cell_w: u16, cell_h: u16, max_w: u16, is_dark: bool, content: &[u8]) -> [u8; 32];
pub fn hex_encode(hash: &[u8; 32], out: &mut [u8; 64]);
pub fn read_blob(dir_fd: BorrowedFd<'_>, key: &[u8; 32], rgba: &mut Vec<u8>, lz4: &mut Vec<u8>) -> Result<Option<(u32, u32)>, Error>;
pub fn write_blob(dir_fd: BorrowedFd<'_>, key: &[u8; 32], w: u32, h: u32, rgba: &[u8], lz4: &mut Vec<u8>) -> Result<(), Error>;
pub fn ensure_dirs() -> Result<OwnedFd, Error>;
```

### 3.12 `kgp` — Kitty Graphics Protocol Encoder

```rust
pub fn stage_shm(
    rgba: &[u8], w: u16, h: u16, image_id: u32,
    cols: u16, rows: u16, is_tmux: bool,
    seq: &mut u32, pid: u32, apc: &mut Vec<u8>,
) -> Result<(), Error>;

pub fn stage_base64(
    rgba: &[u8], w: u16, h: u16, image_id: u32,
    cols: u16, rows: u16, is_tmux: bool,
    apc: &mut Vec<u8>, b64: &mut Vec<u8>,
);

pub fn emit_placeholder(image_id: u32, cols: u16, rows: u16, text: &mut Vec<u8>);
```

### 3.13 `tty` — Terminal Probing (I/O Boundary)

```rust
pub fn probe() -> (Metrics, &'static Palette, Option<OwnedFd>);
fn query_bg(fd: BorrowedFd<'_>) -> Option<(u8, u8, u8)>;
fn query_fg(fd: BorrowedFd<'_>) -> Option<(u8, u8, u8)>;
fn luminance(r: u8, g: u8, b: u8) -> u8; // Rec. 709, integer-scaled
```

### 3.14 `style` — Markdown → ANSI/SGR

```rust
pub fn render(
    src: &[u8], spans: &[Span], cols: u16, palette: &Palette, out: &mut Vec<u8>,
);
```

### 3.15 `ipc` — Wire Codec Helpers

```rust
pub fn encode_dual_response(cmd: Cmd, apc: &[u8], text: &[u8], out: &mut Vec<u8>);
pub fn encode_error(cmd: Cmd, msg: &str, out: &mut Vec<u8>);
pub fn encode_extmarks(records: &[Extmark], out: &mut Vec<u8>);
```

### 3.16 `daemon` — Non-Blocking Poll Reactor

```rust
pub enum Phase {
    ReadHeader { got: u8 },
    ReadPayload { header: Header, got: u32 },
    WriteResp { sent: usize },
}

pub struct ClientSlot {
    pub fd: Option<OwnedFd>,
    pub phase: Phase,
    pub hdr: [u8; 32],
    pub buf: Vec<u8>,
}

pub fn run(cfg: Cfg, listen_fd: OwnedFd) -> Result<(), Error>;
pub fn bind_socket() -> Result<OwnedFd, Error>;
pub fn acquire_lock() -> Result<OwnedFd, Error>;
```

> [!WARNING] Original daemon used blocking reads — single slow client stalls reactor. Fixed:
> per-client `Phase` FSM with non-blocking `read`/`write` and `poll(2)`.

### 3.17 `compose` — Frame Orchestrator + Format Dispatch

```rust
pub struct Session {
    pub vram:       vram::Table,
    pub l1:         vram::Cache,
    pub heal:       heal::State,
    pub shm_seq:    u32,
    pub pid:        u32,
    pub request_id: u64,
}

/// Disjoint context passed through FORMAT_TABLE to every frontend.
pub struct CompileCtx<'a> {
    pub palette: &'a Palette,
    pub width:   u16,
    pub heal:    &'a mut heal::State,
    pub svg:     &'a mut String,
    pub aux:     &'a mut Vec<u8>,
}

type CompileFn = fn(src: &[u8], ctx: &mut CompileCtx<'_>) -> Result<(), Error>;

static FORMAT_TABLE: [CompileFn; 14] = [
    compile_math_display,  // 0: MathDisplay
    compile_math_inline,   // 1: MathInline
    compile_mermaid,       // 2: Mermaid
    compile_raw_svg,       // 3: RawSvg
    compile_vegalite,      // 4: VegaLite
    compile_plotly,        // 5: Plotly
    compile_csv,           // 6: CsvTsv
    compile_dot,           // 7: Dot
    compile_quarto,        // 8: Quarto
    compile_callout,       // 9: QmdCallout
    compile_notebook_out,  // 10: NotebookOut
    compile_pipe_table,    // 11: PipeTable
    compile_chem,          // 12: Chem
    compile_typst_math,    // 13: TypstMath
];

/// O(1) dispatch via Kind discriminant as array index.
#[inline]
pub fn dispatch(span: Span, src: &[u8], ctx: &mut CompileCtx<'_>) -> Result<(), Error> {
    FORMAT_TABLE[span.kind() as usize](span.payload(src), ctx)
}

pub fn compose_ansi(src: &[u8], cfg: &Cfg, session: &mut Session, scratch: &mut Scratch) -> Result<(), Error>;
pub fn compose_extmarks(src: &[u8], cfg: &Cfg, session: &mut Session, scratch: &mut Scratch) -> Result<(), Error>;
```

> [!IMPORTANT] Original `FormatDesc.compile` took `(src, palette, width, svg)` — insufficient for
> `Chem` (needs `aux_buf`), `VegaLite`/`Plotly` (needs JSON scratch), `Heal` (needs `&mut State`).
> Fixed: unified `CompileCtx` eliminates hidden heap allocations.

### 3.18 `font` — Font Specimen Card

```rust
pub fn specimen(src: &[u8], palette: &Palette, width: u16, out: &mut String) -> Result<(), Error>;
```

### 3.19 `main` — Entry Point

```rust
fn main() {
    // 1. SIG_IGN on SIGPIPE
    // 2. Hand-rolled CLI (no clap)
    // 3. Dispatch: --daemon | --ping | --shutdown | stdin pipe | file arg
    // 4. Exit 0 / 1
}
```

---

## §4 Module Dependency Graph

```
                      ┌──────────┐
                      │  main.rs │
                      └────┬─────┘
                           │
              ┌────────────┼────────────┐
              ▼            ▼            ▼
         ┌─────────┐ ┌─────────┐ ┌──────────┐
         │ compose  │ │ daemon  │ │  (CLI)   │
         └────┬─────┘ └────┬────┘ └──────────┘
              │            │
    ┌─────────┼────────────┤
    │         │            │
    ▼         ▼            ▼
┌──────┐  ┌──────┐    ┌──────┐
│ scan │  │ style│    │ ipc  │         ◄── Pure logic
└──┬───┘  └──┬───┘    └──────┘
   │         │
   ▼         ▼
┌──────────────────────────────────┐
│          ir (Span, Scratch)      │    ◄── Shared types
└──────────────────────────────────┘
   │    │    │    │    │    │    │
   ▼    ▼    ▼    ▼    ▼    ▼    ▼
┌────┐┌────┐┌───┐┌───┐┌────┐┌────┐┌─────┐
│heal││chem││dot││csv││chart││ipynb││font │  ◄── Content frontends
└────┘└────┘└───┘└───┘└────┘└──┬─┘└─────┘
                               │
                            ┌──▼──┐
                            │json │          ◄── Zero-serde navigator
                            └─────┘

   ▼         ▼         ▼
┌──────┐  ┌──────┐  ┌──────┐
│ svg  │  │raster│  │ kgp  │                 ◄── Render backend
└──────┘  └──────┘  └──┬───┘
                       │
              ┌────────┼────────┐
              ▼        ▼        ▼
          ┌──────┐ ┌──────┐ ┌──────┐
          │ vram │ │cache │ │ tty  │         ◄── State + I/O boundary
          └──────┘ └──────┘ └──────┘

              ▼        ▼
          ┌──────┐ ┌──────┐
          │theme │ │ err  │                  ◄── Foundation (zero deps)
          └──────┘ └──────┘
          ┌──────┐
          │ cfg  │
          └──────┘
```

**Invariant:** arrows flow downward only. No cycles. `scan`, `heal`, `chem`, `chart`, `dot`, `csv`,
`json`, `style`, `svg` — 100% pure, deterministic, Miri-safe. `tty`, `cache`, `kgp`, `daemon` — I/O
boundary, `rustix`-based.

---

## §5 `Cargo.toml`

```toml
[package]
name = "ii"
version = "0.0.0"
edition = "2024"
rust-version = "1.99"
resolver = "3"

[dependencies]
resvg      = { version = "0.50", default-features = false }
tiny-skia  = "0.11"
blake3     = "1"
lz4_flex   = { version = "0.11", default-features = false, features = ["frame"] }
memchr     = "2"
rustix     = { version = "1", features = ["fs", "mm", "net", "event", "process", "termios", "shm"] }
ratex-parser = "0.1"
ratex-layout = "0.1"
ratex-svg    = "0.1"
mermaid-svg  = "0.1"
png        = "0.17"

[dev-dependencies]
proptest = "1"
divan    = "0.1"

[profile.release]
opt-level     = 3
codegen-units = 1
incremental   = false
lto           = "fat"
panic         = "abort"
strip         = "symbols"

[lints.rust]
unsafe_op_in_unsafe_fn       = "deny"
unfulfilled_lint_expectations = "warn"
unreachable_pub              = "warn"
unused_qualifications        = "warn"
unused_lifetimes             = "warn"
trivial_casts                = "warn"
trivial_numeric_casts        = "warn"

[lints.clippy]
all                             = { level = "deny", priority = -1 }
pedantic                        = { level = "warn", priority = -1 }
cargo                           = { level = "warn", priority = -1 }
multiple_unsafe_ops_per_block   = "deny"
allow_attributes                = "deny"
allow_attributes_without_reason = "deny"
undocumented_unsafe_blocks      = "deny"
unwrap_used                     = "deny"
expect_used                     = "deny"
panic                           = "deny"
todo                            = "deny"
dbg_macro                       = "deny"
unimplemented                   = "deny"
print_stdout                    = "deny"
print_stderr                    = "deny"
inline_always                   = "warn"
needless_pass_by_value          = "warn"
```

> [!NOTE] `clippy::nursery` is `warn`-level in ARCHITECTURE.md but **not denied in CI** per Core
> Directive §3 (unstable lints break CI unpredictably). Removed from this manifest.

---

## §6 Disjoint Borrow Architecture

```
┌───────────────────────────────────────────────────────────────┐
│  compose_ansi(                                                │
│    src: &[u8],              ← borrowed, immutable             │
│    cfg: &Cfg,               ← & (immutable session config)    │
│    session: &mut Session,   ← &mut (VRAM, L1, healer, seq)   │
│    scratch: &mut Scratch,   ← &mut (frame-local arena)       │
│  )                                                            │
└───────────────────────────────────────────────────────────────┘

Cfg     = { Metrics, Viewport, &'static Palette, cache_dir }
Session = { vram::Table, vram::Cache, heal::State, shm_seq, pid, request_id }
Scratch = { Vec<Span>, Vec<u8> × 7, String, Vec<Cell> }

Borrow map:
  cfg.palette      → chart, dot, csv, style, svg (read)
  session.vram     → compose (read + write)
  session.l1       → compose (read + write)
  session.heal     → compose stream mode (write via CompileCtx)
  scratch.spans    → scan (write), compose (read)
  scratch.svg_buf  → chart/dot/csv (write via CompileCtx), raster (read)
  scratch.aux_buf  → chem/json/svg rewrite (write via CompileCtx)
  scratch.rgba_buf → raster (write), kgp/cache (read)
  scratch.apc_buf  → kgp (write), compose (read → write to tty/socket)
  scratch.text_buf → style (write), compose (read → write to tty/socket)
```

---

## §7 Build Plan — Implementation Order

### Phase 1: Foundation (zero I/O, zero external deps)

| Step | Module            | Gate                                                                     |
| ---- | ----------------- | ------------------------------------------------------------------------ |
| 1.1  | `err.rs`          | `Display` all variants. `size_of::<Error>() <= 12`.                      |
| 1.2  | `cfg.rs`          | `size_of` assertions. `Metrics::FALLBACK`.                               |
| 1.3  | `theme.rs`        | `const fn token_from_rgb` roundtrip. `size_of::<Token>() == 32`.         |
| 1.4  | `ir.rs` (types)   | `Span` pack/unpack. `Kind::from_u4` exhaustive.                          |
| 1.5  | `ir.rs` (Scratch) | `reset_frame` preserves capacity. `shrink_if_bloated` reclaims in-place. |

**Gate:** `cargo test` + `cargo clippy` + Miri on layouts.

### Phase 2: Pure Parsers (deterministic, Miri-safe)

| Step | Module    | Gate                                                                  |
| ---- | --------- | --------------------------------------------------------------------- |
| 2.1  | `json.rs` | Navigate nested JSON. `str_of` lifetime correctness. `proptest` fuzz. |
| 2.2  | `scan.rs` | No panic on arbitrary `&[u8]`. Monotonic spans. Known fixtures.       |
| 2.3  | `heal.rs` | Cross-chunk brace/env/delim balancing.                                |
| 2.4  | `chem.rs` | `\ce{H2O}` corpus.                                                    |

**Gate:** Miri Tree Borrows + `proptest` on `scan` + `json`.

### Phase 3: Content Frontends (pure)

| Step | Module     | Gate                                |
| ---- | ---------- | ----------------------------------- |
| 3.1  | `chart.rs` | Vega-Lite bar/line/pie → valid SVG. |
| 3.2  | `dot.rs`   | DAG → SVG rank ordering.            |
| 3.3  | `csv.rs`   | CSV/TSV/pipe → SVG table.           |
| 3.4  | `ipynb.rs` | Cell extraction. MIME priority.     |
| 3.5  | `font.rs`  | Specimen card.                      |
| 3.6  | `svg.rs`   | Streaming rewrite: `src → dst`.     |
| 3.7  | `style.rs` | SGR output. CJK wrap.               |

**Gate:** Visual diff against golden SVG fixtures.

### Phase 4: Cache & VRAM

| Step | Module            | Gate                                        |
| ---- | ----------------- | ------------------------------------------- |
| 4.1  | `vram.rs` (Table) | Distribution test, PLRU eviction.           |
| 4.2  | `vram.rs` (Cache) | 4-way insert/lookup, capacity preservation. |

### Phase 5: Render Backend

| Step | Module      | Gate                                  |
| ---- | ----------- | ------------------------------------- |
| 5.1  | `raster.rs` | Cell-snap. Unpremultiply. PNG decode. |
| 5.2  | `kgp.rs`    | APC format. Base64 chunks. SHM names. |

### Phase 6: I/O Boundary

| Step | Module     | Gate                                                       |
| ---- | ---------- | ---------------------------------------------------------- |
| 6.1  | `tty.rs`   | OSC 11 parse. Luminance.                                   |
| 6.2  | `cache.rs` | Atomic write/read roundtrip. Corrupt detection. ASan/LSan. |

### Phase 7: Orchestration

| Step | Module       | Gate                                        |
| ---- | ------------ | ------------------------------------------- |
| 7.1  | `compose.rs` | Full pipeline `.md` → APC + text. L0→L1→L2. |
| 7.2  | `ipc.rs`     | Header encode/decode. Dual-channel.         |
| 7.3  | `daemon.rs`  | PING roundtrip. SHUTDOWN. Non-blocking FSM. |
| 7.4  | `main.rs`    | CLI dispatch. Stream mode.                  |

### Phase 8: Hardening

| Task | Description                                                                           |
| ---- | ------------------------------------------------------------------------------------- |
| 8.1  | `cargo deny check` — no dupe crates, no `syn` v1+v2.                                  |
| 8.2  | Miri: `-Zmiri-tree-borrows -Zmiri-strict-provenance -Zmiri-symbolic-alignment-check`. |
| 8.3  | `cargo-mutants` on `scan`, `heal`, `vram`, `cache`.                                   |
| 8.4  | Benchmark: 10 KiB MD + 20 LaTeX spans → <16 ms, 0 hot-path alloc.                     |
| 8.5  | Socket hardening: symlink check, UID check, 0700 perms.                               |

---

## §8 Defect Registry (Resolved from Argon Audit)

| #   | Defect                                                    | Resolution                                                   |
| --- | --------------------------------------------------------- | ------------------------------------------------------------ |
| 1   | L1 Ring 1-way direct-mapped → birthday-paradox thrashing  | 16-set × 4-way `vram::Cache` with Tree-PLRU                  |
| 2   | `Table` + `plru` cross-cacheline access                   | `plru: [u8; 64]` packed after 64 × 64 B sets, same alignment |
| 3   | `Scratch::shrink_if_bloated` double-alloc peak            | `.clear()` + `.shrink_to()` in-place                         |
| 4   | 7-pass scan + O(N log N) dedup                            | True single-pass, monotonic spans, zero dedup                |
| 5   | Dual-hash (wymix + BLAKE3) + dual image_id                | Single wymix → `image_id_from_tag`. BLAKE3 on L1 miss only   |
| 6   | `cfg::TtyMetrics`, `ipc::ExtmarkRecord`, `vram::RingSlot` | `tty::Metrics`, `ipc::Extmark`, `vram::Replica`              |
| 7   | Unaligned wire transmute UB                               | Explicit `from_le_bytes` / `to_le_bytes`                     |
| 8   | `FORMAT_TABLE` missing scratch buffers                    | `CompileCtx` with `aux`, `heal`, `svg`                       |
| 9   | `FORMAT_TABLE` signature mismatch (compile error)         | Unified `CompileFn` type                                     |
| 10  | `json::Nav::str_of` lifetime unsound                      | `&'b [u8]` tied to `scratch` with `'a: 'b` bound             |
| 11  | `svg::rewrite_colors` O(N²) in-place memmove              | `src → dst` streaming rewrite                                |
| 12  | `[resolver] version = 3` invalid TOML                     | `resolver = "3"` under `[package]`                           |
| 13  | `Error` size unlocked                                     | `const _: () = assert!(size_of::<Error>() <= 12);`           |
| 14  | Daemon blocking reads → DoS                               | Per-client `Phase` FSM + non-blocking I/O                    |

---

_End of architecture._
