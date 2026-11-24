//! Compiler adapters (`ratex-*`, `mermaid-svg`) and full `showcase/` corpus verification.

use std::path::Path;

use ii::{
    err::Error,
    fmt::{self, mermaid, tex},
    raster,
    scene::Scene,
    theme::DARK,
};

#[test]
fn showcase_math_spans_compile_without_literal_fallback() -> Result<(), Box<dyn std::error::Error>>
{
    let mut failures = Vec::new();
    let mut count = 0;
    for entry in std::fs::read_dir("showcase")? {
        let path = entry?.path();
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        let Some(kind) = fmt::kind_from_ext(ext) else {
            continue;
        };
        if !matches!(
            kind,
            fmt::Kind::Md | fmt::Kind::Mdx | fmt::Kind::Tex | fmt::Kind::Typst | fmt::Kind::Ipynb
        ) {
            continue;
        }
        let src = std::fs::read(&path)?;
        let mut normalized = Vec::new();
        match kind {
            fmt::Kind::Tex => tex::document_to_markdown(&src, &mut normalized)?,
            fmt::Kind::Typst => fmt::typst::to_markdown(&src, &mut normalized)?,
            fmt::Kind::Ipynb => fmt::ipynb::to_markdown(&src, &mut normalized)?,
            _ => normalized.extend_from_slice(&src),
        }
        let mut spans = Vec::new();
        fmt::md::scan(&normalized, &mut spans);
        for span in spans {
            if !matches!(
                span.kind(),
                fmt::Kind::Tex | fmt::Kind::TexInline | fmt::Kind::Chem
            ) {
                continue;
            }
            let payload = &normalized[span.inner_start as usize..span.inner_end() as usize];
            let mut scene = Scene::with_capacity(8, 1024);
            tex::parse_with_palette(
                payload,
                span.kind() != fmt::Kind::TexInline,
                span.kind() == fmt::Kind::Chem,
                &DARK,
                &mut scene,
            )?;
            count += 1;
            if scene.svgs.is_empty() {
                failures.push(format!(
                    "{}: {}",
                    path.display(),
                    String::from_utf8_lossy(payload)
                ));
            }
        }
    }
    assert!(count > 100, "math corpus unexpectedly small: {count}");
    assert!(
        failures.is_empty(),
        "literal math fallbacks:\n{}",
        failures.join("\n")
    );
    Ok(())
}

#[test]
fn notebook_plotly_outputs_render_actual_bars_and_heatmap() -> Result<(), Error> {
    let mut normalized = Vec::new();
    fmt::ipynb::to_markdown(include_bytes!("../showcase/03.ipynb"), &mut normalized)?;
    let mut spans = Vec::new();
    fmt::md::scan(&normalized, &mut spans);
    let mut plotly = 0;
    for span in spans {
        let payload = &normalized[span.inner_start as usize..span.inner_end() as usize];
        if span.kind() != fmt::Kind::Chart || !payload.windows(8).any(|w| w == b"\"layout\"") {
            continue;
        }
        for palette in [&DARK, &ii::theme::LIGHT] {
            let mut scene = Scene::with_capacity(8, 4096);
            fmt::dispatch_with_palette(span.kind(), payload, palette, &mut scene)?;
            assert_eq!(scene.svgs.len(), 1);
            let svg = core::str::from_utf8(scene.svg_bytes(&scene.svgs[0])).unwrap();
            assert!(svg.contains("fill=\"none\""));
            if payload.windows(7).any(|w| w == b"heatmap") {
                assert_eq!(svg.matches("class=\"heatmap-cell\"").count(), 36);
                assert!(svg.contains("0.82"));
                assert!(svg.contains("The"));
            }
        }
        plotly += 1;
    }
    assert_eq!(plotly, 2);
    Ok(())
}

