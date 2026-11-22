//! Pure Sans-I/O format parser & scanner tests (Miri + Tree Borrows clean).

use proptest::{
    collection::vec as prop_vec,
    prelude::any,
    test_runner::{Config, RngAlgorithm, TestRng, TestRunner},
};

use ii::{
    err::Error,
    fmt::{self, Kind, archive, chart, csv, dot, font, ipynb, json, md, svg},
    scene::Scene,
    theme::{DARK, LIGHT},
};

fn runner() -> TestRunner {
    let cases = if cfg!(miri) { 16 } else { 128 };
    let cfg = Config {
        cases,
        failure_persistence: None,
        ..Config::default()
    };
    TestRunner::new_with_rng(cfg, TestRng::deterministic_rng(RngAlgorithm::ChaCha))
}

fn build_synthetic_zip(entry_name: &[u8], body: &[u8]) -> Vec<u8> {
    let name_len = u16::try_from(entry_name.len()).unwrap_or(0);
    let body_len = u32::try_from(body.len()).unwrap_or(0);

    let mut zip = Vec::new();
    zip.extend_from_slice(b"PK\x03\x04");
    zip.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    zip.extend_from_slice(&0u32.to_le_bytes());
    zip.extend_from_slice(&body_len.to_le_bytes());
    zip.extend_from_slice(&body_len.to_le_bytes());
    zip.extend_from_slice(&name_len.to_le_bytes());
    zip.extend_from_slice(&0u16.to_le_bytes());
    zip.extend_from_slice(entry_name);
    zip.extend_from_slice(body);

    let cd_offset = u32::try_from(zip.len()).unwrap_or(0);
    zip.extend_from_slice(b"PK\x01\x02");
    zip.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
    zip.extend_from_slice(&0u32.to_le_bytes());
    zip.extend_from_slice(&body_len.to_le_bytes());
    zip.extend_from_slice(&body_len.to_le_bytes());
    zip.extend_from_slice(&name_len.to_le_bytes());
    zip.extend_from_slice(&[0u8; 16]);
    zip.extend_from_slice(entry_name);

    let cd_size = u32::try_from(zip.len()).unwrap_or(0) - cd_offset;
    zip.extend_from_slice(b"PK\x05\x06");
    zip.extend_from_slice(&[0u8; 4]);
    zip.extend_from_slice(&1u16.to_le_bytes());
    zip.extend_from_slice(&1u16.to_le_bytes());
    zip.extend_from_slice(&cd_size.to_le_bytes());
    zip.extend_from_slice(&cd_offset.to_le_bytes());
    zip.extend_from_slice(&0u16.to_le_bytes());
    zip
}

fn build_synthetic_tar(entry_name: &[u8], body: &[u8]) -> Vec<u8> {
    let mut tar = vec![0u8; 1024];
    let nlen = entry_name.len().min(100);
    tar[..nlen].copy_from_slice(&entry_name[..nlen]);
    let oct = format!("{:011o}", body.len());
    tar[124..135].copy_from_slice(oct.as_bytes());
    tar[156] = b'0';
    tar[257..262].copy_from_slice(b"ustar");
    let blen = body.len().min(512);
    tar[512..512 + blen].copy_from_slice(&body[..blen]);
    tar
}

