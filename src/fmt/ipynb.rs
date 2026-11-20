//! Zero-serde `.ipynb` cell descriptor, extraction, and scene ingest.
//!
//! `parse_into_scene` dispatches each cell's graphical MIME output to the appropriate parser.

use crate::{
    err::Error,
    fmt::json::Nav,
    fmt::md::Kind,
    scene::{GlyphRun, Scene},
    theme::ColorToken,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum CellKind {
    Code     = 0,
    Markdown = 1,
    Raw      = 2,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct Cell {
    pub source_start:    u32,
    pub source_end:      u32,
    pub outputs_start:   u32,
    pub outputs_end:     u32,
    pub cell_type:       u32,
    pub execution_count: u32,
    pub cell_index:      u32,
    pub output_count:    u32,
}

const _: () = assert!(size_of::<Cell>() == 32);
const _: () = assert!(align_of::<Cell>() == 4);

/// Extract all notebook cells from `.ipynb` JSON `src` into `cells`.
/// Returns the root `Nav` for downstream MIME resolution.
///
/// # Errors
///
/// Returns [`Error::JsonNav`] if `cells` key is missing.
pub fn extract_cells<'a>(src: &'a [u8], cells: &mut Vec<Cell>) -> Result<Nav<'a>, Error> {
    let root      = Nav::new(src);
    let cells_nav = root.get(b"cells").ok_or(Error::JsonNav)?;

    for (idx, item) in cells_nav.items().enumerate() {
        let ct_bytes = item
            .get(b"cell_type")
            .and_then(|n| n.str_raw())
            .unwrap_or(b"code");
        let kind = match ct_bytes {
            b"markdown" => CellKind::Markdown,
            b"raw"      => CellKind::Raw,
            _           => CellKind::Code,
        };

        let (source_start, source_end) = item.get(b"source").map_or((0, 0), |n| n.span());
        let outputs_nav = item.get(b"outputs");
        let (outputs_start, outputs_end) = outputs_nav.map_or((0, 0), |n| n.span());
        let output_count = outputs_nav.map_or(0, |n| {
            u32::try_from(n.items().count()).unwrap_or(u32::MAX)
        });
        let execution_count = item
            .get(b"execution_count")
            .and_then(|n| n.int_of())
            .and_then(|v| u32::try_from(v).ok())
            .unwrap_or(0);

        cells.push(Cell {
            source_start,
            source_end,
            outputs_start,
            outputs_end,
            cell_type: kind as u32,
            execution_count,
            cell_index: u32::try_from(idx).unwrap_or(u32::MAX),
            output_count,
        });
    }

    Ok(root)
}

/// Resolve the highest-priority graphical MIME output from a notebook output JSON.
/// Priority: SVG > `VegaLite` > Plotly > `LaTeX` > PNG.
#[must_use]
pub fn resolve_output<'a>(nav: Nav<'a>, _src: &'a [u8]) -> Option<(Kind, &'a [u8])> {
    let d = nav.get(b"data").unwrap_or(nav);
    if let Some(svg) = d.get(b"image/svg+xml") {
        return Some((Kind::RawSvg, svg.str_raw().unwrap_or_else(|| svg.raw())));
    }
    if let Some(vl) = d.get(b"application/vnd.vegalite.v5+json").or_else(|| d.get(b"application/vnd.vegalite.v4+json")) {
        return Some((Kind::VegaLite, vl.raw()));
    }
    if let Some(pl) = d.get(b"application/vnd.plotly.v1+json") {
        return Some((Kind::Plotly, pl.raw()));
    }
    if let Some(latex) = d.get(b"text/latex") {
        return Some((Kind::MathDisplay, latex.str_raw().unwrap_or_else(|| latex.raw())));
    }
    if let Some(png) = d.get(b"image/png") {
        return Some((Kind::NotebookOut, png.str_raw().unwrap_or_else(|| png.raw())));
    }
    None
}

// ── Scene ingest ──────────────────────────────────────────────────────────────

