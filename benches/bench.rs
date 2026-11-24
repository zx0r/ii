//! Criterion benchmarks for `ii`: SIMD scan throughput across diverse workloads,
//! real-time format parsing, zero-alloc cache hit latency, and warm end-to-end pipeline projection.

#![forbid(unsafe_code)]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::hint::black_box;

use criterion::{Criterion, Throughput, criterion_group, criterion_main};
use ii::{
    fmt::{ipynb, md, tex},
    kitty,
    scene::Scene,
    theme::DARK,
    vram::{Cache, Tag},
};

/// Synthetic dense markup chunk (65% formulas and diagrams, 10–20B delimiter distance).
const DENSE_CHUNK: &[u8] = b"# Section\nText with $100 currency and `$code$`.\n$$\\int_0^\\infty e^{-x^2} dx = \\frac{\\sqrt{\\pi}}{2}$$\n```mermaid\ngraph TD; A-->B;\n```\n";

/// Realistic technical article chunk (paragraphs of prose, inline math, display equation, and mermaid).
const PROSE_CHUNK: &[u8] = b"## Distributed System Architecture and State Machine Replication\n\
In high-throughput distributed systems, achieving linearizable consensus requires careful coordination \
between replicas. Under the Raft protocol, the leader maintains an invariant that all committed log entries \
are durably replicated across a majority quorum of $2f + 1$ nodes before acknowledging client writes. \
When network partitions occur, the system guarantees safety at the expense of temporary availability loss \
as formalized by Brewer's CAP theorem.\n\n\
Consider an asynchronous network where message transmission delay follows an exponential tail distribution. \
The bound on election timeout convergence is given by the following concentration inequality:\n\
$$\\Pr[\\bar{X}_n - \\mu \\ge t] \\le \\exp\\left(-\\frac{2n^2 t^2}{\\sum_{i=1}^n (b_i - a_i)^2}\\right)$$\n\n\
During stable leader election with term $\\tau \\in \\mathbb{N}$, the message exchange pattern proceeds \
according to the following sequence diagram:\n\
```mermaid\n\
sequenceDiagram\n\
    autonumber\n\
    Client->>Leader: Propose(Command, Key=$42)\n\
    Leader->>Follower1: AppendEntries(Term, LogIndex)\n\
    Leader->>Follower2: AppendEntries(Term, LogIndex)\n\
    Follower1-->>Leader: AppendSuccess(MatchIndex)\n\
    Leader-->>Client: CommitAck(Offset=1024)\n\
```\n\n\
Operational infrastructure cost estimation indicates that running dedicated c7g.4xlarge instances \
yields an average expenditure of $1500 to $2400 per month, depending on EBS gp3 throughput allocations. \
Inline expressions like `$O(\\log N)$` index lookup and `$\\lambda \\le 0.05$` error rate demonstrate \
the latency bounds under heavy tail workloads.\n\n";

/// Pure prose chunk without markup to evaluate raw SIMD `memchr3` saturation limit.
const PURE_PROSE_CHUNK: &[u8] = b"The architecture of modern storage engines relies heavily on structured log-structured \
merge-trees to optimize write throughput on non-volatile memory media. By converting random input/output \
operations into sequential disk appends, the engine amortizes the physical seek overhead \
inherent in solid-state drive block erase cycles. Background compaction workers continuously coalesce \
overlapping key ranges across immutable SSTable levels, maintaining tight read amplification bounds while \
preserving linearizable ordering across transactional write batches.\n\n";

fn bench_scan_throughput(c: &mut Criterion) {
    let mut group = c.benchmark_group("scan_throughput");
    let mut spans = Vec::with_capacity(2048);

    let dense_doc = DENSE_CHUNK.repeat(512);
    let prose_doc = PROSE_CHUNK.repeat(48);
    let pure_doc = PURE_PROSE_CHUNK.repeat(100);

    group.throughput(Throughput::Bytes(dense_doc.len() as u64));
    group.bench_function("scan_markdown/dense_syntax_68kb", |b| {
        b.iter(|| {
            md::scan(black_box(&dense_doc), &mut spans);
            black_box(spans.len())
        });
    });

    group.throughput(Throughput::Bytes(prose_doc.len() as u64));
    group.bench_function("scan_markdown/technical_prose_64kb", |b| {
        b.iter(|| {
            md::scan(black_box(&prose_doc), &mut spans);
            black_box(spans.len())
        });
    });

    group.throughput(Throughput::Bytes(pure_doc.len() as u64));
    group.bench_function("scan_markdown/pure_prose_62kb", |b| {
        b.iter(|| {
            md::scan(black_box(&pure_doc), &mut spans);
            black_box(spans.len())
        });
    });

    group.finish();
}

