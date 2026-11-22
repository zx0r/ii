//! Pure Sans-I/O core contracts & deterministic property tests (Miri + Tree Borrows clean).

use proptest::{
    collection::vec as prop_vec,
    prelude::any,
    prop_assert, prop_assert_eq, prop_assert_ne,
    test_runner::{Config, RngAlgorithm, TestRng, TestRunner},
};

use ii::{
    cache,
    err::Error,
    ipc::{
        self, Cmd, ERR_FLAG, Extmark, Header, MAX_PAYLOAD, RESP_FLAG, SpanCoord, compose_extmarks,
        encode_dual_response, encode_error, encode_extmarks,
    },
    kitty::{self, StageParams},
    layout::{LinearScale, TableMetrics, compute_table_widths},
    raster::unpremultiply,
    scene::{GlyphRun, Rect, RenderScratch, Scene, Verb},
    theme::{self, ColorToken, DARK, LIGHT, Rgba, SemanticRole},
    tty::{Metrics, Term, luminance, parse_osc_rgb, render_ansi},
    vram::{Cache, Payload, Table, Tag, compute_tag},
};

fn runner() -> TestRunner {
    let cases = if cfg!(miri) { 16 } else { 256 };
    let cfg = Config {
        cases,
        failure_persistence: None,
        ..Config::default()
    };
    TestRunner::new_with_rng(cfg, TestRng::deterministic_rng(RngAlgorithm::ChaCha))
}

#[test]
fn scene_utf8_clamp_series_and_scratch_rss_cap() {
    let mut scene = Scene::with_capacity(8, 32);
    scene.push_rect(Rect {
        x: 1.0,
        y: 2.0,
        w: 30.0,
        h: 40.0,
        color: ColorToken::ACCENT,
        radius: 2,
        stroke_width: 1,
        flags: Rect::FILL | Rect::STROKE,
    });
    scene.push_text_styled(
        4.0,
        8.0,
        "π ≈ 3.14",
        ColorToken::FG_PRIMARY,
        14,
        GlyphRun::BOLD,
    );
    assert_eq!(scene.text(&scene.glyphs[0]), "π ≈ 3.14");

    // Multi-byte UTF-8 > u16::MAX clamps cleanly on char boundary.
    let big = "λ".repeat(40_000);
    let interned = scene.intern_text(&big);
    assert!(interned.is_some());
    if let Some((off, len)) = interned {
        assert_eq!(len, 65_534);
        assert!(scene.slice_text(off, len).chars().all(|ch| ch == 'λ'));
    }

    scene.push_series_line(
        &[[0.0, 0.0], [10.0, 20.0], [30.0, 5.0]],
        ColorToken::series(3),
        2,
    );
    assert_eq!(scene.paths.len(), 1);
    assert_eq!(scene.verbs, [Verb::MoveTo, Verb::LineTo, Verb::LineTo]);

    assert_eq!(scene.slice_text(u32::MAX, 10), "");
    assert_eq!(scene.slice_text(999_999, 4), "");

    let mut scratch = RenderScratch::new();
    scratch
        .rgba_buf
        .resize(RenderScratch::MAX_RETAINED_RGBA + 4096, 1);
    scratch
        .out_buf
        .resize(RenderScratch::MAX_RETAINED_OUT + 4096, 1);
    scratch.reset();
    assert_eq!(scratch.rgba_buf, b"");
    assert!(scratch.rgba_buf.capacity() <= RenderScratch::MAX_RETAINED_RGBA);
    assert!(scratch.out_buf.capacity() <= RenderScratch::MAX_RETAINED_OUT);
}

