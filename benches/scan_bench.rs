use core::hint::black_box;
use ii::scan::scan;

fn main() {
    divan::main();
}

#[divan::bench]
fn bench_scan_mixed_markdown(bencher: divan::Bencher<'_, '_>) {
    let doc = b"# Bench\nInline $a+b=c$ and display $$\\int_0^1 x^2 dx$$ plus \\ce{2H2 + O2 -> 2H2O}.\n```mermaid\ngraph LR; A-->B;\n```\n"
        .repeat(32);
    let mut spans = Vec::with_capacity(256);
    bencher.bench_local(|| {
        spans.clear();
        scan(black_box(&doc), &mut spans);
        black_box(spans.len())
    });
}
