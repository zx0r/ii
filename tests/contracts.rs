//! Regression checks for source lifetime, wire boundaries and presentation anchors.
#[test]
fn source_anchors_are_independent_of_image_height() -> Result<(), Box<dyn std::error::Error>> {
    use ii::{
        compose::{self, ComposeCtx, ComposeOut, ComposeScratch},
        theme,
    };
    let src = b"before $x$ after\n\n$$y$$\nend";
    let ctx = ComposeCtx {
        cell_w: 8,
        cell_h: 16,
        cols: 80,
        rows: 24,
        max_w: 640,
        palette: &theme::LIGHT,
        is_tmux: false,
        is_remote: true,
        skip_lines: 0,
        max_lines: 0,
    };
    let mut scratch = ComposeScratch::new();
    let mut out = ComposeOut::new();
    compose::compose_document(src, ctx, None, &mut scratch, &mut 0, 1, &mut out)?;
    assert_eq!(out.extmarks.len(), 2);
    let first = out.extmarks[0];
    assert_eq!(first.start_row, 0);
    assert_eq!(first.start_col, 7);
    let second = out.extmarks[1];
    assert_eq!(second.start_row, 2);
    assert!(out.ansi.windows(6).any(|s| s == b"before"));
    assert!(out.ansi.windows(5).any(|s| s == b"after"));
    let mut bytes = b"a\nb\nc\n".to_vec();
    compose::crop_lines(&mut bytes, 1, 1);
    assert_eq!(bytes, b"b\n");
    Ok(())
}

#[test]
fn cli_pipe_is_text_and_end_of_options_accepts_a_file() -> Result<(), Box<dyn std::error::Error>> {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_ii"))
        .args(["--size", "80x2", "--", "showcase/01.md"])
        .output()?;
    assert!(output.status.success());
    assert!(!output.stdout.contains(&0));
    assert!(memchr::memchr_iter(b'\n', &output.stdout).count() <= 2);
    Ok(())
}

#[test]
fn json_strings_decode_surrogates_and_roll_back_invalid_escapes() {
    let mut out = b"prefix".to_vec();
    assert!(ii::fmt::json::unescape_str(
        br#""\uD83D\uDE00\n\u0410""#,
        &mut out
    ));
    assert_eq!(&out[6..], "😀\nА".as_bytes());
    let len = out.len();
    for raw in [
        br#""bad\q""#.as_slice(),
        br#""\uD800""#,
        br#""\uDC00""#,
        br#""\uXX00""#,
    ] {
        assert!(!ii::fmt::json::unescape_str(raw, &mut out));
        assert_eq!(out.len(), len);
    }
}

#[test]
fn visual_paging_respects_wide_and_combining_characters() {
    let mut out = Vec::new();
    ii::viewport::append("ab界c\ne\u{301}fg\n".as_bytes(), 4, 1, 2, &mut out);
    assert_eq!(out, "c\ne\u{301}fg\n".as_bytes());
}

#[test]
fn every_graphics_row_selector_has_zero_width() -> Result<(), Box<dyn std::error::Error>> {
    for mark in ii::kitty::DIACRITICS_UTF8 {
        let ch = std::str::from_utf8(mark)?
            .chars()
            .next()
            .ok_or("empty mark")?;
        assert_eq!(ii::viewport::cells(ch), 0, "{ch:?}");
    }
    let mut image = Vec::new();
    ii::kitty::emit_placeholder(0x0012_3456, 80, 297, &mut image);
    let mut page = Vec::new();
    ii::viewport::append(&image, 80, 290, 7, &mut page);
    assert_eq!(memchr::memchr_iter(b'\n', &page).count(), 7);
    assert_eq!(
        memchr::memmem::find_iter(&page, &"\u{10eeee}".as_bytes()).count(),
        7 * 80
    );
    Ok(())
}

#[test]
fn math_delimiters_and_fence_lengths_are_not_lost() {
    let mut spans = Vec::new();
    ii::fmt::md::scan(br"before \(x+1\) middle \[y+2\] after", &mut spans);
    assert_eq!(spans.len(), 2);
    ii::fmt::md::scan(b"````mermaid\ngraph TD\n```\nA-->B\n````\n", &mut spans);
    assert_eq!(spans.len(), 1);
    assert_eq!(
        &b"````mermaid\ngraph TD\n```\nA-->B\n````\n"
            [spans[0].inner_start as usize..spans[0].inner_end() as usize],
        b"graph TD\n```\nA-->B\n"
    );
}

#[test]
fn notebook_png_mime_survives_normalization_and_scanning() -> Result<(), Box<dyn std::error::Error>>
{
    let mut normalized = Vec::new();
    ii::fmt::ipynb::to_markdown(
        include_bytes!("../showcase/scenarios/analysis.ipynb"),
        &mut normalized,
    )?;
    let mut spans = Vec::new();
    ii::fmt::md::scan(&normalized, &mut spans);
    let png = spans
        .iter()
        .find(|s| s.kind() == ii::fmt::Kind::Png)
        .ok_or("PNG MIME was discarded")?;
    let data = &normalized[png.inner_start as usize..png.inner_end() as usize];
    // The fence adds a newline; the PNG itself must remain byte-for-byte intact.
    let expected = include_bytes!("../showcase/scenarios/incident-heatmap.png");
    assert_eq!(&data[..expected.len()], expected);
    let mut rgba = Vec::new();
    ii::raster::decode_png(data, &mut rgba)?;
    assert!(rgba.as_chunks::<4>().0.iter().any(|p| p[3] > 0));
    Ok(())
}

#[test]
fn math_environments_and_inline_code_follow_historical_contract() {
    let src = br"before \begin{aligned} a &= b \\ c &= d \end{aligned} after";
    let mut spans = Vec::new();
    ii::fmt::md::scan(src, &mut spans);
    assert_eq!(spans.len(), 1);
    assert_eq!(spans[0].kind(), ii::fmt::Kind::Tex);
    ii::fmt::md::scan(b"$a `code` b$", &mut spans);
    assert_eq!(spans.len(), 0);
}

#[test]
fn numeric_math_is_not_misclassified_as_currency() {
    let mut spans = Vec::new();
    ii::fmt::md::scan(
        b"$100 per month, $2f + 1$ nodes and $2 + 3$; $42$.",
        &mut spans,
    );
    assert_eq!(spans.len(), 3);
    for span in spans {
        assert_eq!(span.kind(), ii::fmt::Kind::TexInline);
    }
}