#[test]
fn vram_l0_plru_eviction_and_l1_capacity_bounds() {
    let make_tag =
        |val: u64| Tag::new(val).unwrap_or_else(|| compute_tag(8, 16, 80, 0, [0; 3], &[]));

    let mut tbl = Table::new();
    for idx in 0..4u32 {
        let evicted = tbl.insert(
            make_tag(7 + u64::from(idx) * 64),
            Payload {
                image_id: 10 + idx,
                cols: 2,
                rows: 1,
            },
        );
        assert_eq!(evicted, None);
    }
    assert!(tbl.lookup(make_tag(7)).is_some());
    assert!(tbl.lookup(make_tag(7 + 128)).is_some());
    assert!(tbl.lookup(make_tag(7 + 192)).is_some());

    let victim = tbl.insert(
        make_tag(7 + 256),
        Payload {
            image_id: 99,
            cols: 3,
            rows: 2,
        },
    );
    assert_eq!(
        victim,
        Some(Payload {
            image_id: 11,
            cols: 2,
            rows: 1
        })
    );
    assert_eq!(tbl.lookup(make_tag(7 + 64)), None);
    assert_eq!(tbl.lookup(make_tag(7 + 256)).map(|p| p.image_id), Some(99));

    tbl.invalidate();
    assert_eq!(tbl.lookup(make_tag(7 + 256)), None);

    let mut cache = Cache::new();
    let base = 1u64 << 6;
    let outlier = vec![0xAA; Cache::MAX_RETAINED_SLOT_CAP + 4096];
    cache.insert(make_tag(base + 1), 10, 5, 80, 80, &outlier);
    cache.insert(make_tag(base + 1), 2, 1, 16, 16, &[0xBB; 64]);
    let rep = cache.lookup(make_tag(base + 1));
    assert!(rep.is_some());
    if let Some(slot) = rep {
        assert_eq!(slot.rgba.len(), 64);
        assert!(slot.rgba.capacity() <= Cache::MAX_RETAINED_SLOT_CAP);
    }
    cache.invalidate();
    assert!(cache.lookup(make_tag(base + 1)).is_none());
}

#[test]
fn prop_vram_tag_and_image_id_never_zero() {
    let strat = (
        any::<u16>(),
        any::<u16>(),
        any::<u16>(),
        any::<u8>(),
        any::<[u8; 3]>(),
        prop_vec(any::<u8>(), 0..256),
    );
    let res = runner().run(&strat, |(cw, ch, cols, kind, rgb, payload)| {
        let tag = compute_tag(cw, ch, cols, kind, rgb, &payload);
        prop_assert_ne!(tag.get(), 0);
        let id = tag.image_id().get();
        prop_assert!((1..=0x00FF_FFFF).contains(&id));
        Ok(())
    });
    assert!(res.is_ok(), "{res:?}");
}

#[test]
fn prop_ipc_wire_codecs_and_validation() {
    let strat = (
        any::<u32>(),
        any::<u16>(),
        any::<u16>(),
        any::<u32>(),
        any::<u32>(),
        any::<u32>(),
        any::<u32>(),
        any::<u16>(),
        any::<u16>(),
        any::<u16>(),
        any::<u16>(),
    );
    let res = runner().run(
        &strat,
        |(
            magic,
            version,
            command,
            req_id_lo,
            req_id_hi,
            flags,
            payload_len,
            cols,
            rows,
            cell_w,
            cell_h,
        )| {
            let hdr = Header {
                magic,
                version,
                command,
                req_id_lo,
                req_id_hi,
                flags,
                payload_len,
                cols,
                rows,
                cell_w,
                cell_h,
            };
            prop_assert_eq!(Header::from_le_bytes(&hdr.to_le_bytes()), hdr);
            Ok(())
        },
    );
    assert!(res.is_ok(), "{res:?}");

    let mut hdr = Header::new(Cmd::ComposeAnsi, 128);
    assert_eq!(hdr.validate(), Ok(Cmd::ComposeAnsi));
    hdr.magic = 0;
    assert_eq!(hdr.validate(), Err(Error::BadMagic));
    hdr.magic = ipc::MAGIC;
    hdr.version = 99;
    assert_eq!(hdr.validate(), Err(Error::BadVersion(99)));
    hdr.version = ipc::VERSION;
    hdr.payload_len = MAX_PAYLOAD + 1;
    assert_eq!(hdr.validate(), Err(Error::PayloadTooLarge(MAX_PAYLOAD + 1)));
    hdr.payload_len = 0;
    hdr.command = 0x00FF;
    assert_eq!(hdr.validate(), Err(Error::UnknownCommand(0x00FF)));

    let mut wire = Vec::new();
    encode_dual_response(Cmd::RenderFile, b"APC", b"TXT", &mut wire);
    let mut hdr_bytes = [0u8; 32];
    hdr_bytes.copy_from_slice(&wire[..32]);
    let resp = Header::from_le_bytes(&hdr_bytes);
    let resp_cmd = resp.command;
    let resp_payload_len = resp.payload_len;
    let resp_flags = resp.flags;
    assert_eq!(resp_cmd, (Cmd::RenderFile as u16) | RESP_FLAG);
    assert_eq!(resp_payload_len, 6);
    assert_eq!(resp_flags, 3);
    assert_eq!(&wire[32..35], b"APC");
    assert_eq!(&wire[35..38], b"TXT");

    encode_error(Cmd::Ping as u16, b"boom", &mut wire);
    hdr_bytes.copy_from_slice(&wire[..32]);
    let err_hdr = Header::from_le_bytes(&hdr_bytes);
    let err_cmd = err_hdr.command;
    assert_eq!(err_cmd, (Cmd::Ping as u16) | RESP_FLAG | ERR_FLAG);
    assert_eq!(&wire[32..], b"boom");
}