/// Parse `.ipynb` JSON `src`, extract cells, and push each graphical output
/// into `scene` by dispatching to the appropriate format parser.
///
/// # Errors
///
/// Returns `Err` if cell extraction or sub-parser fails.
pub fn parse_into_scene(src: &[u8], scene: &mut Scene) -> Result<(), Error> {
    let mut cells = Vec::with_capacity(32);
    let _ = extract_cells(src, &mut cells)?;

    for cell in &cells {
        // Markdown cells: push source text as GlyphRun and scan for rich spans.
        if cell.cell_type == CellKind::Markdown as u32 {
            let ss = cell.source_start as usize;
            let se = cell.source_end   as usize;
            if ss < se && se <= src.len() {
                crate::fmt::md::parse_into_scene(&src[ss..se], scene)?;
            }
        }

        // Output cells: dispatch per MIME type.
        if cell.output_count == 0 { continue; }
        let os = cell.outputs_start as usize;
        let oe = cell.outputs_end   as usize;
        if os >= oe || oe > src.len() { continue; }

        let outputs_src = &src[os..oe];
        for out_nav in Nav::new(outputs_src).items() {
            let Some((kind, payload)) = resolve_output(out_nav, outputs_src) else { continue };
            match kind {
                Kind::RawSvg => {
                    crate::fmt::svg::parse_into_scene(payload, scene)?;
                }
                Kind::VegaLite => {
                    let svg_start = u32::try_from(scene.text_pool.len()).unwrap_or(u32::MAX);
                    let mut tmp = Vec::with_capacity(2048);
                    crate::fmt::chart::vegalite_bytes(Nav::new(payload), &crate::theme::DARK, 800, &mut tmp);
                    if !tmp.is_empty() {
                        let svg_len = tmp.len();
                        scene.text_pool.extend_from_slice(&tmp);
                        let (w, h) = crate::fmt::svg::extract_svg_dimensions(&tmp);
                        scene.glyphs.push(GlyphRun {
                            x: 0.0, y: scene.height, text_off: svg_start,
                            text_len: u16::try_from(svg_len.min(usize::from(u16::MAX))).unwrap_or(u16::MAX),
                            font_size: 0, color: ColorToken::FG_PRIMARY, flags: 0x80, _pad: [0; 2],
                        });
                        scene.height += h;
                        if w > scene.width { scene.width = w; }
                    }
                }
                Kind::MathDisplay | Kind::MathInline => {
                    crate::fmt::tex::parse_into_scene(payload, scene)?;
                }
                _ => {
                    let text_off = u32::try_from(scene.text_pool.len()).unwrap_or(u32::MAX);
                    let text_len = u16::try_from(payload.len().min(usize::from(u16::MAX))).unwrap_or(u16::MAX);
                    scene.text_pool.extend_from_slice(&payload[..usize::from(text_len)]);
                    scene.glyphs.push(GlyphRun {
                        x: 0.0, y: scene.height, text_off, text_len, font_size: 12,
                        color: ColorToken::FG_MUTED, flags: 0, _pad: [0; 2],
                    });
                    scene.height += 18.0;
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_cells_and_resolves_mime_priority() {
        let nb = b"{\"cells\":[{\"cell_type\":\"markdown\",\"source\":[\"# Title\"]},{\"cell_type\":\"code\",\"execution_count\":3,\"source\":[\"plot()\"],\"outputs\":[{\"data\":{\"image/svg+xml\":\"<svg/>\",\"image/png\":\"iVBORw0KGgo=\"}}]}]}";
        let mut cells = Vec::new();
        let root = extract_cells(nb, &mut cells);
        assert!(root.is_ok());
        assert_eq!(cells.len(), 2);
        assert_eq!(cells[0].cell_type, CellKind::Markdown as u32);
        assert_eq!(cells[1].execution_count, 3);
        assert_eq!(cells[1].output_count, 1);
    }

    #[test]
    fn resolve_output_svg_priority_over_png() {
        let out = b"{\"data\":{\"image/svg+xml\":\"<svg/>\",\"image/png\":\"abc\"}}";
        let nav = Nav::new(out);
        let resolved = resolve_output(nav, out);
        assert_eq!(resolved, Some((Kind::RawSvg, &b"<svg/>"[..])));
    }
}
