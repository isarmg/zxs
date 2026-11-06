use super::*;
use json_codec::json;

fn disk(slot: &str, level: u32, upstream: Option<&str>) -> Value {
    json!({"slot_id":slot,"disk_id":format!("disk-{slot}"),"level":level,
        "upstream_slot_id":upstream,"online":true})
}

#[test]
fn real_backup_routes_have_primary_at_bottom_and_arrows_to_the_next_tier() {
    let tree = read(json!({"disks":[
        disk("C01",3,Some("B01")),disk("P02",1,None),
        disk("B01",2,Some("P01")),disk("B02",2,Some("P02")),disk("P01",1,None),
    ]}))
    .unwrap();
    assert_eq!((tree.columns, tree.levels), (2, 3));
    assert_eq!(tree.ordered_slots(), ["C01", "B01", "B02", "P01", "P02"]);
    assert_eq!(tree.selection(None).as_deref(), Some("P01"));
    for (slot, row, column) in [
        ("P01", 5, 1),
        ("B01", 3, 1),
        ("C01", 1, 1),
        ("P02", 5, 2),
        ("B02", 3, 2),
    ] {
        let node = &tree.nodes[slot];
        assert_eq!((node.row, node.column), (row, column), "{slot}");
    }
    assert_eq!(
        tree.edges.keys().cloned().collect::<Vec<_>>(),
        [
            ("B01".into(), "C01".into()),
            ("P01".into(), "B01".into()),
            ("P02".into(), "B02".into()),
        ]
    );
    for ((source, target), edge) in &tree.edges {
        let source = &tree.nodes[source];
        let target = &tree.nodes[target];
        assert_eq!(source.row, edge.row + 1);
        assert_eq!(target.row + 1, edge.row);
        assert_eq!((source.column, target.column), (edge.column, edge.column));
    }
}

#[test]
fn input_order_and_slot_spelling_do_not_invent_or_reverse_relationships() {
    let mut disks = vec![
        disk("Z-primary", 1, None),
        disk("A-backup", 2, Some("z-PRIMARY")),
        disk("M-primary", 1, None),
    ];
    let first = read(json!({"disks":disks.clone()})).unwrap();
    disks.reverse();
    let reversed = read(json!({"disks":disks})).unwrap();
    assert_eq!(first, reversed);
    assert!(
        first
            .edges
            .contains_key(&("Z-primary".into(), "A-backup".into()))
    );
    assert_eq!(
        first.nodes["A-backup"].source,
        Some(Source::Registered("Z-primary".into()))
    );
    assert_eq!(
        first.selection(Some("A-backup")).as_deref(),
        Some("A-backup")
    );
    assert_eq!(
        first.selection(Some("removed")).as_deref(),
        Some("M-primary")
    );
}

#[test]
fn missing_sources_and_sparse_large_tiers_remain_visible_without_fabricated_arrows() {
    let mut orphan = disk("orphan", u32::MAX, Some("removed"));
    orphan["online"] = json!(false);
    let tree = read(json!({"disks":[disk("P01",1,None),orphan]})).unwrap();
    assert_eq!((tree.columns, tree.levels), (2, 2));
    assert!(tree.edges.is_empty());
    assert_eq!(
        tree.nodes["orphan"].source,
        Some(Source::Missing("removed".into()))
    );
    assert!(!tree.nodes["orphan"].online);
    assert_eq!(tree.nodes["orphan"].row, 1);
    assert_eq!(tree.nodes["P01"].row, 3);
    assert_eq!(tree.rows_style().matches("auto").count(), 2);
    let empty = read(json!({"disks":[]})).unwrap();
    assert_eq!(empty, Topology::default());
    assert!(empty.selection(Some("P01")).is_none());
}

#[test]
fn displayed_topology_fields_are_required_and_not_defaulted_from_slot_names() {
    for field in ["slot_id", "disk_id", "level", "upstream_slot_id", "online"] {
        let mut entry = disk("P01", 1, None);
        entry.as_object_mut().unwrap().remove(field);
        assert!(
            read(json!({"disks":[entry]})).is_err(),
            "accepted missing {field}"
        );
    }
    for (field, value) in [
        ("level", json!(0)),
        ("level", json!("1")),
        ("level", json!(-1)),
        ("online", json!(null)),
        ("online", json!("true")),
        ("upstream_slot_id", json!([])),
        ("disk_id", json!("")),
        ("slot_id", json!("../bad")),
    ] {
        let mut entry = disk("P01", 1, None);
        entry[field] = value;
        assert!(
            read(json!({"disks":[entry]})).is_err(),
            "accepted invalid {field}"
        );
    }
}

#[test]
fn invalid_roles_edges_and_duplicate_identities_are_rejected_at_ingress() {
    for disks in [
        vec![disk("P01", 1, Some("P01"))],
        vec![disk("B01", 2, None)],
        vec![disk("B01", 2, Some(""))],
        vec![disk("P01", 1, None), disk("p01", 1, None)],
        vec![disk("P01", 1, None), disk("P01", 1, None)],
        vec![disk("P01", 1, None), disk("B01", 3, Some("P01"))],
        vec![disk("B01", 2, Some("C01")), disk("C01", 3, Some("B01"))],
        vec![
            disk("P01", 1, None),
            disk("B01", 2, Some("P01")),
            disk("B02", 2, Some("p01")),
        ],
    ] {
        assert!(read(json!({"disks":disks})).is_err());
    }
    let mut duplicate_id = disk("P02", 1, None);
    duplicate_id["disk_id"] = json!("disk-P01");
    assert!(read(json!({"disks":[disk("P01",1,None),duplicate_id]})).is_err());
}