#[test]
fn fmt_mod_detection_and_formatters() {
    assert_eq!(fmt::detect_kind(b"", "qmd"), Kind::Md);
    assert_eq!(fmt::detect_kind(b"", "TYP"), Kind::Md);
    assert_eq!(fmt::detect_kind(b"", "ipynb"), Kind::Ipynb);
    assert_eq!(fmt::detect_kind(b"", "latex"), Kind::Tex);
    assert_eq!(fmt::detect_kind(b"", "mmd"), Kind::Mermaid);
    assert_eq!(fmt::detect_kind(b"", "gv"), Kind::Dot);
    assert_eq!(fmt::detect_kind(b"", "tsv"), Kind::Tsv);
    assert_eq!(fmt::detect_kind(b"", "vl"), Kind::Chart);
    assert_eq!(fmt::detect_kind(b"", "plotly"), Kind::Chart);
    assert_eq!(fmt::detect_kind(b"", "whl"), Kind::Archive);
    assert_eq!(fmt::detect_kind(b"", "tzst"), Kind::Archive);

    assert_eq!(fmt::detect_kind(b"  <svg width=\"10\"/>", ""), Kind::Svg);
    assert_eq!(fmt::detect_kind(b"digraph G { a -> b }", ""), Kind::Dot);
    assert_eq!(fmt::detect_kind(b"{\"cells\":[]}", ""), Kind::Ipynb);
    assert_eq!(fmt::detect_kind(b"{\"mark\":\"bar\"}", ""), Kind::Chart);
    assert_eq!(fmt::detect_kind(b"\x1f\x8b\x08\x00", ""), Kind::Archive);

    let mut out = Vec::new();
    fmt::push_u32(&mut out, 0);
    out.push(b',');
    fmt::push_u64(&mut out, 123_456_789);
    out.push(b',');
    fmt::push_f64_1(&mut out, -12.36);
    out.push(b',');
    fmt::push_f64_1(&mut out, f64::NAN);
    assert_eq!(out, b"0,123456789,-12.4,0.0");

    out.clear();
    fmt::write_xml_escaped(b"a<b&c>\"'d", &mut out);
    assert_eq!(out, b"a&lt;b&amp;c&gt;&quot;&apos;d");

    assert!(font::measure_str("WWW", 14.0) > font::measure_str("iii", 14.0));
    assert!(font::measure_bytes("αβ".as_bytes(), 14.0) > 20.0);
}

#[test]
fn json_navigator_nested_escapes_and_depth_guard() {
    let doc = br#"{"a": 42.5, "nested": {"k": "hi\n\u03bb\"\\"}, "arr": [1, {"x": 2}, "three"]}"#;
    assert_eq!(json::get_field(doc, b"missing"), None);
    let num = json::get_field(doc, b"a").and_then(json::parse_f64);
    assert!(num.is_some_and(|v| (v - 42.5).abs() < 1e-6));

    let nested = json::get_field(doc, b"nested").unwrap_or(&[]);
    let raw_k = json::get_field(nested, b"k").unwrap_or(&[]);
    let mut unescaped = Vec::new();
    assert!(json::unescape_str(raw_k, &mut unescaped));
    assert_eq!(unescaped, "hi\nλ\"\\".as_bytes());

    let arr = json::get_field(doc, b"arr").unwrap_or(&[]);
    let items: Vec<&[u8]> = json::iter_array(arr).collect();
    assert_eq!(items.len(), 3);
    assert_eq!(items[0], b"1");
    assert_eq!(items[2], b"\"three\"");

    let mut deep = vec![b'['; 128];
    deep.extend(core::iter::repeat_n(b']', 128));
    assert_eq!(json::skip_value(&deep, 0, 0), None);
}

#[test]
fn csv_tsv_and_pipe_table_scene_emission() -> Result<(), Error> {
    let mut scene = Scene::with_capacity(16, 256);
    csv::parse_into_scene(b"name,val\n\"a,b\",10\nc,20\n", &mut scene)?;
    assert_eq!(scene.grids.len(), 1);
    assert_eq!((scene.grids[0].rows, scene.grids[0].cols), (3, 2));
    assert_eq!(scene.cell_text(&scene.cells[2]), "a,b");

    scene.clear();
    csv::parse_tsv_into_scene(b"k\tv\nx\t1\n", &mut scene)?;
    assert_eq!((scene.grids[0].rows, scene.grids[0].cols), (2, 2));
    assert_eq!(scene.cell_text(&scene.cells[3]), "1");

    scene.clear();
    csv::parse_into_scene(b"| H1 | H2 |\n|---|---|\n| 1 | 2 |\n", &mut scene)?;
    assert_eq!((scene.grids[0].rows, scene.grids[0].cols), (2, 2));
    assert_eq!(scene.cell_text(&scene.cells[0]), "H1");
    assert_eq!(scene.cell_text(&scene.cells[3]), "2");
    Ok(())
}