#[test]
fn prop_extmarks_and_cache_header_roundtrip() {
    let strat = (
        prop_vec(any::<u8>(), 0..512),
        prop_vec(
            (
                any::<u32>(),
                any::<u32>(),
                any::<u32>(),
                any::<u16>(),
                any::<u16>(),
            ),
            0..16,
        ),
    );
    let res = runner().run(&strat, |(src, raw_spans)| {
        let mut line_starts = Vec::new();
        let mut out = Vec::new();
        let mut encoded = Vec::new();
        let spans: Vec<SpanCoord> = raw_spans
            .into_iter()
            .map(|(outer_start, outer_end, image_id, cols, rows)| SpanCoord {
                outer_start,
                outer_end,
                image_id,
                cols,
                rows,
            })
            .collect();
        compose_extmarks(&src, &spans, &mut line_starts, &mut out);
        prop_assert_eq!(out.len(), spans.len());
        for rec in &out {
            prop_assert!(rec.end_row >= rec.start_row);
            prop_assert_eq!(Extmark::from_le_bytes(&rec.to_le_bytes()), *rec);
        }
        encoded.clear();
        encode_extmarks(&out, &mut encoded);
        prop_assert_eq!(encoded.len(), out.len() * 16);
        Ok(())
    });
    assert!(res.is_ok(), "{res:?}");

    let ch = cache::Header {
        magic: cache::MAGIC,
        width: 320,
        height: 240,
    };
    assert_eq!(cache::Header::from_le_bytes(ch.to_le_bytes()), ch);
}

#[test]
fn kitty_base64_chunking_tmux_delete_and_placeholder() {
    let rgba = vec![0x5A; 3075];
    let mut apc = Vec::new();
    let mut b64 = Vec::new();
    let params = StageParams {
        image_id: 0x0012_3456,
        w: 25,
        h: 31,
        cols: 4,
        rows: 2,
        is_tmux: false,
    };
    kitty::stage_base64(&rgba, params, &mut apc, &mut b64);
    let text_apc = core::str::from_utf8(&apc).unwrap_or("");
    assert!(text_apc.starts_with("\x1b_Ga=t,t=d,f=32,q=2,i=1193046,s=25,v=31,m=1;"));
    assert!(text_apc.contains("\x1b\\\x1b_Gm=0;"));
    assert!(text_apc.ends_with("\x1b_Ga=p,U=1,q=2,i=1193046,c=4,r=2\x1b\\"));

    apc.clear();
    kitty::emit_delete(42, true, &mut apc);
    assert_eq!(apc, b"\x1bPtmux;\x1b\x1b_Ga=d,d=I,q=2,i=42\x1b\x1b\\\x1b\\");

    let mut grid = Vec::new();
    kitty::emit_placeholder(0x0012_3456, 0, 2, &mut grid);
    assert_eq!(grid, b"");
    kitty::emit_placeholder(0x0012_3456, 2, 2, &mut grid);
    assert!(grid.starts_with(b"\x1b[38;2;18;52;86m"));
    assert_eq!(memchr::memchr_iter(b'\n', &grid).count(), 2);
}