#[test]
fn topology_errors_keep_the_specific_boundary_source() {
    let error = read(json!({"disks":[disk("P01",1,None),disk("B01",3,Some("P01"))]})).unwrap_err();
    assert!(
        matches!(error.source.as_deref(),Some(api::ErrorSource::DiskDetails(
        DecodeError::Topology(InvalidTopology::Level { slot,upstream })
    )) if slot=="B01"&&upstream=="P01")
    );
    assert_eq!(error.status, 200);
}

#[test]
fn tree_positions_use_cssom_properties_under_the_existing_content_security_policy() {
    let source = include_str!("../../../../src/web/src/admin/disks/tree/view.rs");
    assert!(
        !source.contains("style=move"),
        "whole style attributes are blocked by style-src self"
    );
    assert_eq!(source.matches("style:grid-row=").count(), 3);
    assert_eq!(source.matches("style:grid-column=").count(), 3);
    assert!(source.contains("style:grid-template-rows="));
    assert!(source.contains("style=(\"--zxs-backup-columns\","));
}

#[test]
fn unadmitted_candidates_use_a_separate_bottom_row_without_backup_edges() {
    let tree = read(json!({"disks":[disk("P01",1,None),disk("B01",2,Some("P01"))]})).unwrap();
    assert_eq!(tree.pending_row(), tree.nodes["P01"].row + 2);
    assert_eq!(tree.columns_with_pending(3), 3);
    assert_eq!(tree.rows_with_pending(true).matches("auto").count(), 3);
    assert_eq!(tree.rows_with_pending(false), tree.rows_style());
    assert_eq!(
        tree.edges.len(),
        1,
        "discovery cannot add configured backup arrows"
    );
    let empty = Topology::default();
    assert_eq!(empty.pending_row(), 1);
    assert_eq!(empty.columns_with_pending(2), 2);
    assert_eq!(empty.rows_with_pending(true), "auto");
    assert!(empty.edges.is_empty());
    let source = include_str!("../../../../src/web/src/admin/disks/tree/view.rs");
    assert!(source.contains("data-level=\"-1\""));
    assert!(source.contains("storage::choose_disk(state,&path.get_value())"));
    assert!(source.contains("aria-controls=\"zxs-disk-wizard\""));
}

#[test]
fn tier_labels_keep_registered_levels_instead_of_numbering_backups_from_one() {
    let tree = read(json!({"disks":[
        disk("P01",1,None),disk("B01",2,Some("P01")),
        disk("C01",3,Some("B01")),disk("D01",4,Some("C01")),
    ]}))
    .unwrap();
    for (slot, expected) in [
        ("P01", "Primary · Tier 1"),
        ("B01", "Backup disk · Tier 2"),
        ("C01", "Backup disk · Tier 3"),
        ("D01", "Backup disk · Tier 4"),
    ] {
        assert_eq!(tree.nodes[slot].tier_label(), expected);
    }
}

#[test]
fn sparse_tier_labels_do_not_infer_a_contiguous_layer_or_a_missing_disk() {
    let tree = read(json!({"disks":[
        disk("P01",1,None),disk("orphan",u32::MAX,Some("missing")),
    ]}))
    .unwrap();
    assert_eq!(
        tree.nodes["orphan"].tier_label(),
        "Backup disk · Tier 4294967295"
    );
    assert_eq!(tree.nodes.len(), 2);
    assert_eq!(tree.levels, 2);
    assert!(tree.edges.is_empty());
    assert_eq!(tree.pending_row(), tree.nodes["P01"].row + 2);
}

#[test]
fn candidate_row_does_not_shift_or_connect_either_registered_backup_chain() {
    let tree = read(json!({"disks":[
        disk("P01",1,None),disk("B01",2,Some("P01")),disk("C01",3,Some("B01")),
        disk("P02",1,None),disk("B02",2,Some("P02")),
    ]}))
    .unwrap();
    let registered = tree.clone();
    for candidates in [1, 2, 4] {
        assert_eq!(tree.columns_with_pending(candidates), 2.max(candidates));
        assert_eq!(tree.rows_with_pending(true).matches("auto").count(), 4);
        for node in tree.nodes.values() {
            assert!(node.row < tree.pending_row());
        }
        for ((source, target), edge) in &tree.edges {
            assert_eq!(tree.nodes[source].row, tree.nodes[target].row + 2);
            assert!(edge.row < tree.pending_row());
        }
        assert_eq!(tree, registered);
    }
    assert_eq!(tree.pending_row(), 7);
    assert_eq!(tree.nodes["P01"].row, 5);
    assert_eq!(tree.nodes["P02"].row, 5);
}
