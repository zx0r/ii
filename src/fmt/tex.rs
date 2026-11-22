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

fn transpile_ce(body: &[u8], out: &mut Vec<u8>) {
    let mut pos = 0usize;
    let mut after_atom = false;
    while pos < body.len() {
        let rest = &body[pos..];
        if rest.starts_with(b"<=>") {
            out.extend_from_slice(b" \\rightleftharpoons ");
            pos += 3;
            after_atom = false;
        } else if rest.starts_with(b"<->") {
            out.extend_from_slice(b" \\leftrightarrow ");
            pos += 3;
            after_atom = false;
        } else if rest.starts_with(b"->") {
            out.extend_from_slice(b" \\rightarrow ");
            pos += 2;
            after_atom = false;
        } else if rest.starts_with(b"<-") {
            out.extend_from_slice(b" \\leftarrow ");
            pos += 2;
            after_atom = false;
        } else if body[pos].is_ascii_alphabetic() {
            let start = pos;
            pos += 1;
            while pos < body.len() && body[pos].is_ascii_lowercase() {
                pos += 1;
            }
            out.extend_from_slice(b"\\mathrm{");
            out.extend_from_slice(&body[start..pos]);
            out.push(b'}');
            after_atom = true;
        } else if body[pos].is_ascii_digit() && after_atom {
            let start = pos;
            while pos < body.len() && body[pos].is_ascii_digit() {
                pos += 1;
            }
            out.extend_from_slice(b"_{");
            out.extend_from_slice(&body[start..pos]);
            out.push(b'}');
            after_atom = false;
        } else if body[pos] == b'^' {
            pos += 1;
            let start = pos;
            while pos < body.len()
                && (body[pos].is_ascii_alphanumeric() || matches!(body[pos], b'+' | b'-'))
            {
                pos += 1;
            }
            out.extend_from_slice(b"^{");
            out.extend_from_slice(&body[start..pos]);
            out.push(b'}');
            after_atom = false;
        } else {
            let byte = body[pos];
            out.push(byte);
            after_atom = matches!(byte, b')' | b']');
            pos += 1;
        }
    }
}

fn expand_ce(src: &[u8], out: &mut Vec<u8>) {
    out.clear();
    out.reserve(src.len().saturating_mul(2));
    let mut pos = 0usize;
    while pos < src.len() {
        let Some(rel) = src[pos..].windows(4).position(|w| w == b"\\ce{") else {
            out.extend_from_slice(&src[pos..]);
            break;
        };
        out.extend_from_slice(&src[pos..pos + rel]);
        let body_start = pos + rel + 4;
        let mut depth = 1u32;
        let mut cur = body_start;
        while cur < src.len() && depth > 0 {
            match src[cur] {
                b'{' => depth += 1,
                b'}' => depth -= 1,
                _ => {}
            }
            if depth > 0 {
                cur += 1;
            }
        }
        transpile_ce(&src[body_start..cur], out);
        pos = (cur + 1).min(src.len());
    }
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
    let svg_str = ratex_svg::render_to_svg(&display_list, &ratex_svg::SvgOptions::default());
    svg::rewrite_colors(svg_str.as_bytes(), palette, out_svg);
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
    let raw = strip_delims(latex_bytes);
    let mut expanded = Vec::new();
    let src = if raw.windows(4).any(|w| w == b"\\ce{") {
        expand_ce(raw, &mut expanded);
        expanded.as_slice()
    } else {
        raw
    };
    let expr = core::str::from_utf8(src).map_err(|_| Error::LatexCompile)?;
    compile_expr(expr, display, palette, out_svg)
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

fn stage_or_fallback(raw_src: &[u8], display: bool, is_chem: bool, scene: &mut Scene) {
    let start = scene.text_pool.len();
    let compiled = if is_chem {
        let mut buf = Vec::with_capacity(raw_src.len().saturating_mul(2));
        transpile_ce(trim_ascii(raw_src), &mut buf);
        core::str::from_utf8(&buf)
            .map_err(|_| Error::LatexCompile)
            .and_then(|expr| compile_expr(expr, display, &theme::DARK, &mut scene.text_pool))
    } else {
        compile_latex(raw_src, display, &theme::DARK, &mut scene.text_pool)
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
        if compile_latex_streaming(
            raw_src,
            display,
            &theme::DARK,
            &mut healed,
            &mut scene.text_pool,
        )
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
    stage_or_fallback(src, true, false, scene);
    Ok(())
}

/// Compile inline `LaTeX` math (`$...$`) into `Scene`.
///
/// # Errors
/// Never fails hard; returns `Ok(())`.
pub fn parse_inline_into_scene(src: &[u8], scene: &mut Scene) -> Result<(), Error> {
    stage_or_fallback(src, false, false, scene);
    Ok(())
}

/// Compile bare `mhchem` block into `Scene` via `ratex-*`.
///
/// # Errors
/// Never fails hard; returns `Ok(())`.
pub fn parse_chem_into_scene(src: &[u8], scene: &mut Scene) -> Result<(), Error> {
    stage_or_fallback(src, true, true, scene);
    Ok(())
}
