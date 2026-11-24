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
        explicit_graphics: false,
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
    let src = b"An inline ~~~ run.\n  ~~~~text\n$x$\n~~~\n$y$\n  ~~~~\n~~~math\nz^2\n~~~\n";
    ii::fmt::md::scan(src, &mut spans);
    assert_eq!(spans.len(), 1);
    assert_eq!(
        &src[spans[0].inner_start as usize..spans[0].inner_end() as usize],
        b"z^2\n"
    );
}

#[test]
fn notebook_png_mime_survives_normalization_and_scanning() -> Result<(), Box<dyn std::error::Error>>
{
    let notebook = br#"{"cells":[{"cell_type":"code","outputs":[{"data":{"image/png":"iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR4nGP4X8/wHwAGfQJ+ZSLZFwAAAABJRU5ErkJggg=="}}]}]}"#;
    let mut normalized = Vec::new();
    ii::fmt::ipynb::to_markdown(notebook, &mut normalized)?;
    let mut spans = Vec::new();
    ii::fmt::md::scan(&normalized, &mut spans);
    let png = spans
        .iter()
        .find(|s| s.kind() == ii::fmt::Kind::Png)
        .ok_or("PNG MIME was discarded")?;
    let data = &normalized[png.inner_start as usize..png.inner_end() as usize];
    let mut rgba = Vec::new();
    assert_eq!(ii::raster::decode_png(data, &mut rgba)?, (1, 1));
    assert_eq!(rgba, [255, 127, 0, 255]);
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

#[test]
fn native_table_respects_viewport_width() {
    let mut out = Vec::new();
    ii::style::emit_prose(b"| Long header | Other header |\n|---|---|\n| very long content here | more content here |\n",&ii::theme::DARK,&mut ii::style::State::with_width(24),&mut out);
    let mut page = Vec::new();
    ii::viewport::append(&out, 24, 0, 0, &mut page);
    assert_eq!(
        memchr::memchr_iter(b'\n', &out).count(),
        memchr::memchr_iter(b'\n', &page).count()
    );
}

#[test]
fn fenced_code_has_keyword_string_and_number_highlights() {
    let mut out = Vec::new();
    ii::style::emit_prose(
        b"```python\nimport pandas as pd\nx = 42\nname = \"sample\"\n```\n",
        &ii::theme::DARK,
        &mut ii::style::State::default(),
        &mut out,
    );
    for token in [b"import".as_slice(), b"42", b"sample"] {
        assert!(memchr::memmem::find(&out, token).is_some());
    }
    assert!(memchr::memchr_iter(0x1b, &out).count() > 10);
}

#[test]
fn commonmark_inline_resolves_nested_markup_entities_and_reference_links() {
    let mut out = Vec::new();
    ii::style::emit_prose(
        b"**bold _nested_** &amp; [reference][key] and `**literal**`\n\n[key]: https://example.org\n",
        &ii::theme::DARK,
        &mut ii::style::State::default(),
        &mut out,
    );
    assert!(memchr::memmem::find(&out, b"\x1b[3mnested\x1b[23m").is_some());
    assert!(memchr::memmem::find(&out, b"&amp;").is_none());
    assert!(memchr::memmem::find(&out, b"[reference][key]").is_none());
    assert!(memchr::memmem::find(&out, b"**literal**").is_some());
}

#[test]
fn native_fences_preserve_shorter_delimiters_and_tables_accept_optional_pipes() {
    let mut out = Vec::new();
    ii::style::emit_prose(
        b"````md\n```\n| not | table |\n| --- | --- |\n````\n\nName | Value\n--- | ---\na\\|b | 42\n",
        &ii::theme::DARK,
        &mut ii::style::State::with_width(40),
        &mut out,
    );
    assert!(memchr::memmem::find(&out, b"```").is_some());
    assert_eq!(memchr::memmem::find_iter(&out, "┌".as_bytes()).count(), 1);
    assert!(memchr::memmem::find(&out, b"a|b").is_some());
}

#[test]
fn long_table_cells_fit_without_source_length_dependent_shrinking() {
    let mut source = b"| Name | Value |\n|---|---|\n| ".to_vec();
    source.extend(std::iter::repeat_n(b'x', 1_000_000));
    source.extend_from_slice(b" | 42 |\n");
    let mut out = Vec::new();
    ii::style::emit_prose(
        &source,
        &ii::theme::DARK,
        &mut ii::style::State::with_width(40),
        &mut out,
    );
    assert!(out.len() < 4096);
    let mut wrapped = Vec::new();
    ii::viewport::append(&out, 40, 0, 0, &mut wrapped);
    assert_eq!(
        memchr::memchr_iter(b'\n', &out).count(),
        memchr::memchr_iter(b'\n', &wrapped).count()
    );
}

#[test]
fn code_frame_and_wrapped_rails_fit_narrow_preview() {
    for cols in [16, 24, 40, 80] {
        let mut out = Vec::new();
        ii::style::emit_prose(b"```python-with-a-very-long-language-label\nprint(\"a deliberately long code line which must wrap\")\n```\n",&ii::theme::DARK,&mut ii::style::State::with_width(cols),&mut out);
        let mut wrapped = Vec::new();
        ii::viewport::append(&out, cols, 0, 0, &mut wrapped);
        assert_eq!(
            memchr::memchr_iter(b'\n', &out).count(),
            memchr::memchr_iter(b'\n', &wrapped).count(),
            "border exceeded {cols} columns"
        );
        assert!(memchr::memmem::find_iter(&out, "│ ".as_bytes()).count() >= 1);
        assert_eq!(memchr::memmem::find_iter(&out, "╭".as_bytes()).count(), 1);
        assert_eq!(memchr::memmem::find_iter(&out, "╰".as_bytes()).count(), 1);
    }
}

#[test]
fn qmd_table_with_math_keeps_all_rows_in_one_native_block() -> Result<(), Box<dyn std::error::Error>>
{
    use ii::compose::{self, ComposeCtx, ComposeOut, ComposeScratch};
    let src = include_bytes!("../showcase/02.qmd");
    let start =
        memchr::memmem::find(src, b"| Parameter | Mean").ok_or("table missing from fixture")?;
    let end =
        start + memchr::memmem::find(&src[start..], b"\n\n").ok_or("table terminator missing")? + 1;
    let ctx = ComposeCtx {
        cell_w: 9,
        cell_h: 18,
        cols: 80,
        rows: 40,
        max_w: 720,
        palette: &ii::theme::DARK,
        is_tmux: false,
        is_remote: true,
        skip_lines: 0,
        max_lines: 0,
        explicit_graphics: false,
    };
    let mut out = ComposeOut::new();
    compose::compose_document(
        src,
        ctx,
        None,
        &mut ComposeScratch::new(),
        &mut 0,
        1,
        &mut out,
    )?;
    let block = out
        .blocks
        .iter()
        .find(|&&(a, b, _, _)| a == start && b == end)
        .ok_or("math spans split the native table after its header")?;
    let table = &out.ansi[block.2..block.3];
    for value in [
        b"0.452".as_slice(),
        b"1.91",
        b"3.18",
        b"1.42",
        b"0.31",
        b"Reporting rate",
    ] {
        assert!(
            memchr::memmem::find(table, value).is_some(),
            "missing body cell {:?}",
            String::from_utf8_lossy(value)
        );
    }
    assert_eq!(memchr::memmem::find_iter(table, "┌".as_bytes()).count(), 1);
    assert_eq!(memchr::memmem::find_iter(table, "└".as_bytes()).count(), 1);
    for row in table.split(|&b| b == b'\n').filter(|row| {
        [b"0.452".as_slice(), b"1.91", b"3.18", b"1.42"]
            .iter()
            .any(|number| memchr::memmem::find(row, number).is_some())
    }) {
        assert!(
            memchr::memmem::find(row, "\u{10eeee}".as_bytes()).is_some(),
            "missing inline graphic in {}",
            String::from_utf8_lossy(row)
        );
    }
    let table_row = memchr::memchr_iter(b'\n', &src[..start]).count();
    for row in table_row + 2..table_row + 6 {
        assert!(
            out.extmarks
                .iter()
                .any(|mark| usize::from(mark.start_row) == row && mark.rows == 1)
        );
    }
    assert!(
        out.extmarks
            .iter()
            .any(|mark| usize::from(mark.start_row) < table_row)
    );
    compose::compose_document(
        b"| Term | Example |\n| --- | --- |\n| **$x$** and `$x$` | $x$ |\n",
        ctx,
        None,
        &mut ComposeScratch::new(),
        &mut 0,
        1,
        &mut out,
    )?;
    assert_eq!(
        out.uploads.len(),
        1,
        "repeated table graphic was transferred twice"
    );
    assert_eq!(out.extmarks.len(), 2, "code span was rendered as math");
    assert!(
        memchr::memmem::find(&out.ansi, b"**").is_none(),
        "strong emphasis leaked markup"
    );
    Ok(())
}

#[test]
fn inline_image_row_moves_whole_to_next_visual_line() {
    let mut text = b"1234567".to_vec();
    ii::kitty::emit_placeholder(42, 4, 1, &mut text);
    let mut page = Vec::new();
    ii::viewport::append(&text, 10, 0, 0, &mut page);
    let counts: Vec<_> = page
        .split(|&b| b == b'\n')
        .map(|line| memchr::memmem::find_iter(line, "\u{10eeee}".as_bytes()).count())
        .filter(|&n| n > 0)
        .collect();
    assert_eq!(counts, [4], "formula image was sliced across terminal rows");
}

#[test]
fn mdx_module_imports_are_hidden_without_moving_source_or_hiding_code_samples()
-> Result<(), Box<dyn std::error::Error>> {
    let src=b"import { LightCurve } from '../components/LightCurve'\nimport {\n  A, B\n} from './tools'\nexport const config = {\n mode: 'dark'\n}\n\n# Transit\n```js\nimport { keep } from './example'\n```\n";
    let mut out = Vec::new();
    ii::fmt::mdx::presentation_source(src, &mut out)?;
    assert_eq!(out.len(), src.len());
    assert_eq!(
        memchr::memchr_iter(b'\n', &out).collect::<Vec<_>>(),
        memchr::memchr_iter(b'\n', src).collect::<Vec<_>>()
    );
    assert!(memchr::memmem::find(&out, b"LightCurve").is_none());
    assert!(memchr::memmem::find(&out, b"mode: 'dark'").is_none());
    assert!(memchr::memmem::find(&out, b"# Transit").is_some());
    assert!(memchr::memmem::find(&out, b"import { keep }").is_some());
    assert!(src.starts_with(b"import { LightCurve }"));
    Ok(())
}

#[test]
fn frontmatter_rules_match_each_split_width() {
    for cols in [24, 40, 79, 120] {
        let mut out = Vec::new();
        ii::style::emit_prose(
            b"---\ntitle: Study\n---\n",
            &ii::theme::DARK,
            &mut ii::style::State::with_width(cols),
            &mut out,
        );
        let rules: Vec<_> = out
            .split(|&b| b == b'\n')
            .map(|line| memchr::memmem::find_iter(line, "─".as_bytes()).count())
            .filter(|&count| count > 0)
            .collect();
        assert_eq!(rules, [usize::from(cols), usize::from(cols)]);
        let mut wrapped = Vec::new();
        ii::viewport::append(&out, cols, 0, 0, &mut wrapped);
        assert_eq!(
            memchr::memchr_iter(b'\n', &out).count(),
            memchr::memchr_iter(b'\n', &wrapped).count()
        );
    }
}

#[test]
fn borders_and_frontmatter_share_the_theme_muted_color() {
    use ii::theme::{DARK, LIGHT, SemanticRole};
    for palette in [&DARK, &LIGHT] {
        assert_eq!(
            palette.role_color(SemanticRole::Border),
            palette.role_color(SemanticRole::TextMuted)
        );
        let mut out = Vec::new();
        ii::style::emit_prose(
            b"---\ntitle: Study\n---\n\n```rust\nlet x = 1;\n```\n",
            palette,
            &mut ii::style::State::with_width(40),
            &mut out,
        );
        let muted = palette.sgr_fg(SemanticRole::TextMuted);
        for glyph in ["─", "╭", "╮", "│", "╰", "╯"] {
            for at in memchr::memmem::find_iter(&out, glyph.as_bytes()) {
                let before = &out[..at];
                let start = memchr::memmem::rfind(before, b"\x1b[").unwrap();
                let end = start + memchr::memchr(b'm', &before[start..]).unwrap() + 1;
                assert_eq!(&before[start..end], muted, "wrong color for {glyph}");
            }
        }
    }
}

#[test]
fn code_frames_close_on_the_right_without_spaced_language_brackets() {
    let mut out = Vec::new();
    ii::style::emit_prose(
        b"```{python}\nx = 1\n```\n",
        &ii::theme::DARK,
        &mut ii::style::State::with_width(24),
        &mut out,
    );
    let lines: Vec<_> = out
        .split(|&b| b == b'\n')
        .filter(|s| !s.is_empty())
        .collect();
    assert!(lines[0].ends_with("╮\x1b[0m".as_bytes()));
    assert!(lines[1].ends_with("│\x1b[0m".as_bytes()));
    assert!(lines[2].ends_with("╯\x1b[0m".as_bytes()));
    assert!(memchr::memmem::find(&out, b" [ ").is_none());
    assert!(memchr::memmem::find(&out, b" ] ").is_none());
    assert!(memchr::memmem::find(&out, b"{python}").is_none());
}

#[test]
fn code_indentation_stays_inside_the_frame() {
    for cols in [24, 40, 80] {
        let mut out = Vec::new();
        ii::style::emit_prose(
            b"```python\ndef f():\n    if True:\n        return 42\n```\n",
            &ii::theme::DARK,
            &mut ii::style::State::with_width(cols),
            &mut out,
        );
        let text = String::from_utf8(out).unwrap();
        for line in text.lines() {
            // Every code row starts with a color sequence followed by its rail.
            if line.contains('│') {
                assert!(
                    line.starts_with('\x1b'),
                    "code indentation moved the frame: {line:?}"
                );
            }
        }
        assert!(text.contains("    "), "code indentation was discarded");
    }
}

#[test]
fn graphic_cells_keep_coordinates_when_neovim_redraws_a_row_fragment() {
    let mut text = Vec::new();
    ii::kitty::emit_explicit_placeholder(0x0012_3456, 8, 4, &mut text);
    let text = text.split(|&byte| byte == b'\n').nth(3).unwrap();
    let marker = "\u{10eeee}".as_bytes();
    let starts: Vec<_> = memchr::memmem::find_iter(text, marker).collect();
    assert_eq!(starts.len(), 8);
    for (col, &start) in starts.iter().enumerate() {
        let coordinates = &text[start + marker.len()..];
        assert!(
            coordinates.starts_with(ii::kitty::DIACRITICS_UTF8[3]),
            "redrawn cell lost image row"
        );
        assert!(
            coordinates[ii::kitty::DIACRITICS_UTF8[3].len()..]
                .starts_with(ii::kitty::DIACRITICS_UTF8[col]),
            "redrawn cell lost image column"
        );
    }
}

#[test]
fn viewport_keeps_the_upload_for_a_wrapped_cell_with_a_shared_rgb() {
    use ii::compose::{ComposeOut, Upload, append_visible_uploads};
    let frame = ComposeOut {
        apc: b"ab".to_vec(),
        uploads: vec![
            Upload {
                image_id: 0x0112_3456,
                start: 0,
                end: 1,
            },
            Upload {
                image_id: 0x0212_3456,
                start: 1,
                end: 2,
            },
        ],
        ..ComposeOut::default()
    };
    let mut src = b"abc".to_vec();
    ii::kitty::emit_explicit_placeholder(0x0212_3456, 2, 2, &mut src);
    for skip in [0, 1, 2] {
        let mut page = Vec::new();
        ii::viewport::append(&src, 4, skip, 2, &mut page);
        let mut apc = Vec::new();
        append_visible_uploads(&frame, &page, &mut apc);
        assert_eq!(apc, b"b", "wrong upload at row {skip}");
    }
}

#[test]
fn typst_anchors_follow_a_multiline_table() -> Result<(), Box<dyn std::error::Error>> {
    use ii::compose::{self, ComposeCtx, ComposeOut, ComposeScratch};
    let src =
        b"= Before\n\n#table(\ncolumns: 2,\n[Name], [Value],\n[x], [42],\n)\n\nAfter the table\n";
    let ctx = ComposeCtx {
        cell_w: 9,
        cell_h: 18,
        cols: 80,
        rows: 40,
        max_w: 720,
        palette: &ii::theme::DARK,
        is_tmux: false,
        is_remote: true,
        skip_lines: 0,
        max_lines: 0,
        explicit_graphics: false,
    };
    let mut out = ComposeOut::new();
    compose::render_file(
        src,
        ii::fmt::Kind::Typst,
        ctx,
        None,
        &mut ComposeScratch::new(),
        &mut 0,
        1,
        &mut out,
    )?;
    let tail = src.len() - b"After the table\n".len();
    let &(start, end, first, last) = out.blocks.last().ok_or("missing Typst blocks")?;
    assert_eq!((start, end), (tail, src.len()));
    assert!(out.ansi[first..last].starts_with(b"After the table"));
    let table = out
        .blocks
        .iter()
        .find(|&&(a, _, _, _)| src[a..].starts_with(b"#table("))
        .ok_or("missing source range for Typst table")?;
    assert!(memchr::memmem::find(&out.ansi[table.2..table.3], b"42").is_some());
    Ok(())
}

#[test]
fn standalone_diagrams_fit_window_height_and_invalidate_cached_size()
-> Result<(), Box<dyn std::error::Error>> {
    use ii::compose::{self, ComposeCtx, ComposeOut, ComposeScratch};
    let source = include_bytes!("../showcase/05.mmd");
    let mut scratch = ComposeScratch::new();
    let mut out = ComposeOut::new();
    let mut ids = Vec::new();
    let mut heights = Vec::new();
    for rows in [8, 24] {
        let context = ComposeCtx {
            cell_w: 9,
            cell_h: 18,
            cols: 80,
            rows,
            max_w: 720,
            palette: &ii::theme::DARK,
            is_tmux: false,
            is_remote: true,
            skip_lines: 0,
            max_lines: 0,
            explicit_graphics: false,
        };
        compose::render_file(
            source,
            ii::fmt::Kind::Mermaid,
            context,
            None,
            &mut scratch,
            &mut 0,
            1,
            &mut out,
        )?;
        let height = memchr::memchr_iter(b'\n', &out.ansi).count();
        assert!(
            height <= usize::from(rows),
            "diagram exceeded viewport height"
        );
        heights.push(height);
        ids.push(out.uploads[0].image_id);
    }
    assert!(
        heights[1] > heights[0],
        "diagram did not adapt when window grew"
    );
    assert_ne!(ids[0], ids[1], "raster cache ignored viewport height");
    Ok(())
}
