//! Closed-enum format router (`Kind`) and zero-allocation byte/ASCII/XML formatters.

pub mod archive;
pub mod chart;
pub mod csv;
pub mod dot;
pub mod font;
pub mod heal;
pub mod html;
pub mod ipynb;
pub mod json;
pub mod md;
pub mod mdx;
pub mod mermaid;
pub mod svg;
pub mod tex;
pub mod typst;

use crate::{
    err::Error,
    scene::Scene,
    theme::{self, Palette},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Kind {
    Md = 0,
    Ipynb = 1,
    Tex = 2,
    TexInline = 3,
    Chem = 4,
    Mermaid = 5,
    Dot = 6,
    Svg = 7,
    Csv = 8,
    Tsv = 9,
    Chart = 10,
    Archive = 11,
    Png = 12,
    Typst = 13,
    Mdx = 14,
    Html = 15,
}

const EXT_MAP: &[(&[&str], Kind)] = &[
    (&["md", "markdown", "qmd", "rmd"], Kind::Md),
    (&["mdx"], Kind::Mdx),
    (&["html", "htm"], Kind::Html),
    (&["typ", "typst"], Kind::Typst),
    (&["ipynb"], Kind::Ipynb),
    (&["tex", "latex"], Kind::Tex),
    (&["chem", "mhchem"], Kind::Chem),
    (&["mmd", "mermaid"], Kind::Mermaid),
    (&["dot", "gv"], Kind::Dot),
    (&["svg"], Kind::Svg),
    (&["png"], Kind::Png),
    (&["csv"], Kind::Csv),
    (&["tsv"], Kind::Tsv),
    (&["vl", "vega-lite", "plotly"], Kind::Chart),
    (
        &["zip", "jar", "whl", "tar", "tgz", "tbz2", "txz", "tzst"],
        Kind::Archive,
    ),
];

/// Map a file extension or Markdown fence tag (ASCII case-insensitive) to canonical `Kind`.
#[must_use]
pub fn kind_from_ext(ext: &str) -> Option<Kind> {
    if ext.is_empty() {
        return None;
    }
    EXT_MAP.iter().find_map(|&(exts, kind)| {
        exts.iter()
            .any(|candidate| ext.eq_ignore_ascii_case(candidate))
            .then_some(kind)
    })
}

/// Resolve `Kind` by extension first, falling back to zero-copy magic-byte sniffing.
#[must_use]
pub fn detect_kind(src: &[u8], ext: &str) -> Kind {
    if let Some(kind) = kind_from_ext(ext) {
        return kind;
    }
    if src.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Kind::Png;
    }
    let head = trim_ascii(src);
    if archive::looks_like_archive(src) {
        return Kind::Archive;
    }
    if head.starts_with(b"<svg") || head.starts_with(b"<?xml") {
        return Kind::Svg;
    }
    if head.starts_with(b"digraph") || head.starts_with(b"graph ") || head.starts_with(b"strict ") {
        return Kind::Dot;
    }
    if head.starts_with(b"{") {
        return if head.windows(7).take(64).any(|w| w == b"\"cells\"") {
            Kind::Ipynb
        } else {
            Kind::Chart
        };
    }
    Kind::Md
}

/// Dispatch bytes by resolved `Kind` into `Scene` (single closed-enum routing boundary).
///
/// # Errors
/// Propagates parser errors from the selected format adapter.
pub fn dispatch_kind(kind: Kind, src: &[u8], scene: &mut Scene) -> Result<(), Error> {
    dispatch_with_palette(kind, src, &theme::DARK, scene)
}

