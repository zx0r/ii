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