#[test]
fn latex_and_mhchem_compile_to_valid_svg() -> Result<(), Error> {
    let mut out_svg = Vec::new();
    tex::compile_latex(b"$$E = mc^2$$", true, &DARK, &mut out_svg)?;
    assert!(out_svg.windows(4).any(|w| w == b"<svg"));

    out_svg.clear();
    tex::compile_latex(b"\\ce{2H2 + O2 -> 2H2O}", true, &DARK, &mut out_svg)?;
    assert!(out_svg.windows(4).any(|w| w == b"<svg"));

    // Corrupt LaTeX degrades honestly to fallback text in Scene without returning Err.
    let mut scene = Scene::with_capacity(8, 256);
    tex::parse_into_scene(b"\\definitely_unknown_cmd{", &mut scene)?;
    assert_eq!(scene.glyphs.len(), 1);

    scene.clear();
    tex::parse_inline_into_scene(b"x^2 + y^2", &mut scene)?;
    assert!(scene.width > 0.0 && scene.height > 0.0);

    scene.clear();
    tex::parse_chem_into_scene(b"H2O <=> H^+ + OH^-", &mut scene)?;
    assert!(scene.width > 0.0 && scene.height > 0.0);
    Ok(())
}

#[test]
fn mermaid_compiles_flowchart_and_rejects_invalid_utf8() -> Result<(), Error> {
    let mut out_svg = Vec::new();
    mermaid::render(b"graph TD\n  A[Start] --> B[End]\n", &DARK, &mut out_svg)?;
    assert!(out_svg.windows(4).any(|w| w == b"<svg"));

    let mut scene = Scene::with_capacity(8, 512);
    mermaid::parse_into_scene(b"graph LR\n  X --> Y\n", &mut scene)?;
    assert!(scene.width > 0.0 && scene.height > 0.0);

    assert_eq!(
        mermaid::render(&[0xFF, 0xFE], &DARK, &mut out_svg),
        Err(Error::MermaidCompile)
    );
    Ok(())
}

#[test]
fn showcase_corpus_dispatches_and_rasterizes_without_panic()
-> Result<(), Box<dyn core::error::Error>> {
    let showcase = Path::new("showcase");
    if !showcase.is_dir() {
        return Ok(());
    }
    let mut scene = Scene::with_capacity(256, 32 * 1024);
    let mut rgba = Vec::new();
    let mut count = 0usize;

    for entry in std::fs::read_dir(showcase)? {
        let path = entry?.path();
        if !path.is_file() {
            continue;
        }
        let ext = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        let Some(kind) = fmt::kind_from_ext(ext) else {
            continue;
        };
        let data = std::fs::read(&path)?;
        if kind == fmt::Kind::Png {
            let (w, h) = raster::decode_png(&data, &mut rgba)?;
            assert_eq!(rgba.len(), usize::try_from(w)? * usize::try_from(h)? * 4);
            count += 1;
            continue;
        }
        scene.clear();
        let res = fmt::dispatch_into_scene(&data, ext, &mut scene);
        assert!(
            res.is_ok(),
            "dispatch failed for {}: {res:?}",
            path.display()
        );

        let (width, height, cols, rows) =
            raster::rasterize_scene(&scene, &DARK, 8, 16, 640, &mut rgba)?;
        assert!(width > 0 && height > 0 && cols > 0 && rows > 0);
        assert_eq!(rgba.len(), usize::from(width) * usize::from(height) * 4);
        count += 1;
    }
    assert!(count > 0);
    Ok(())
}

#[test]
fn math_glyphs_are_self_contained_and_visible_in_both_themes() -> Result<(), Error> {
    for palette in [&DARK, &ii::theme::LIGHT] {
        let mut svg = Vec::new();
        tex::compile_latex(
            br"\alpha + \beta = \sum_{i=1}^{n} x_i",
            true,
            palette,
            &mut svg,
        )?;
        assert!(memchr::memmem::find(&svg, b"<path").is_some());
        assert!(memchr::memmem::find(&svg, b"<text").is_none());
        let mut rgba = Vec::new();
        raster::rasterize(&svg, 9, 18, 720, &mut rgba)?;
        let pixels: Vec<_> = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[3] > 128)
            .collect();
        assert!(!pixels.is_empty(), "mathematical outlines disappeared");
        let expected = palette.color(ii::theme::ColorToken::ACCENT).to_array();
        assert!(
            pixels.iter().any(|p| p[..3] == expected[..3]),
            "wrong foreground palette"
        );
    }
    Ok(())
}

