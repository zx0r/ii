//! Mermaid diagram adapter backed by pure-Rust `mermaid-svg`.
//! Delegates SVG ingestion through `fmt::dispatch_kind` (zero direct coupling to `svg::parse_into_scene`).

use crate::{
    err::Error,
    fmt::{self, Kind, svg},
    scene::Scene,
    theme::Palette,
};

/// Compile Mermaid source into theme-adapted SVG inside `out_svg`.
///
/// # Errors
/// Returns `Error::MermaidCompile` on invalid UTF-8 or Mermaid syntax error.
pub fn render(src: &[u8], palette: &Palette, out_svg: &mut Vec<u8>) -> Result<(), Error> {
    let text = core::str::from_utf8(src).map_err(|_| Error::MermaidCompile)?;
    let raw_svg = mermaid_svg::render(text).map_err(|_| Error::MermaidCompile)?;
    out_svg.clear();
    svg::rewrite_colors(raw_svg.as_bytes(), palette, out_svg);
    Ok(())
}

/// Compile Mermaid diagram and dispatch resulting SVG into `Scene`.
///
/// # Errors
/// Returns `Error::MermaidCompile` or `Error::SvgParse`.
pub fn parse_into_scene(src: &[u8], scene: &mut Scene) -> Result<(), Error> {
    let text = core::str::from_utf8(src).map_err(|_| Error::MermaidCompile)?;
    let raw_svg = mermaid_svg::render(text).map_err(|_| Error::MermaidCompile)?;
    fmt::dispatch_kind(Kind::Svg, raw_svg.as_bytes(), scene)
}
