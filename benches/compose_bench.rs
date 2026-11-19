use core::hint::black_box;
use ii::{
    cfg::Cfg,
    compose::{Session, compose_ansi},
    ir::Scratch,
};

fn main() {
    divan::main();
}

#[divan::bench]
fn bench_compose_cached_frame(bencher: divan::Bencher<'_, '_>) {
    let doc = b"# Title\nEquation $E = mc^2$ and chart:\n```vega-lite\n{\"mark\":\"bar\",\"data\":{\"values\":[{\"x\":\"A\",\"y\":10}]}}\n```\n";
    let mut cfg = Cfg::new();
    cfg.tty.is_remote = 1; // use deterministic in-memory Base64 path for benchmark
    let mut session = Session::new(1);
    let mut scratch = Scratch::new();
    let _ = compose_ansi(doc, &cfg, &mut session, &mut scratch);

    bencher.bench_local(|| {
        let _ = compose_ansi(black_box(doc), &cfg, &mut session, &mut scratch);
        black_box(scratch.text_buf.len())
    });
}
