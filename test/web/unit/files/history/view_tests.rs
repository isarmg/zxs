use super::*;
use crate::files::history::{FileNode, NodeKind, ParentSet};
use storage_contract::history_window::{
    HistoryBranchId, HistoryFieldInfo, HistoryMergeInputMember, HistoryNodeFields, HistoryNodeId,
};

fn fragment(offset: u64, total: u64, text: &str, end: bool) -> FieldFragment {
    FieldFragment {
        node: "node".into(),
        field: HistoryField::Message,
        total,
        offset,
        text: text.into(),
        end,
        next: (!end).then(|| "next-fragment".into()),
    }
}

#[test]
fn a_last_fragment_does_not_claim_the_whole_field_is_present() {
    let first = fragment(0, 9, "start", false);
    let last = fragment(5, 9, "tail", true);
    assert!(!field_complete(&first));
    assert!(!field_complete(&last));
    assert_ne!(field_status(&first), field_status(&last));
    let whole = fragment(0, 9, "starttail", true);
    assert!(field_complete(&whole));
    assert_ne!(field_status(&last), field_status(&whole));
}

#[test]
fn field_ranges_count_utf8_bytes_and_explicitly_accept_an_empty_complete_field() {
    let part = fragment(4, 9, "文é", true);
    assert_eq!(field_range(&part), "UTF-8 bytes [4, 9) / 9");
    assert!(!field_complete(&part));
    let empty = fragment(0, 0, "", true);
    assert_eq!(field_range(&empty), "UTF-8 bytes [0, 0) / 0");
    assert!(field_complete(&empty));
}

#[test]
fn an_old_fragment_key_cannot_act_on_a_replacement_fragment() {
    let old = fragment(0, 9, "start", false);
    let key = FieldKey::new(&old);
    let mut history = FileHistory {
        fragment: Some(old),
        ..Default::default()
    };
    assert!(key.matches(&history));
    history.fragment = Some(fragment(5, 9, "tail", true));
    assert!(!key.matches(&history));
    history.close_fragment();
    assert!(!key.matches(&history));
}

fn node(ordinal: u64) -> FileNode {
    FileNode {
        id: format!("fmeta_{ordinal:032x}"),
        version: Some(format!("ver_{ordinal:032x}")),
        sequence: ordinal,
        kind: NodeKind::Tags,
        note: String::new(),
        body_note: None,
        tags: Vec::new(),
        path: None,
        previous_path: None,
        parents: Vec::new(),
        parent_set: ParentSet::Complete,
        materialized: true,
        inherited: false,
        fields: HistoryNodeFields {
            message: HistoryFieldInfo { byte_length: 0 },
            author: HistoryFieldInfo { byte_length: 0 },
            created_at: HistoryFieldInfo { byte_length: 0 },
        },
    }
}

#[test]
fn merge_input_rows_preserve_ordinal_branch_and_node_pairs_in_response_order() {
    let history = FileHistory {
        nodes: vec![node(1)].into(),
        ..Default::default()
    };
    let inputs = [
        HistoryMergeInputMember {
            ordinal: 11,
            branch_id: HistoryBranchId::parse(&format!("br_{:032x}", 3)).unwrap(),
            node_id: HistoryNodeId::parse(&node(2).id).unwrap(),
        },
        HistoryMergeInputMember {
            ordinal: 12,
            branch_id: HistoryBranchId::parse(&format!("br_{:032x}", 4)).unwrap(),
            node_id: HistoryNodeId::parse(&node(1).id).unwrap(),
        },
    ];
    let rows = input_rows(&history, &inputs);
    assert_eq!(rows.len(), 2);
    for (row, input) in rows.iter().zip(&inputs) {
        assert_eq!(row.label, format!("#{} · {} → {}", input.ordinal, input.branch_id, input.node_id));
        assert_eq!(row.node.as_deref(), Some(input.node_id.as_str()));
    }
    assert!(rows[0].outside);
    assert!(!rows[1].outside);
}

#[test]
fn a_separate_current_summary_is_still_outside_the_loaded_node_window() {
    let current = node(2);
    let history = FileHistory {
        nodes: vec![node(1)].into(),
        current_node: Some(current.clone()),
        ..Default::default()
    };
    assert!(history.node(current.id()).is_some());
    let row = relation_node(&history, current.id().to_owned(), current.id().to_owned());
    assert!(row.outside);
    assert_eq!(history.nodes().len(), 1);
}
