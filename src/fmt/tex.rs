//! `LaTeX` / `mhchem` math compiler backed by `ratex-*`.
//! Zero-copy fast-path when no `\ce{...}` macro is present.

use crate::{
    err::Error,
    fmt::{font, heal, svg, trim_ascii},
    scene::Scene,
    theme::{self, ColorToken, Palette},
};

#[inline]
fn strip_delims(src: &[u8]) -> &[u8] {
    let trimmed = trim_ascii(src);
    if let Some(inner) = trimmed
        .strip_prefix(b"\x24\x24")
        .and_then(|s| s.strip_suffix(b"\x24\x24"))
    {
        return trim_ascii(inner);
    }
    if let Some(inner) = trimmed
        .strip_prefix(b"\\[")
        .and_then(|s| s.strip_suffix(b"\\]"))
    {
        return trim_ascii(inner);
    }
    if let Some(inner) = trimmed
        .strip_prefix(b"\\(")
        .and_then(|s| s.strip_suffix(b"\\)"))
    {
        return trim_ascii(inner);
    }
    trimmed
        .strip_prefix(b"$")
        .and_then(|s| s.strip_suffix(b"$"))
        .map_or(trimmed, trim_ascii)
}

fn compile_expr(
    expr: &str,
    display: bool,
    palette: &Palette,
    out_svg: &mut Vec<u8>,
) -> Result<(), Error> {
    let nodes = ratex_parser::parse(expr).map_err(|_| Error::LatexCompile)?;
    let style = if display {
        ratex_types::MathStyle::Display
    } else {
        ratex_types::MathStyle::Text
    };
    let opts = ratex_layout::LayoutOptions::default().with_style(style);
    let lbox = ratex_layout::layout(&nodes, &opts);
    let display_list = ratex_layout::to_display_list(&lbox);
    // Canonical 18px terminal cell; image::Context scales this with actual DPI.
    // ratex's 40pt/10pt defaults turn an inline glyph into a four-row poster.
    let svg_opts = ratex_svg::SvgOptions {
        font_size: if display { 12.24 } else { 11.7 },
        padding: 1.2,
        embed_glyphs: true,
        ..ratex_svg::SvgOptions::default()
    };
    let svg_str = ratex_svg::render_to_svg_with_color_syntax(
        &display_list,
        &svg_opts,
        ratex_svg::SvgColorSyntax::Rgb,
    );
    svg::rewrite_colors_as(
        svg_str.as_bytes(),
        palette,
        theme::SemanticRole::Accent,
        out_svg,
    );
    Ok(())
}