/// Dispatch with the palette supplied by the receiving surface.
///
/// # Errors
/// Propagates the selected format adapter error.
pub fn dispatch_with_palette(
    kind: Kind,
    src: &[u8],
    palette: &Palette,
    scene: &mut Scene,
) -> Result<(), Error> {
    match kind {
        Kind::Md => md::parse_with_palette(src, palette, scene),
        Kind::Mdx => {
            let mut presentation = Vec::new();
            mdx::presentation_source(src, &mut presentation)?;
            md::parse_with_palette(&presentation, palette, scene)
        }
        Kind::Html => {
            let mut presentation = Vec::new();
            html::presentation_source(src, &mut presentation)?;
            md::parse_with_palette(&presentation, palette, scene)
        }
        Kind::Ipynb => ipynb::parse_with_palette(src, palette, scene),
        Kind::Tex => tex::parse_with_palette(src, true, false, palette, scene),
        Kind::TexInline => tex::parse_with_palette(src, false, false, palette, scene),
        Kind::Chem => tex::parse_with_palette(src, true, true, palette, scene),
        Kind::Mermaid => mermaid::parse_with_palette(src, palette, scene),
        Kind::Dot => dot::parse_with_palette(src, palette, scene),
        Kind::Svg => svg::parse_into_scene_with_palette(src, palette, scene),
        Kind::Csv => csv::parse_into_scene(src, scene),
        Kind::Tsv => csv::parse_tsv_into_scene(src, scene),
        Kind::Chart => chart::parse_with_palette(src, palette, scene),
        Kind::Archive => archive::parse_into_scene(src, scene),
        Kind::Png => Err(Error::PngDecode),
        Kind::Typst => {
            let mut normalized = Vec::new();
            typst::to_markdown(src, &mut normalized)?;
            md::parse_with_palette(&normalized, palette, scene)
        }
    }
}

/// Dispatch raw document bytes into `Scene` by extension or magic sniffing.
///
/// # Errors
/// Propagates parser errors from the selected format adapter.
pub fn dispatch_into_scene(src: &[u8], ext: &str, scene: &mut Scene) -> Result<(), Error> {
    dispatch_kind(detect_kind(src, ext), src, scene)
}

#[inline]
#[must_use]
pub const fn trim_ascii(bytes: &[u8]) -> &[u8] {
    bytes.trim_ascii()
}

#[inline]
pub fn push_u32(out: &mut Vec<u8>, n: u32) {
    push_u64(out, u64::from(n));
}

#[inline]
pub fn push_u64(out: &mut Vec<u8>, mut n: u64) {
    let mut buf = [0u8; 20];
    let mut idx = 20;
    loop {
        idx -= 1;
        buf[idx] = b'0' + u8::try_from(n % 10).unwrap_or(0);
        n /= 10;
        if n == 0 {
            break;
        }
    }
    out.extend_from_slice(&buf[idx..]);
}

/// Append `val` formatted to 1 decimal place into `out` without heap allocation.
#[inline]
pub fn push_f64_1(out: &mut Vec<u8>, val: f64) {
    if !val.is_finite() {
        out.extend_from_slice(b"0.0");
        return;
    }
    let abs = if val.is_sign_negative() {
        out.push(b'-');
        -val
    } else {
        val
    };
    let clamped = abs.min(900_719_925_474_099.0);
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "clamped non-negative finite f64 fits in u64 after * 10"
    )]
    let tenths = clamped.mul_add(10.0, 0.5) as u64;
    push_u64(out, tenths / 10);
    out.push(b'.');
    out.push(b'0' + u8::try_from(tenths % 10).unwrap_or(0));
}

/// Write XML-escaped bytes into `out` using batched slice copies.
pub fn write_xml_escaped(src: &[u8], out: &mut Vec<u8>) {
    out.reserve(src.len());
    let mut start = 0usize;
    for (idx, &byte) in src.iter().enumerate() {
        let esc: &[u8] = match byte {
            b'&' => b"&amp;",
            b'<' => b"&lt;",
            b'>' => b"&gt;",
            b'"' => b"&quot;",
            b'\'' => b"&apos;",
            _ => continue,
        };
        if idx > start {
            out.extend_from_slice(&src[start..idx]);
        }
        out.extend_from_slice(esc);
        start = idx + 1;
    }
    if start < src.len() {
        out.extend_from_slice(&src[start..]);
    }
}
