//! Ingest parsers: format adapters.
//!
//! Rule 1: Ingest != Render. All formats push into `crate::scene::Scene`.
//! Rule 2: Table-driven dispatch (`magic_bytes`, extensions, `parser_fn`).
//! Rule 3: `write_xml_escaped` writes to `&mut Vec<u8>` — zero heap in hot path.

pub mod chart;
pub mod chem;
pub mod csv;
pub mod dot;
pub mod font;
pub mod heal;
pub mod ipynb;
pub mod json;
pub mod md;
pub mod mermaid;
pub mod svg;
pub mod tex;

use crate::{err::Error, scene::Scene};

// ── XML escape ───────────────────────────────────────────────────────────────

pub trait XmlBuffer {
    fn extend_bytes(&mut self, b: &[u8]);
}

impl XmlBuffer for Vec<u8> {
    #[inline]
    fn extend_bytes(&mut self, b: &[u8]) { self.extend_from_slice(b); }
}

impl XmlBuffer for String {
    #[inline]
    fn extend_bytes(&mut self, b: &[u8]) {
        // SAFETY: callers only pass valid UTF-8 slices or ASCII XML entities.
        unsafe { self.as_mut_vec().extend_from_slice(b); }
    }
}

/// Write XML-escaped UTF-8 `bytes` into `out` with zero heap allocation.
/// Encodes only `& < > "` as required by XML 1.0 §2.4.
#[inline]
pub fn write_xml_escaped<B: XmlBuffer + ?Sized>(out: &mut B, bytes: &[u8]) {
    let mut pos = 0usize;
    while pos < bytes.len() {
        let b = bytes[pos];
        match b {
            b'&' => { out.extend_bytes(b"&amp;");  pos += 1; }
            b'<' => { out.extend_bytes(b"&lt;");   pos += 1; }
            b'>' => { out.extend_bytes(b"&gt;");   pos += 1; }
            b'"' => { out.extend_bytes(b"&quot;"); pos += 1; }
            _ => {
                let start = pos;
                pos += 1;
                while pos < bytes.len()
                    && !matches!(bytes[pos], b'&' | b'<' | b'>' | b'"')
                {
                    pos += 1;
                }
                out.extend_bytes(&bytes[start..pos]);
            }
        }
    }
}

/// Write XML-escaped `str` slice into `out`.
#[inline]
pub fn write_xml_escaped_str<B: XmlBuffer + ?Sized>(out: &mut B, s: &str) {
    write_xml_escaped(out, s.as_bytes());
}

// ── ASCII utilities ───────────────────────────────────────────────────────────

/// Trim ASCII whitespace from both ends of byte slice (const-compatible).
#[inline]
#[must_use]
pub const fn trim_ascii(mut b: &[u8]) -> &[u8] {
    while let Some((&first, rest)) = b.split_first() {
        if first.is_ascii_whitespace() {
            b = rest;
        } else {
            break;
        }
    }
    while let Some((&last, rest)) = b.split_last() {
        if last.is_ascii_whitespace() {
            b = rest;
        } else {
            break;
        }
    }
    b
}

// ── SVG integer/float emitters (zero fmt overhead) ───────────────────────────

/// Write a `u32` decimal into `out` without any heap allocation.
#[inline]
pub fn push_u32(out: &mut Vec<u8>, mut v: u32) {
    let mut buf = [0u8; 10];
    let mut i = 10usize;
    if v == 0 {
        out.push(b'0');
        return;
    }
    while v > 0 {
        i -= 1;
        buf[i] = b'0' + u8::try_from(v % 10).unwrap_or(0);
        v /= 10;
    }
    out.extend_from_slice(&buf[i..]);
}

/// Write an `f64` with one decimal place into `out` (no `core::fmt` overhead).
///
/// Precision adequate for SVG pixel coordinates (sub-pixel).
#[inline]
pub fn push_f64_1(out: &mut Vec<u8>, v: f64) {
    let neg = v < 0.0;
    let v = v.abs();
    let int = v as u64;
    let frac = ((v - int as f64) * 10.0 + 0.5) as u64 % 10;
    if neg {
        out.push(b'-');
    }
    push_u64(out, int);
    out.push(b'.');
    out.push(b'0' + u8::try_from(frac).unwrap_or(0));
}

/// Write an `f64` with zero decimal places (rounded integer) into `out`.
#[inline]
pub fn push_f64_0(out: &mut Vec<u8>, v: f64) {
    let neg = v < 0.0;
    let v = v.abs() + 0.5;
    if neg {
        out.push(b'-');
    }
    push_u64(out, v as u64);
}

#[inline]
fn push_u64(out: &mut Vec<u8>, mut v: u64) {
    let mut buf = [0u8; 20];
    let mut i = 20usize;
    if v == 0 {
        out.push(b'0');
        return;
    }
    while v > 0 {
        i -= 1;
        buf[i] = b'0' + u8::try_from(v % 10).unwrap_or(0);
        v /= 10;
    }
    out.extend_from_slice(&buf[i..]);
}

// ── Format dispatch registry ──────────────────────────────────────────────────

pub type ParserFn = fn(src: &[u8], scene: &mut Scene) -> Result<(), Error>;

#[derive(Debug)]
pub struct FormatEntry {
    pub name:       &'static str,
    pub magic:      &'static [u8],
    pub extensions: &'static [&'static str],
    pub parse:      ParserFn,
}

pub static FORMAT_REGISTRY: &[FormatEntry] = &[
    FormatEntry {
        name:       "csv",
        magic:      b"",
        extensions: &["csv", "tsv"],
        parse:      csv::parse_into_scene,
    },
    FormatEntry {
        name:       "md",
        magic:      b"",
        extensions: &["md", "markdown"],
        parse:      md::parse_into_scene,
    },
    FormatEntry {
        name:       "ipynb",
        magic:      b"{\n \"cells\":",
        extensions: &["ipynb"],
        parse:      ipynb::parse_into_scene,
    },
    FormatEntry {
        name:       "dot",
        magic:      b"digraph",
        extensions: &["dot", "gv"],
        parse:      dot::parse_into_scene,
    },
    FormatEntry {
        name:       "svg",
        magic:      b"<svg",
        extensions: &["svg"],
        parse:      svg::parse_into_scene,
    },
    FormatEntry {
        name:       "tex",
        magic:      b"\\",
        extensions: &["tex", "latex"],
        parse:      tex::parse_into_scene,
    },
    FormatEntry {
        name:       "chem",
        magic:      b"",
        extensions: &["mol", "sdf", "rxn"],
        parse:      chem::parse_into_scene,
    },
];

/// Table-driven O(entries) format dispatch by extension then magic bytes.
pub fn dispatch_into_scene(src: &[u8], ext: &str, scene: &mut Scene) -> Result<(), Error> {
    // Extension match first (O(1) for small registry, branch predictor-friendly).
    if !ext.is_empty() {
        for entry in FORMAT_REGISTRY {
            if entry.extensions.iter().any(|&e| e.eq_ignore_ascii_case(ext)) {
                return (entry.parse)(src, scene);
            }
        }
    }
    // Magic-byte fallback.
    for entry in FORMAT_REGISTRY {
        if !entry.magic.is_empty() && src.starts_with(entry.magic) {
            return (entry.parse)(src, scene);
        }
    }
    Ok(())
}