/// Compile `LaTeX` (with optional inline `\ce{...}`) into theme-adapted SVG inside `out_svg`.
///
/// # Errors
/// Returns `Error::LatexCompile` if UTF-8 validation or `ratex_parser::parse` fails.
pub fn compile_latex(
    latex_bytes: &[u8],
    display: bool,
    palette: &Palette,
    out_svg: &mut Vec<u8>,
) -> Result<(), Error> {
    let expr = core::str::from_utf8(strip_delims(latex_bytes)).map_err(|_| Error::LatexCompile)?;
    compile_expr(expr, display, palette, out_svg)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TexMode {
    Complete,
    Streaming,
}

/// Compile `LaTeX` with an explicit compilation mode (`Complete` or `Streaming`).
///
/// # Errors
/// Returns `Error::LatexCompile` if compilation or repair fails.
pub fn compile_latex_mode(
    latex_bytes: &[u8],
    display: bool,
    palette: &Palette,
    mode: TexMode,
    heal_buf: &mut Vec<u8>,
    out_svg: &mut Vec<u8>,
) -> Result<(), Error> {
    match mode {
        TexMode::Complete => compile_latex(latex_bytes, display, palette, out_svg),
        TexMode::Streaming => {
            compile_latex_streaming(latex_bytes, display, palette, heal_buf, out_svg)
        }
    }
}

/// Compile a live (possibly incomplete) expression after bounded structural repair.
/// Complete expressions borrow the original bytes and do not copy through the healer.
///
/// # Errors
/// Returns `Error::LatexCompile` when the repaired expression cannot be parsed.
pub fn compile_latex_streaming(
    latex_bytes: &[u8],
    display: bool,
    palette: &Palette,
    heal_buf: &mut Vec<u8>,
    out_svg: &mut Vec<u8>,
) -> Result<(), Error> {
    compile_latex(heal::heal(latex_bytes, heal_buf), display, palette, out_svg)
}

fn stage_or_fallback(
    raw_src: &[u8],
    display: bool,
    is_chem: bool,
    palette: &Palette,
    scene: &mut Scene,
) {
    let start = scene.text_pool.len();
    let compiled = if is_chem && memchr::memmem::find(raw_src, b"\\ce{").is_none() {
        let mut wrapped = b"\\ce{".to_vec();
        wrapped.extend_from_slice(trim_ascii(raw_src));
        wrapped.push(b'}');
        compile_latex(&wrapped, display, palette, &mut scene.text_pool)
    } else {
        compile_latex(raw_src, display, palette, &mut scene.text_pool)
    };

    if compiled.is_ok() {
        let (svg_w, svg_h) = svg::extract_svg_dimensions(&scene.text_pool[start..]);
        if scene.commit_svg(0.0, scene.height, svg_w, svg_h, start) {
            scene.width = scene.width.max(svg_w);
            scene.height += svg_h;
            return;
        }
    }

    scene.text_pool.truncate(start);
    if !is_chem {
        let mut healed = Vec::with_capacity(raw_src.len().saturating_add(64));
        if compile_latex_streaming(raw_src, display, palette, &mut healed, &mut scene.text_pool)
            .is_ok()
        {
            let (svg_w, svg_h) = svg::extract_svg_dimensions(&scene.text_pool[start..]);
            if scene.commit_svg(0.0, scene.height, svg_w, svg_h, start) {
                scene.width = scene.width.max(svg_w);
                scene.height += svg_h;
                return;
            }
        }
        scene.text_pool.truncate(start);
    }
    let fallback = core::str::from_utf8(trim_ascii(raw_src)).unwrap_or("");
    let cursor_y = scene.height + 16.0;
    scene.push_text(8.0, cursor_y, fallback, ColorToken::FG_MUTED, 14);
    scene.width = scene.width.max(font::measure_str(fallback, 14.0) + 16.0);
    scene.height = cursor_y + 8.0;
}

/// Compile `LaTeX` math block into `Scene` (honest degradation to literal text on syntax error).
///
/// # Errors
/// Never fails hard; returns `Ok(())`.
pub fn parse_into_scene(src: &[u8], scene: &mut Scene) -> Result<(), Error> {
    stage_or_fallback(src, true, false, &theme::DARK, scene);
    Ok(())
}

/// Compile inline `LaTeX` math (`$...$`) into `Scene`.
///
/// # Errors
/// Never fails hard; returns `Ok(())`.
pub fn parse_inline_into_scene(src: &[u8], scene: &mut Scene) -> Result<(), Error> {
    stage_or_fallback(src, false, false, &theme::DARK, scene);
    Ok(())
}

/// Compile bare `mhchem` block into `Scene` via `ratex-*`.
///
/// # Errors
/// Never fails hard; returns `Ok(())`.
pub fn parse_chem_into_scene(src: &[u8], scene: &mut Scene) -> Result<(), Error> {
    stage_or_fallback(src, true, true, &theme::DARK, scene);
    Ok(())
}

/// Compile using the receiving surface palette.
///
/// # Errors
/// Invalid expressions produce literal fallback text.
pub fn parse_with_palette(
    src: &[u8],
    display: bool,
    chem: bool,
    palette: &Palette,
    scene: &mut Scene,
) -> Result<(), Error> {
    stage_or_fallback(src, display, chem, palette, scene);
    Ok(())
}

/// Normalize non-executing document wrappers and section headings for prose composition.
///
/// # Errors
/// Rejects invalid UTF-8.
pub fn document_to_markdown(src: &[u8], out: &mut Vec<u8>) -> Result<(), Error> {
    let text = core::str::from_utf8(src).map_err(|_| Error::LatexCompile)?;
    for line in text.lines() {
        let clean = line.trim();
        if [
            "\\documentclass",
            "\\usepackage",
            "\\begin{document}",
            "\\end{document}",
        ]
        .iter()
        .any(|prefix| clean.starts_with(prefix))
        {
            continue;
        }
        let heading = [
            ("\\section{", b"# ".as_slice()),
            ("\\subsection{", b"## ".as_slice()),
            ("\\subsubsection{", b"### ".as_slice()),
        ]
        .into_iter()
        .find_map(|(tag, marker)| {
            clean
                .strip_prefix(tag)
                .and_then(|s| s.strip_suffix('}'))
                .map(|body| (marker, body))
        });
        if let Some((marker, body)) = heading {
            out.extend_from_slice(marker);
            out.extend_from_slice(body.as_bytes());
        } else {
            out.extend_from_slice(
                line.replace("\\[", "$$")
                    .replace("\\]", "$$")
                    .replace("\\(", "$")
                    .replace("\\)", "$")
                    .as_bytes(),
            );
        }
        out.push(b'\n');
    }
    Ok(())
}