#[test]
fn archive_zip_tar_and_compressed_containers() -> Result<(), Error> {
    let zip = build_synthetic_zip(b"pkg/mod.py", b"print(1)\n");
    assert!(archive::looks_like_archive(&zip));
    let mut scene = Scene::with_capacity(16, 256);
    archive::parse_into_scene(&zip, &mut scene)?;
    assert_eq!(scene.grids.len(), 1);
    assert_eq!(scene.grids[0].rows, 2);
    assert_eq!(scene.cell_text(&scene.cells[3]), "pkg/mod.py");

    let tar = build_synthetic_tar(b"README.md", b"# hi");
    assert!(archive::looks_like_archive(&tar));
    scene.clear();
    archive::parse_into_scene(&tar, &mut scene)?;
    assert_eq!(scene.grids[0].rows, 2);
    assert_eq!(scene.cell_text(&scene.cells[3]), "README.md");

    for (magic, expected_label) in [
        (
            &b"\x1f\x8b\x08\x00\x00\x00\x00\x00\x00\x03\x10\x00\x00\x00"[..],
            "tar.gz (gzip)",
        ),
        (&b"BZh91AY&SY"[..], "tar.bz2 (bzip2)"),
        (&b"\xfd7zXZ\x00\x00"[..], "tar.xz (xz)"),
        (&b"\x28\xb5\x2f\xfd\x00"[..], "tar.zst (zstd)"),
    ] {
        assert!(archive::looks_like_archive(magic));
        scene.clear();
        archive::parse_into_scene(magic, &mut scene)?;
        assert_eq!(scene.cell_text(&scene.cells[3]), expected_label);
    }
    Ok(())
}

#[test]
fn svg_dot_chart_and_ipynb_emit_valid_scenes() -> Result<(), Error> {
    let raw_svg = br##"<svg viewBox="0 0 120 60"><path stroke="currentColor" fill="#000"/></svg>"##;
    let mut rewritten = Vec::new();
    svg::rewrite_colors(raw_svg, &DARK, &mut rewritten);
    assert!(rewritten.windows(7).any(|w| {
        w == DARK
            .hex_str(ii::theme::SemanticRole::TextPrimary)
            .as_bytes()
    }));
    assert_eq!(svg::extract_svg_dimensions(raw_svg), (120.0, 60.0));

    let mut scene = Scene::with_capacity(32, 1024);
    svg::parse_into_scene(raw_svg, &mut scene)?;
    assert!((scene.width - 120.0).abs() < 1e-3);
    assert_eq!(
        svg::parse_into_scene(b"not-an-svg", &mut scene),
        Err(Error::SvgParse)
    );

    let dot_src = b"digraph G { a [label=\"Start\"]; a -> b; b -> c; c -> a; }";
    let mut dot_svg = Vec::new();
    dot::render(dot_src, &LIGHT, 400, &mut dot_svg);
    assert!(dot_svg.starts_with(b"<svg"));
    assert!(dot_svg.ends_with(b"</svg>"));
    scene.clear();
    dot::parse_into_scene(dot_src, &mut scene)?;
    assert_eq!(scene.rects.len(), 3);
    assert_eq!(scene.paths.len(), 3);

    let vl = br#"{"mark":"bar","data":{"values":[{"x":1,"y":10},{"x":2,"y":25}]}}"#;
    scene.clear();
    chart::parse_into_scene(vl, &mut scene)?;
    assert_eq!(scene.rects.len(), 3);

    let plotly = br#"{"data":[{"type":"scatter","x":[0,1,2],"y":[3.0,5.5,2.0]}]}"#;
    scene.clear();
    chart::parse_into_scene(plotly, &mut scene)?;
    assert_eq!(scene.paths.len(), 1);
    assert_eq!(
        chart::parse_into_scene(b"{}", &mut scene),
        Err(Error::JsonNav)
    );

    let nb = br##"{
        "cells": [
            {"cell_type": "markdown", "source": ["# Title\n", "Body"]},
            {"cell_type": "code", "source": ["x = 1\n"], "outputs": [
                {"output_type": "stream", "text": ["1\n"]},
                {"output_type": "display_data", "data": {"image/svg+xml": ["<svg width=\"40\" height=\"20\"/>"]}}
            ]}
        ]
    }"##;
    scene.clear();
    ipynb::parse_into_scene(nb, &mut scene)?;
    assert_ne!(scene.glyphs, []);
    assert_ne!(scene.rects, []);
    assert_eq!(
        ipynb::parse_into_scene(b"{}", &mut scene),
        Err(Error::JsonNav)
    );
    Ok(())
}

