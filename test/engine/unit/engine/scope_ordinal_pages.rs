//! Source fixtures only. One row is an occurrence, not an object hash.
use super::*;
use crate::metadata_map::ReferenceKind;

#[test]
fn canonical_order_spans_pages_without_deduplicating_occurrences() {
    let mut rows = OrdinalBuilder::new(130);
    let mut expected = json_codec::Map::new();
    for i in 0..130 {
        let value = if i == 3 { "second origin" } else { "same physical bytes" };
        rows.push(&value).unwrap();
        expected.insert(format!("{i:020}"), json_codec::to_value(value).unwrap());
    }
    assert!(rows.push(&"one too many").is_err());
    let actual = rows.finish().unwrap();
    assert_eq!(actual.entries(), 130);
    assert_eq!(actual.root().kind(), ReferenceKind::Index);
    let mut direct = Encoder::new(PAGE_LIMIT);
    assert_eq!(actual.root(), direct.map_root(expected, ProofMeasure::Plain).unwrap().reference());
    assert_eq!(actual.bytes(), direct.finish().iter().map(|p| p.bytes().len()).sum::<usize>());

    let mut different = OrdinalBuilder::new(130);
    for i in 0..130 {
        different.push(&if i == 4 { "second origin" } else { "same physical bytes" }).unwrap();
    }
    assert_ne!(actual.root(), different.finish().unwrap().root());
}

#[test]
fn an_oversized_singleton_or_a_row_in_a_proved_empty_table_is_not_truncated() {
    let mut rows = OrdinalBuilder::new(1);
    assert!(rows.push(&"x".repeat(PAGE_LIMIT)).is_err());
    rows.push(&"smaller").unwrap();
    assert_eq!(rows.finish().unwrap().entries(), 1);

    let mut empty = OrdinalBuilder::new(0);
    assert!(empty.push(&"not absent").is_err());
    let page = empty.finish().unwrap();
    assert_eq!(page.entries(), 0);
    assert_eq!(page.root().kind(), ReferenceKind::Page);
}
