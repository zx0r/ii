//! LaTeX / Typst math ingest adapter via ratex.
//! Translates math formulas into SVG rewritten into scene.text_pool.
//! parse_into_scene: scan for display/inline math blocks, compile each via ratex.

use crate::{err::Error, fmt::heal, scene::Scene, theme::Palette};

/// Compile a single LaTeX math expression into themed SVG, appending to `out_svg`.
/// `display` = true → `\[...\]` display math; false → `\(...\)` inline.
pub fn compile_latex(
    latex_bytes: &[u8],
    display: bool,
    palette: &Palette,
    heal_state: &mut heal::State,
    aux: &mut Vec<u8>,
    out_svg: &mut Vec<u8>,
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
    let dlist      = ratex_layout::to_display_list(&layout_box);
    let svg_opts   = ratex_svg::SvgOptions::default();
    let raw_svg    = ratex_svg::render_to_svg(&dlist, &svg_opts);

    aux.clear();
    crate::fmt::svg::rewrite_colors(raw_svg.as_bytes(), palette, aux);
    out_svg.extend_from_slice(aux);
    Ok(())
}

/// Scan `src` for `\[...\]` and `\(...\)` math blocks and compile each into `scene`.
/// Each compiled SVG blob is stored as a GlyphRun with `flags = 0x80` (SVG marker).
pub fn parse_into_scene(src: &[u8], scene: &mut Scene) -> Result<(), Error> {
    use crate::{
        fmt::md::{scan, Kind},
        scene::GlyphRun,
        theme::ColorToken,
    };

    let mut spans = Vec::with_capacity(16);
    scan(src, &mut spans);

    let mut heal_state = heal::State::new();
    let mut aux        = Vec::with_capacity(2048);

    for sp in &spans {
        if !matches!(sp.kind(), Kind::MathDisplay | Kind::MathInline) {
            continue;
        }
        let display = sp.kind() == Kind::MathDisplay;
        let ps = sp.payload_start as usize;
        let pe = sp.payload_end() as usize;
        if ps >= pe || pe > src.len() {
            continue;
        }
        let payload = &src[ps..pe];

        let svg_start = scene.text_pool.len() as u32;
        // compile_latex appends rewritten SVG into scene.text_pool via aux+extend
        // We build into aux first, then push.
        if compile_latex(payload, display, &crate::theme::DARK, &mut heal_state, &mut aux, &mut scene.text_pool).is_err() {
            // Graceful degradation: store raw LaTeX bytes as a text run.
            let text_off = scene.text_pool.len() as u32;
            let text_len = payload.len().min(u16::MAX as usize) as u16;
            scene.text_pool.extend_from_slice(&payload[..text_len as usize]);
            scene.glyphs.push(GlyphRun {
                x: 0.0,
                y: scene.height,
                text_off,
                text_len,
                font_size: 14,
                color: ColorToken::FG_MUTED,
                flags: 0x02, // italic
                _pad: [0; 2],
            });
            scene.height += 20.0;
            continue;
        }

        let svg_len = scene.text_pool.len() - svg_start as usize;
        if svg_len == 0 { continue; }

        let (svg_w, svg_h) = crate::fmt::svg::extract_svg_dimensions(
            &scene.text_pool[svg_start as usize..],
        );

        scene.glyphs.push(GlyphRun {
            x: 0.0,
            y: scene.height,
            text_off: svg_start,
            text_len: svg_len.min(u16::MAX as usize) as u16,
            font_size: 0,
            color: ColorToken::FG_PRIMARY,
            flags: 0x80, // SVG blob marker
            _pad: [0; 2],
        });
        scene.height += svg_h;
        if svg_w > scene.width { scene.width = svg_w; }
    }

    Ok(())
}
