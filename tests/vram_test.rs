use ii::vram::{Cache, Payload, Table, compute_tag, image_id_from_tag};

#[test]
fn vram_table_and_l1_cache_set_associative_invariants() {
    let mut tbl = Table::new();
    for i in 1..=256u64 {
        let tag = compute_tag(8, 16, 80, (i & 13) as u8, [200, 210, 220], &i.to_le_bytes());
        let id = image_id_from_tag(tag);
        assert_ne!(id, 0);
        tbl.insert(
            tag,
            Payload {
                image_id: id,
                cols: 10,
                rows: 4,
            },
        );
        assert_eq!(tbl.lookup(tag).map(|p| p.image_id), Some(id));
    }
    tbl.invalidate();
    let sample_tag = compute_tag(8, 16, 80, 0, [200, 210, 220], &1u64.to_le_bytes());
    assert_eq!(tbl.lookup(sample_tag), None);

    let mut l1 = Cache::new();
    l1.insert(sample_tag, 12, 6, 96, 96, &[0x7F; 512]);
    let rep = l1.lookup(sample_tag);
    assert!(rep.is_some());
    if let Some(r) = rep {
        assert_eq!((r.cols, r.rows, r.w, r.h), (12, 6, 96, 96));
        assert_eq!(r.rgba.len(), 512);
    }
}
