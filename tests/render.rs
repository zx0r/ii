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
        let data = std::fs::read(&path)?;
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
