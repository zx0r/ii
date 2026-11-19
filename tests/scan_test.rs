use ii::{ir::Kind, json::Nav, scan::scan};
use proptest::prelude::*;

proptest! {
    #[test]
    fn scan_never_panics_and_emits_monotonic_spans(bytes in proptest::collection::vec(any::<u8>(), 0..2048)) {
        let mut spans = Vec::new();
        scan(&bytes, &mut spans);
        for pair in spans.windows(2) {
            prop_assert!(pair[0].outer_end <= pair[1].outer_start);
        }
        for sp in &spans {
            prop_assert!(sp.outer_start <= sp.payload_start);
            prop_assert!(sp.payload_start <= sp.payload_end());
            prop_assert!(sp.payload_end() <= sp.outer_end);
            prop_assert!((sp.outer_end as usize) <= bytes.len());
        }
    }

    #[test]
    fn json_nav_never_panics_on_arbitrary_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..1024)) {
        let nav = Nav::new(&bytes);
        let mut scratch = Vec::new();
        let _ = nav.get(b"mark");
        let _ = nav.str_raw();
        let _ = nav.str_of(&mut scratch);
        let _ = nav.num_of();
        let _ = nav.int_of();
        for item in nav.items().take(16) {
            let _ = item.str_raw();
        }
    }
}

#[test]
fn scan_currency_vs_inline_math_and_fences() {
    let src = b"Price is $5 and $10, not math. Real math: $x + y = z$ and \\[a^2 + b^2\\].\n```vega-lite\n{\"mark\":\"bar\"}\n```\n";
    let mut spans = Vec::new();
    scan(src, &mut spans);
    assert_eq!(spans.len(), 3);
    assert_eq!(spans[0].kind(), Kind::MathInline);
    assert_eq!(spans[0].payload(src), b"x + y = z");
    assert_eq!(spans[1].kind(), Kind::MathDisplay);
    assert_eq!(spans[1].payload(src), b"a^2 + b^2");
    assert_eq!(spans[2].kind(), Kind::VegaLite);
}