#[test]
fn tty_osc_parse_luminance_and_render_ansi_newline_invariant() {
    assert_eq!(parse_osc_rgb(b"\x1b]11;rgb:1e1e/1e1e/2"), None);
    assert_eq!(
        parse_osc_rgb(b"\x1b]11;rgb:1e1e/1e1e/2e2e\x07"),
        Some((0x1e, 0x1e, 0x2e))
    );
    assert_eq!(
        parse_osc_rgb(b"\x1b]11;rgb:f/0/a\x1b\\"),
        Some((0xff, 0x00, 0xaa))
    );
    assert!(luminance(0x1e, 0x1e, 0x2e) < 128);
    assert!(luminance(0xef, 0xf1, 0xf5) >= 128);
    let mut metrics = Metrics::FALLBACK;
    metrics.cols = u16::MAX;
    metrics.cell_w = 2;
    assert_eq!(metrics.max_width_px(), u16::MAX);
    assert_eq!(Term::default().metrics, Metrics::FALLBACK);

    // Trailing newline before span must not duplicate into `\n\n` after SGR_RESET.
    let mut out = Vec::new();
    let src = b"line1\n$$x$$\nline2";
    let spans = [SpanCoord {
        outer_start: 6,
        outer_end: 11,
        image_id: 7,
        cols: 1,
        rows: 1,
    }];
    render_ansi(src, &spans, true, &mut out);
    assert!(!out.windows(2).any(|w| w == b"\n\n"));
    assert!(!out.windows(5).any(|w| w == b"$$x$$"));

    let res = runner().run(&prop_vec(any::<u8>(), 0..128), |buf| {
        let _ = parse_osc_rgb(&buf);
        Ok(())
    });
    assert!(res.is_ok(), "{res:?}");
}

#[test]
fn prop_layout_no_panic_and_exact_width_sum() {
    let scale_strat = (
        any::<f32>(),
        any::<f32>(),
        any::<f32>(),
        any::<f32>(),
        any::<f32>(),
        0usize..128,
    );
    let res = runner().run(&scale_strat, |(d0, d1, r0, r1, val, count)| {
        let mut ticks = Vec::new();
        let scale = LinearScale::new(d0, d1, r0, r1);
        let _ = scale.map(val);
        scale.compute_ticks(count, &mut ticks);
        prop_assert!(ticks.len() <= 64);
        Ok(())
    });
    assert!(res.is_ok(), "{res:?}");

    let tbl_strat = (
        0usize..32,
        prop_vec(0usize..500, 0..128),
        any::<f32>(),
        any::<f32>(),
        any::<f32>(),
    );
    let res = runner().run(&tbl_strat, |(cols, lens, cw, pad, max_w)| {
        let mut widths = Vec::new();
        let metrics = compute_table_widths(cols, &lens, cw, pad, max_w, &mut widths);
        if cols == 0 || !cw.is_finite() || !pad.is_finite() || !max_w.is_finite() {
            prop_assert_eq!(metrics, TableMetrics::default());
            prop_assert!(widths.is_empty());
        } else {
            prop_assert_eq!(widths.len(), cols);
            let sum: f32 = widths.iter().sum();
            prop_assert!((metrics.total_width - sum).abs() <= 1e-2);
        }
        Ok(())
    });
    assert!(res.is_ok(), "{res:?}");
}