#[test]
fn svg_text_is_drawn_and_xml_declaration_preserves_dimensions() -> Result<(), Error> {
    let svg = br#"<?xml version="1.0"?><svg xmlns="http://www.w3.org/2000/svg" width="240" height="60"><text x="8" y="35" font-family="sans-serif" font-size="24" fill="red">Readable text</text></svg>"#;
    assert_eq!(fmt::svg::extract_svg_dimensions(svg), (240.0, 60.0));
    let mut rgba = Vec::new();
    raster::rasterize(svg, 9, 18, 720, &mut rgba)?;
    assert!(
        rgba.as_chunks::<4>()
            .0
            .iter()
            .filter(|p| p[3] > 128)
            .count()
            > 100,
        "SVG labels disappeared from the raster"
    );
    Ok(())
}

#[test]
fn native_scene_labels_render_distinct_unicode_glyphs() -> Result<(), Error> {
    let mut scene = Scene::with_capacity(1, 32);
    let mut first = Vec::new();
    let mut second = Vec::new();
    for (label, rgba) in [("Ж", &mut first), ("Й", &mut second)] {
        scene.clear();
        scene.width = 80.0;
        scene.height = 60.0;
        scene.push_text(8.0, 40.0, label, ii::theme::ColorToken::TEXT_PRIMARY, 28);
        raster::rasterize_scene(&scene, &DARK, 9, 18, 720, rgba)?;
        assert!(rgba.as_chunks::<4>().0.iter().any(|p| p[3] > 128));
    }
    assert_ne!(
        first, second,
        "Unicode labels collapsed into the same replacement box"
    );
    Ok(())
}

