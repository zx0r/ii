#!/usr/bin/env python3
"""Compare historical and current scanners in one optimized executable on identical corpora."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
COMMIT = 'a2d84066f838f42364ecc8b7dd56573758dc4f2e'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--history', type=Path, default=ROOT.parent / 'ii_old')
    parser.add_argument('--report', type=Path)
    parser.add_argument('--native', action='store_true', help='Compile both scanners for the local CPU')
    args = parser.parse_args()
    old = subprocess.check_output(['git', '-C', str(args.history), 'show', COMMIT+':src/scan.rs'])
    corpus = (ROOT / 'benches/bench.rs').read_text()
    corpus = corpus[corpus.index('const DENSE_CHUNK:'):corpus.index('fn bench_scan_throughput')]
    with tempfile.TemporaryDirectory(prefix='ii-scan-compare-') as tmp:
        tmp = Path(tmp)
        (tmp / 'src').mkdir()
        (tmp / 'Cargo.toml').write_text('[package]\nname="ii-scan-comparison"\nversion="0.0.0"\nedition="2024"\n[dependencies]\nii={path='+json.dumps(str(ROOT))+'}\nmemchr="2.8.3"\n[profile.release]\nopt-level=3\nlto="fat"\ncodegen-units=1\n')
        (tmp / 'src/old.rs').write_bytes(old)
        (tmp / 'src/main.rs').write_text('''#![allow(dead_code)]
mod old;
use std::{hint::black_box, time::{Instant, Duration}};
''' + corpus + '''
fn measure(mut run: impl FnMut()) -> f64 {
    for _ in 0..1000 {run();}
    let mut samples=Vec::new();
    for _ in 0..7 {
        let start=Instant::now(); let mut n=0;
        while start.elapsed()<Duration::from_millis(180) {
            for _ in 0..100 {run();} n+=100;
        }
        samples.push(start.elapsed().as_secs_f64()/f64::from(n));
    }
    samples.sort_by(f64::total_cmp); samples[3]
}
fn main() {
    println!("corpus,bytes,old_spans,current_spans,old_ns,current_ns,old_GiBs,current_GiBs,old_MiBs,current_MiBs");
    for (name,src) in [("dense",DENSE_CHUNK.repeat(512)),("technical",PROSE_CHUNK.repeat(48)),("pure",PURE_PROSE_CHUNK.repeat(100))] {
        let mut a=Vec::new();let mut b=Vec::new();
        old::scan_markdown(&src,&mut a);ii::fmt::md::scan(&src,&mut b);
        let old=measure(|| {old::scan_markdown(black_box(&src), &mut a);black_box(a.len());});
        let new=measure(|| {ii::fmt::md::scan(black_box(&src), &mut b);black_box(b.len());});
        println!("{name},{},{},{},{:.1},{:.1},{:.3},{:.3},{:.1},{:.1}",src.len(),a.len(),b.len(),old*1e9,new*1e9,src.len() as f64/old/1073741824.0,src.len() as f64/new/1073741824.0,src.len() as f64/old/1048576.0,src.len() as f64/new/1048576.0);
    }
}
''')
        env = dict(os.environ, CARGO_TARGET_DIR=str(ROOT / 'target/history-comparison'))
        if args.native: env['RUSTFLAGS'] = env.get('RUSTFLAGS', '') + ' -C target-cpu=native'
        output = subprocess.check_output(['cargo', '+stable', 'run', '--offline', '--release', '--manifest-path', str(tmp / 'Cargo.toml')], env=env).decode()
    print(output, end='')
    if args.report: args.report.write_text(output)


if __name__ == '__main__': main()
