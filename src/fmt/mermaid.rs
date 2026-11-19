//! Mermaid diagram ingest adapter via mermaid-svg.

use crate::{err::Error, theme::Palette};

pub fn compile_mermaid(
    src: &[u8],
    palette: &Palette,
    aux: &mut Vec<u8>,
    out_svg: &mut String,
) -> Result<(), Error> {
    let text = core::str::from_utf8(src).map_err(|_| Error::MermaidCompile)?;
    let raw_svg = mermaid_svg::render(text).map_err(|_| Error::MermaidCompile)?;
    aux.clear();
    crate::fmt::svg::rewrite_colors(raw_svg.as_bytes(), palette, aux);
    let themed = core::str::from_utf8(aux).map_err(|_| Error::SvgParse)?;
    out_svg.clear();
    out_svg.push_str(themed);
    Ok(())
}
