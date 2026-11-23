//! Mermaid diagram adapter backed by pure-Rust `mermaid-svg`.
//! Delegates SVG ingestion through `fmt::dispatch_kind` (zero direct coupling to `svg::parse_into_scene`).

use crate::theme;
use crate::{
    err::Error,
    fmt::{self, Kind, svg},
    scene::Scene,
    theme::Palette,
};

fn themed_svg(src: &[u8], palette: &Palette) -> Result<String, Error> {
    use crate::theme::SemanticRole;
    use std::borrow::Cow;
    let text = core::str::from_utf8(src).map_err(|_| Error::MermaidCompile)?;
    let fg = Cow::Borrowed(palette.hex_str(SemanticRole::TextPrimary));
    let line = Cow::Borrowed(palette.series_hex_str(0));
    let theme = mermaid_svg::Theme {
        fg: fg.clone(),
        fg_muted: Cow::Borrowed(palette.hex_str(SemanticRole::TextMuted)),
        bg: Cow::Borrowed("none"),
        actor_fill: Cow::Borrowed("none"),
        actor_stroke: line.clone(),
        actor_text_color: Some(fg.clone()),
        lifeline: line.clone(),
        arrow_stroke: line.clone(),
        signal_text_color: Some(fg),
        note_fill: Cow::Borrowed("none"),
        activation_fill: Cow::Borrowed("none"),
        frame_label_fill: Cow::Borrowed("none"),
        flow_node_fill: Cow::Borrowed("none"),
        flow_node_stroke: line.clone(),
        flow_edge_stroke: line,
        flow_label_bg: Cow::Borrowed("none"),
        flow_cluster_fill: Cow::Borrowed("none"),
        ..mermaid_svg::Theme::default()
    };
    mermaid_svg::render_with(text, &theme).map_err(|_| Error::MermaidCompile)
}

/// Compile Mermaid source into theme-adapted SVG inside `out_svg`.
///
/// # Errors
/// Returns `Error::MermaidCompile` on invalid UTF-8 or Mermaid syntax error.
pub fn render(src: &[u8], palette: &Palette, out_svg: &mut Vec<u8>) -> Result<(), Error> {
    let raw_svg = themed_svg(src, palette)?;
    out_svg.clear();
    svg::rewrite_colors(raw_svg.as_bytes(), palette, out_svg);
    Ok(())
}

/// Compile Mermaid diagram and dispatch resulting SVG into `Scene`.
///
/// # Errors
/// Returns `Error::MermaidCompile` or `Error::SvgParse`.
pub fn parse_into_scene(src: &[u8], scene: &mut Scene) -> Result<(), Error> {
    parse_with_palette(src, &theme::DARK, scene)
}

/// Parse with an explicit receiving palette.
///
/// # Errors
/// Propagates format decoding errors.
pub fn parse_with_palette(src: &[u8], palette: &Palette, scene: &mut Scene) -> Result<(), Error> {
    let raw_svg = themed_svg(src, palette)?;
    fmt::dispatch_with_palette(Kind::Svg, raw_svg.as_bytes(), palette, scene)
}
