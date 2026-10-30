use super::*;

fn node(id: &str, tag: &str) -> SelectedNode {
    SelectedNode {
        id: id.into(),
        version: format!("body-{id}"),
        tags: [tag.into()].into_iter().collect(),
    }
}

#[test]
fn selecting_other_pages_preserves_precise_node_bodies_and_tags() {
    let mut selected = Selection::default();
    selected.insert("root-one", node("old", "inherited")).unwrap();
    selected.insert("root-one", node("new", "other")).unwrap();
    assert_eq!(selected.len(), 2);
    assert!(selected.contains("old") && selected.contains("new"));
    let old = selected.values().find(|node| node.id == "old").unwrap();
    assert_eq!(old.version, "body-old");
    assert_eq!(old.tags, BTreeSet::from(["inherited".into()]));
    selected.remove("new");
    assert!(selected.contains("old"));
    assert_eq!(selected.root(), Some("root-one"));
}

#[test]
fn a_new_root_cannot_mix_new_nodes_with_an_unreviewed_selection() {
    let mut selected = Selection::default();
    selected.insert("root-one", node("old", "inherited")).unwrap();
    let before = selected.clone();
    assert!(matches!(
        selected.insert("root-two", node("new", "other")),
        Err(SelectionError::Changed)
    ));
    assert_eq!(selected, before);
    for incomplete in [vec![], vec![node("other", "tag")], vec![node("old", "tag"); 2]] {
        assert!(matches!(
            selected.accept_review("root-two".into(), incomplete),
            Err(SelectionError::IncompleteReview)
        ));
        assert_eq!(selected, before);
    }
    selected
        .accept_review("root-two".into(), vec![node("old", "reviewed")])
        .unwrap();
    assert_eq!(selected.root(), Some("root-two"));
    assert_eq!(selected.values().next().unwrap().tags, BTreeSet::from(["reviewed".into()]));
    selected.insert("root-two", node("new", "other")).unwrap();
    assert_eq!(selected.len(), 2);
    selected.clear();
    assert!(selected.is_empty());
    assert!(selected.root().is_none());
}