#[test]
fn markdown_scanner_frontmatter_fences_inline_math_and_prop_fuzz() {
    let doc = concat!(
        "---\ntitle: test\n---\n# H1\n",
        "Inline $a+b$ and `$not_math$` and \\$5 + $10.\n\n",
        "$\x24\nx^2\n$\x24\n",
        "```dot\na -> b\n```\n",
        "| a | b |\n|---|---|\n| 1 | 2 |\n"
    )
    .as_bytes();
    let mut spans = Vec::new();
    md::scan(doc, &mut spans);

    let kinds: Vec<Kind> = spans.iter().map(|sp| sp.kind).collect();
    assert_eq!(kinds, [Kind::TexInline, Kind::Tex, Kind::Dot, Kind::Csv]);
    assert_eq!(
        &doc[spans[0].inner_start as usize..spans[0].inner_end as usize],
        b"a+b"
    );

    let res = runner().run(&prop_vec(any::<u8>(), 0..512), |bytes| {
        let mut local_spans = Vec::new();
        let mut scene = Scene::with_capacity(64, 1024);
        md::scan(&bytes, &mut local_spans);
        let _ = csv::parse_into_scene(&bytes, &mut scene);
        scene.clear();
        let _ = archive::parse_into_scene(&bytes, &mut scene);
        scene.clear();
        let _ = dot::parse_into_scene(&bytes, &mut scene);
        scene.clear();
        let _ = chart::parse_into_scene(&bytes, &mut scene);
        scene.clear();
        let _ = ipynb::parse_into_scene(&bytes, &mut scene);
        Ok(())
    });
    assert!(res.is_ok(), "{res:?}");
}

#[test]
fn markdown_zero_tofu_and_gfm_callouts() {
    let md_text = concat!(
        "# Heading 1\n",
        "## Heading 2\n",
        "### Heading 3\n",
        "- [x] Completed task\n",
        "- [ ] Pending task\n",
        "- Standard list item\n",
        "---\n",
        "> [!NOTE]\n",
        "> Note body\n",
        "> [!TIP]\n",
        "> Tip body\n",
        "> [!WARNING]\n",
        "> Warning body\n",
        "> [!CAUTION]\n",
        "> Caution body\n",
        "> Regular blockquote\n"
    )
    .as_bytes();

    let mut scene = Scene::with_capacity(64, 1024);
    assert_eq!(md::parse_into_scene(md_text, &mut scene), Ok(()));
    assert!(scene.glyphs.len() >= 10);
    assert!(scene.width > 0.0);
    assert!(scene.height > 0.0);

    // Verify Zero-Tofu glyph prefixes in text_pool
    let text_pool_str = core::str::from_utf8(&scene.text_pool).unwrap_or("");
    assert!(text_pool_str.contains("▌ "));
    assert!(text_pool_str.contains("▎ "));
    assert!(text_pool_str.contains("▪ "));
    assert!(text_pool_str.contains("✓ "));
    assert!(text_pool_str.contains("○ "));
    assert!(text_pool_str.contains("• "));
    assert!(text_pool_str.contains("ℹ NOTE"));
    assert!(text_pool_str.contains("◆ TIP"));
    assert!(text_pool_str.contains("▲ WARNING"));
    assert!(text_pool_str.contains("✕ CAUTION"));
}
