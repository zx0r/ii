//! LaTeX / Typst math ingest adapter via ratex.
//! Translates math formulas into SVG or direct scene geometry.

use crate::{err::Error, fmt::heal, theme::Palette};

pub fn compile_latex(
    latex_bytes: &[u8],
    display: bool,
    palette: &Palette,
    heal_state: &mut heal::State,
    aux: &mut Vec<u8>,
    out_svg: &mut String,
) -> Result<(), Error> {
    aux.clear();
    heal_state.heal(latex_bytes, aux);
    let healed = core::str::from_utf8(aux).map_err(|_| Error::LatexCompile)?;

    let ast = ratex_parser::parse(healed).map_err(|_| Error::LatexCompile)?;
    let math_style = if display {
        ratex_types::MathStyle::Display
    } else {
        ratex_types::MathStyle::Text
    };
    let layout_opts = ratex_layout::LayoutOptions {
        style: math_style,
        ..ratex_layout::LayoutOptions::default()
    };
    let layout_box = ratex_layout::layout(&ast, &layout_opts);
    let dlist = ratex_layout::to_display_list(&layout_box);
    let svg_opts = ratex_svg::SvgOptions::default();
    let raw_svg = ratex_svg::render_to_svg(&dlist, &svg_opts);

    aux.clear();
    crate::fmt::svg::rewrite_colors(raw_svg.as_bytes(), palette, aux);
    let themed = core::str::from_utf8(aux).map_err(|_| Error::SvgParse)?;
    out_svg.clear();
    out_svg.push_str(themed);
    Ok(())
}
