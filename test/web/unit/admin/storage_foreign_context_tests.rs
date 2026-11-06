use super::*;

#[test]
fn every_history_description_has_distinct_ids_across_the_full_review_budget() {
    let mut seen = BTreeSet::new();
    for group in 0..128 {
        let group = format!("{group:064x}");
        let context = HistoryContext::new(&group);
        let references = context.references();
        assert_eq!(
            references.split_ascii_whitespace().collect::<Vec<_>>(),
            [
                context.origin.as_str(),
                context.history.as_str(),
                context.availability.as_str()
            ]
        );
        for id in [&context.origin, &context.history, &context.availability] {
            assert!(id.starts_with("zxs-foreign-plan-"));
            assert!(id.contains(&group));
            assert!(seen.insert(id.clone()));
        }
    }
    assert_eq!(seen.len(), 384);
}

#[test]
fn context_identity_comes_from_the_group_not_shared_repository_or_slot_labels() {
    let first = HistoryContext::new(&"1".repeat(64));
    let second = HistoryContext::new(&"2".repeat(64));
    assert_ne!(first.references(), second.references());
    assert_eq!(
        first.references(),
        HistoryContext::new(&"1".repeat(64)).references(),
        "repeated selection retains stable description identities"
    );
}

#[test]
fn mapping_inputs_reference_visible_provenance_and_retained_body_status() {
    let fields = include_str!("../../../../src/web/src/admin/storage/foreign/plan/fields.rs");
    for wiring in [
        "HistoryContext::new(&history.group_id)",
        "let references=context.references()",
        "id=context.origin",
        "id=context.history",
        "id=context.availability",
        "custom_attribute(\"aria-describedby\",references)",
        "super::super::origin_label(&history.origin)",
        "{history.history_tree_id}",
        "history.temporarily_unavailable_versions.len()",
    ] {
        assert!(fields.contains(wiring), "missing context wiring: {wiring}");
    }
}
