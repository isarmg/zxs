use super::*;

#[test]
fn receipts_settle_only_the_matching_publication_and_preserve_unknown_outcomes() {
    let mut model = Model {
        checked_nodes: crate::files::tags::tests::selection(&["selected-node"]),
        ..Default::default()
    };
    model.publications.insert(
        Kind::Tags,
        Publication {
            operation_id: Some("original-tags-operation".into()),
            error: "network interrupted".into(),
        },
    );
    model.publications.insert(
        Kind::Merge,
        Publication {
            operation_id: Some("different-merge-operation".into()),
            error: String::new(),
        },
    );
    settle(&mut model, "unrelated-operation", SaveStatus::Published);
    assert_eq!(model.publications.len(), 2);
    settle(&mut model, "original-tags-operation", SaveStatus::Unknown);
    assert_eq!(
        model.publications[&Kind::Tags].operation_id.as_deref(),
        Some("original-tags-operation")
    );
    assert!(model.checked_nodes.contains("selected-node"));
    settle(&mut model, "original-tags-operation", SaveStatus::Rejected);
    assert!(model.publications[&Kind::Tags].operation_id.is_none());
    assert!(!model.publications[&Kind::Tags].error.is_empty());
    assert!(model.checked_nodes.contains("selected-node"));
    model
        .publications
        .get_mut(&Kind::Tags)
        .unwrap()
        .operation_id = Some("retry-tags-operation".into());
    settle(&mut model, "original-tags-operation", SaveStatus::Published);
    assert!(model.checked_nodes.contains("selected-node"));
    settle(&mut model, "retry-tags-operation", SaveStatus::Degraded);
    assert!(model.checked_nodes.is_empty());
    assert!(!model.publications.contains_key(&Kind::Tags));
    assert_eq!(
        model.publications[&Kind::Merge].operation_id.as_deref(),
        Some("different-merge-operation")
    );
}
