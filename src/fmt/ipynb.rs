//! Zero-serde `.ipynb` cell descriptor and extraction.


use crate::{err::Error, fmt::{json::Nav, md::Kind}};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u32)]
pub enum CellKind {
    Code = 0,
    Markdown = 1,
    Raw = 2,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
#[repr(C)]
pub struct Cell {
    pub source_start: u32,
    pub source_end: u32,
    pub outputs_start: u32,
    pub outputs_end: u32,
    pub cell_type: u32,
    pub execution_count: u32,
    pub cell_index: u32,
    pub output_count: u32,
}

const _: () = assert!(size_of::<Cell>() == 32);
const _: () = assert!(align_of::<Cell>() == 4);

/// Extract all notebook cells from `.ipynb` JSON `src` into `cells`.
pub fn extract_cells<'a>(src: &'a [u8], cells: &mut Vec<Cell>) -> Result<Nav<'a>, Error> {
    let root = Nav::new(src);
    let cells_nav = root.get(b"cells").ok_or(Error::JsonNav)?;

    for (idx, item) in cells_nav.items().enumerate() {
        let ct_bytes = item
            .get(b"cell_type")
            .and_then(|n| n.str_raw())
            .unwrap_or(b"code");
        let kind = if ct_bytes == b"markdown" {
            CellKind::Markdown
        } else if ct_bytes == b"raw" {
            CellKind::Raw
        } else {
            CellKind::Code
        };

        let (source_start, source_end) = item.get(b"source").map_or((0, 0), |n| n.span());
        let outputs_nav = item.get(b"outputs");
        let (outputs_start, outputs_end) = outputs_nav.map_or((0, 0), |n| n.span());
        let output_count = outputs_nav.map_or(0, |n| n.items().count() as u32);
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
            cell_index: idx as u32,
            output_count,
        });
    }

    Ok(root)
}

/// Resolve the highest-priority graphical MIME output from a notebook output JSON object `nav`.
#[must_use]
pub fn resolve_output<'a>(nav: Nav<'a>, _src: &'a [u8]) -> Option<(Kind, &'a [u8])> {
    let data = nav.get(b"data").unwrap_or(nav);

    if let Some(svg) = data.get(b"image/svg+xml") {
        let raw = svg.str_raw().unwrap_or_else(|| svg.raw());
        return Some((Kind::RawSvg, raw));
    }
    if let Some(vl) = data
        .get(b"application/vnd.vegalite.v5+json")
        .or_else(|| data.get(b"application/vnd.vegalite.v4+json"))
    {
        return Some((Kind::VegaLite, vl.raw()));
    }
    if let Some(pl) = data.get(b"application/vnd.plotly.v1+json") {
        return Some((Kind::Plotly, pl.raw()));
    }
    if let Some(latex) = data.get(b"text/latex") {
        let raw = latex.str_raw().unwrap_or_else(|| latex.raw());
        return Some((Kind::MathDisplay, raw));
    }
    if let Some(png) = data.get(b"image/png") {
        let raw = png.str_raw().unwrap_or_else(|| png.raw());
        return Some((Kind::NotebookOut, raw));
    }
    None
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

        let out_slice = &nb[cells[1].outputs_start as usize..cells[1].outputs_end as usize];
        let first_out = Nav::new(out_slice)
            .items()
            .next()
            .unwrap_or_else(|| Nav::new(b"{}"));
        let resolved = resolve_output(first_out, out_slice);
        assert_eq!(resolved, Some((Kind::RawSvg, &b"<svg/>"[..])));
    }
}