fn bench_stream_frontends(c: &mut Criterion) {
    let mut group = c.benchmark_group("format_compile");

    let math = br"\begin{aligned} \mathcal{L}(\theta) &= \sum_{i=1}^{N} \left[ y_i \log \frac{1}{1 + \exp(-\theta^T x_i)} \right] \end{aligned}";
    let mut scene = Scene::with_capacity(64, 1024);

    group.throughput(Throughput::Bytes(math.len() as u64));
    group.bench_function("parse_tex_into_scene", |b| {
        b.iter(|| {
            scene.clear();
            let _ = tex::parse_into_scene(black_box(math), &mut scene);
            black_box(scene.glyphs.len())
        });
    });

    let chem_formula =
        br"\ce{2 H2 (g) + O2 (g) <=>[\Delta][\text{Pt}] 2 H2O (l)} + \ce{CuSO4.5H2O} + \ce{^14_6C}";

    group.throughput(Throughput::Bytes(chem_formula.len() as u64));
    group.bench_function("parse_chem_into_scene", |b| {
        b.iter(|| {
            scene.clear();
            let _ = tex::parse_chem_into_scene(black_box(chem_formula), &mut scene);
            black_box(scene.glyphs.len())
        });
    });

    group.finish();
}

fn bench_cache_hit_zero_alloc(c: &mut Criterion) {
    let Some(tag) = Tag::new(42) else { return };
    let mut cache = Cache::new();
    let rgba = vec![0xff; 96 * 32 * 4];
    cache.insert(tag, 12, 2, 96, 32, &rgba);

    let mut arena = Vec::with_capacity(4096);

    let mut group = c.benchmark_group("cache");
    group.throughput(Throughput::Elements(1));
    group.bench_function("vram_l1_lookup_and_placeholder", |b| {
        b.iter(|| {
            let hit = cache.lookup(black_box(tag));
            arena.clear();
            if let Some(p) = hit {
                kitty::emit_placeholder(
                    tag.image_id().get(),
                    black_box(p.cols),
                    black_box(p.rows),
                    &mut arena,
                );
            }
            black_box((hit.map(|p| p.rgba.len()), arena.len()))
        });
    });
    group.finish();
}

fn bench_full_pipeline_warm(c: &mut Criterion) {
    let doc = b"# Notes\n$$\\frac{-b \\pm \\sqrt{b^2 - 4ac}}{2a}$$\nInline $\\alpha + \\beta$.\n";
    let mut scratch = ii::compose::ComposeScratch::new();
    let mut out = ii::compose::ComposeOut::new();
    let ctx = ii::compose::ComposeCtx {
        cell_w: 8,
        cell_h: 16,
        cols: 80,
        rows: 24,
        max_w: 640,
        palette: &DARK,
        is_tmux: false,
        is_remote: true,
        skip_lines: 0,
        max_lines: 24,
        explicit_graphics: false,
    };
    let mut seq = 0;
    assert!(
        ii::compose::render_file(
            doc,
            ii::fmt::Kind::Md,
            ctx,
            None,
            &mut scratch,
            &mut seq,
            1,
            &mut out
        )
        .is_ok()
    );
    let mut group = c.benchmark_group("pipeline");
    group.throughput(Throughput::Bytes(doc.len() as u64));
    group.bench_function("compose_with_warm_image_cache", |b| {
        b.iter(|| {
            let result = ii::compose::render_file(
                black_box(doc),
                ii::fmt::Kind::Md,
                ctx,
                None,
                &mut scratch,
                &mut seq,
                1,
                &mut out,
            );
            assert!(result.is_ok());
            black_box((out.apc.len(), out.ansi.len()))
        });
    });
    let notebook = include_bytes!("../showcase/03.ipynb");
    let mut normalized = Vec::new();
    group.throughput(Throughput::Bytes(notebook.len() as u64));
    group.bench_function("notebook_normalize_32_cells", |b| {
        b.iter(|| {
            normalized.clear();
            assert!(ipynb::to_markdown(black_box(notebook), &mut normalized).is_ok());
            black_box(normalized.len())
        });
    });
    group.finish();
}

criterion_group!(
    benches,
    bench_scan_throughput,
    bench_stream_frontends,
    bench_cache_hit_zero_alloc,
    bench_full_pipeline_warm
);
criterion_main!(benches);