#[test]
fn theme_error_and_unpremultiply_invariants() {
    assert_ne!(
        DARK.hex_str(SemanticRole::Surface),
        LIGHT.hex_str(SemanticRole::Surface)
    );
    assert_ne!(DARK.series_hex_str(0), LIGHT.series_hex_str(0));
    assert_ne!(
        theme::sgr(ColorToken::FG_PRIMARY, true),
        theme::sgr(ColorToken::FG_PRIMARY, false)
    );
    assert_eq!(
        theme::resolve_color(ColorToken::BG_PRIMARY, true),
        DARK.color(ColorToken::BG_PRIMARY)
    );
    assert_eq!(Rgba::from_rgba(1, 2, 3, 4).to_array(), [1, 2, 3, 4]);
    assert_eq!(
        Error::UnknownCommand(0xdead).to_string(),
        "unknown command 0xdead"
    );
    assert_eq!(
        Error::from(rustix::io::Errno::PERM),
        Error::Io(rustix::io::Errno::PERM)
    );

    for byte in 0..=255u8 {
        let expected = match byte {
            b'0'..=b'9' => Some(byte - b'0'),
            b'a'..=b'f' => Some(byte - b'a' + 10),
            b'A'..=b'F' => Some(byte - b'A' + 10),
            _ => None,
        };
        assert_eq!(theme::hex_digit(byte), expected);
    }

    let mut px = [64u8, 128, 0, 128, 99, 88, 77, 0, 10, 20, 30, 255];
    unpremultiply(&mut px);
    assert!((i16::from(px[0]) - 128).abs() <= 1);
    assert_eq!(px[1], 255);
    assert_eq!(&px[4..8], &[0, 0, 0, 0]);
    assert_eq!(&px[8..12], &[10, 20, 30, 255]);

    let res = runner().run(&prop_vec(any::<u8>(), 0..128), |mut buf| {
        unpremultiply(&mut buf);
        for chunk in buf.as_chunks::<4>().0 {
            if chunk[3] == 0 {
                prop_assert_eq!(&chunk[..3], &[0, 0, 0]);
            }
        }
        Ok(())
    });
    assert!(res.is_ok(), "{res:?}");
}

#[test]
fn style_metrics_quantum_and_visual_hierarchy_invariants() {
    use ii::layout::StyleMetrics;

    assert_eq!(size_of::<StyleMetrics>(), 40);

    // Standard 1x display:
    let m1x = StyleMetrics::from_cell_geometry(9, 18);
    assert_eq!(m1x.dpi_scale, 1.0);
    assert_eq!(m1x.stroke_hairline, 1.0);
    assert_eq!(m1x.stroke_thin, 1.0);
    assert_eq!(m1x.stroke_medium, 1.5);
    assert_eq!(m1x.stroke_thick, 2.0);
    // Visual hierarchy guarantee on 1x:
    assert!(m1x.stroke_medium < m1x.stroke_thick);
    assert_eq!(m1x.corner_radius, 3.0);
    assert_eq!(m1x.pad_x, 9.0);
    assert_eq!(m1x.pad_y, 9.0);
    assert_eq!(m1x.font_px, 13.5);
    assert_eq!(m1x.icon_size(), 13.5);

    // Retina 2x display:
    let m2x = StyleMetrics::from_cell_geometry(18, 36);
    assert_eq!(m2x.dpi_scale, 2.0);
    assert_eq!(m2x.stroke_hairline, 1.0);
    assert_eq!(m2x.stroke_thin, 2.0);
    assert_eq!(m2x.stroke_medium, 3.0);
    assert_eq!(m2x.stroke_thick, 4.0);
    assert_eq!(m2x.corner_radius, 6.0);
    assert_eq!(m2x.pad_x, 18.0);
    assert_eq!(m2x.pad_y, 18.0);
    assert_eq!(m2x.font_px, 27.0);
    assert_eq!(m2x.icon_size(), 27.0);

    let gs = m1x.grid_stroke();
    assert_eq!(gs.width, 1.0);
    let ps = m1x.path_stroke(m1x.stroke_medium);
    assert_eq!(ps.width, 1.5);
}