#[test]
fn generated_format_canvases_remain_transparent_in_both_themes() -> Result<(), Error> {
    let inputs: &[(fmt::Kind, &[u8])] = &[
        (fmt::Kind::Mermaid, b"graph TD\nA[Start]-->B[End]\n"),
        (fmt::Kind::Dot, b"digraph { a -> b; }"),
        (fmt::Kind::Tex, br"\frac{x+1}{y+2}"),
        (fmt::Kind::Csv, b"name,value\none,1\ntwo,2\n"),
        (fmt::Kind::Chart, br#"{"data":[{"x":[0,1,2],"y":[2,3,1]}]}"#),
        (fmt::Kind::Archive, include_bytes!("../showcase/20.zip")),
    ];
    for palette in [&DARK, &ii::theme::LIGHT] {
        assert_eq!(palette.color(ii::theme::ColorToken::SURFACE).a(), 0);
        for &(kind, src) in inputs {
            let mut scene = Scene::with_capacity(16, 1024);
            fmt::dispatch_with_palette(kind, src, palette, &mut scene)?;
            let mut rgba = Vec::new();
            let (w, h, _, _) = raster::rasterize_scene(&scene, palette, 9, 18, 720, &mut rgba)?;
            let transparent = rgba.as_chunks::<4>().0.iter().filter(|p| p[3] == 0).count();
            assert!(
                transparent > rgba.len() / 4 / 3,
                "opaque canvas fills content area: {kind:?}"
            );
            let last = (usize::from(w) * usize::from(h) - 1) * 4 + 3;
            assert_eq!(rgba[last], 0, "opaque canvas at bottom: {kind:?}");
            assert!(
                rgba.as_chunks::<4>().0.iter().any(|p| p[3] > 0),
                "empty raster: {kind:?}"
            );
        }
    }
    Ok(())
}

#[test]
fn chemistry_is_compiled_to_glyphs_not_literal_fallback() -> Result<(), Error> {
    for src in [
        br"\ce{2H2 + O2 -> 2H2O}".as_slice(),
        br"\ce{NH3 + H2O <=> NH4^+ + OH^-}",
        br"\ce{2 H2 (g) + O2 (g) <=>[\Delta][\text{Pt}] 2 H2O (l)} + \ce{CuSO4.5H2O} + \ce{^14_6C}",
    ] {
        let mut scene = Scene::with_capacity(4, 4096);
        fmt::dispatch_with_palette(fmt::Kind::Chem, src, &DARK, &mut scene)?;
        assert!(
            !scene.svgs.is_empty(),
            "chemistry fell back to source: {}",
            String::from_utf8_lossy(src)
        );
    }
    Ok(())
}

#[test]
fn vector_scale_matches_reference_dpi_and_fits_narrow_viewports() -> Result<(), Error> {
    let mut scene = Scene::with_capacity(4, 256);
    fmt::dispatch_kind(fmt::Kind::Svg, br#"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="60"><path d="M5 5L95 55" stroke="red"/></svg>"#,&mut scene)?;
    let mut rgba = Vec::new();
    let (normal, _, _, _) = raster::rasterize_scene(&scene, &DARK, 9, 18, 720, &mut rgba)?;
    let (retina, h, cols, rows) = raster::rasterize_scene(&scene, &DARK, 18, 36, 1440, &mut rgba)?;
    assert!(f32::from(retina) >= f32::from(normal) * 1.6);
    assert_eq!(
        u32::from(retina) * u32::from(h) * 4,
        u32::try_from(rgba.len()).map_err(|_| Error::ResourceLimit)?
    );
    assert!(cols <= 12 && rows <= 4);
    let (narrow, _, _, _) = raster::rasterize_scene(&scene, &DARK, 18, 36, 90, &mut rgba)?;
    assert!(narrow <= 90);
    for (width, height, expected) in [(1.0, 4000.0, (1, 10)), (4000.0, 1.0, (8, 1))] {
        let layout = raster::scale_down_and_snap(width, height, 8, 10, 9, 18)?;
        assert_eq!((layout.grid_cols, layout.grid_rows), expected);
        assert_eq!(layout.snap_w, 9 * u32::from(expected.0));
        assert_eq!(layout.snap_h, 18 * u32::from(expected.1));
    }
    let extreme = raster::scale_down_and_snap(f32::MAX, f32::MAX, 80, 10, 9, 18)?;
    assert!(extreme.scale > 0.0);
    assert_eq!((extreme.grid_cols, extreme.grid_rows), (20, 10));
    Ok(())
}

#[test]
fn simple_inline_math_fits_one_terminal_row() -> Result<(), Error> {
    use ii::compose::{ComposeCtx, ComposeOut, ComposeScratch, compose_document};
    let mut scratch = ComposeScratch::new();
    let mut out = ComposeOut::new();
    for (cell_w, cell_h) in [(4, 8), (9, 18), (18, 36)] {
        compose_document(
            br"Before $\lambda$ after",
            ComposeCtx {
                cell_w,
                cell_h,
                cols: 40,
                rows: 12,
                max_w: 40 * cell_w,
                palette: &DARK,
                is_tmux: false,
                is_remote: true,
                skip_lines: 0,
                max_lines: 0,
                explicit_graphics: false,
            },
            None,
            &mut scratch,
            &mut 0,
            1,
            &mut out,
        )?;
        assert_eq!(out.extmarks.len(), 1);
        assert_eq!(
            out.extmarks[0].rows, 1,
            "inline glyph at cell height {cell_h}"
        );
        assert_eq!(
            memchr::memchr(b'\n', &out.ansi),
            None,
            "formula split the prose"
        );
    }
    Ok(())
}

#[test]
fn diagram_surfaces_follow_transparent_theme_including_quadrants() -> Result<(), Error> {
    for source in [
        b"sequenceDiagram\nparticipant A\nparticipant B\nNote over A,B: Context\nA->>B: Request\nactivate B\nB-->>A: Response\ndeactivate B\n".as_slice(),
        b"quadrantChart\nx-axis Low --> High\ny-axis Low --> High\nquadrant-1 Q1\nquadrant-2 Q2\nquadrant-3 Q3\nquadrant-4 Q4\nA: [0.7, 0.7]\n",
    ] {
        for palette in [&DARK,&ii::theme::LIGHT] {
            assert_eq!(palette.svg_surface_fill(),"none");
            let mut scene=Scene::with_capacity(8,4096);
            mermaid::parse_with_palette(source,palette,&mut scene)?;
            let mut rgba=Vec::new();
            raster::rasterize_scene(&scene,palette,9,18,720,&mut rgba)?;
            let clear=rgba.as_chunks::<4>().0.iter().filter(|p|p[3]==0).count();
            assert!(clear>rgba.len()/4*3/4,"opaque diagram surface");
        }
    }
    Ok(())
}
