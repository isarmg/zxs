use super::*;
use json_codec::json;

fn page() -> GraphPage {
    GraphPage {
        history: HistorySummary {
            history_tree_id: format!("history_{}", "1".repeat(32)),
            repository_id: format!("repo_{}", "2".repeat(32)),
            root_commit_id: format!("commit_{}", "3".repeat(32)),
            main_branch_id: None,
            generation: u64::MAX,
            state: HistoryState::Active,
            provenance: None,
        },
        history_root: "a".repeat(64),
        items: GraphItems::Branches(Vec::new()),
        next_cursor: None,
    }
}

#[test]
fn graph_limits_reject_zero_and_oversize_without_utf8_stalls() {
    assert!(PageLimit::new(0).is_err());
    assert!(PageLimit::new(129).is_err());
    assert_eq!(PageLimit::new(128).unwrap().get(), 128);
    assert!(TextLimit::new(3).is_err());
    assert!(TextLimit::new(16385).is_err());
    assert_eq!(TextLimit::new(4).unwrap().get(), 4);
}

#[test]
fn graph_wire_preserves_decimal_counts_and_single_collection_identity() {
    let value = json_codec::to_value(page()).unwrap();
    assert_eq!(value["history"]["generation"], u64::MAX.to_string());
    assert_eq!(
        value["items"],
        json!({"collection":"branches","records":[]})
    );
    assert!(value.get("collection").is_none());
    assert_eq!(json_codec::from_value::<GraphPage>(value).unwrap(), page());
}

#[test]
fn graph_required_nullable_fields_reject_omission() {
    let mut value = json_codec::to_value(page()).unwrap();
    value.as_object_mut().unwrap().remove("next_cursor");
    assert!(json_codec::from_value::<GraphPage>(value).is_err());
    let mut value = json_codec::to_value(page()).unwrap();
    value["history"]
        .as_object_mut()
        .unwrap()
        .remove("provenance");
    assert!(json_codec::from_value::<GraphPage>(value).is_err());
}

#[test]
fn graph_wire_rejects_unknown_duplicate_fields_and_object_enum_tokens() {
    let mut value = json_codec::to_value(page()).unwrap();
    value["extra"] = json!(true);
    assert!(json_codec::from_value::<GraphPage>(value).is_err());
    assert!(
        json_codec::from_str::<TextSummary>(r#"{"kind":"inline","text":"a","text":"b"}"#).is_err()
    );
    assert!(json_codec::from_value::<GraphCollection>(json!({"commits":null})).is_err());
    assert!(json_codec::from_value::<CommitField>(json!({"author":null})).is_err());
    assert!(json_codec::from_value::<GraphKey>(json!({"kind":"id","id":"x","extra":1})).is_err());
}

#[test]
fn graph_text_inline_threshold_is_utf8_bytes() {
    let short = "🦀".repeat(64);
    assert_eq!(
        TextSummary::from_text(&short),
        TextSummary::Inline { text: short }
    );
    assert_eq!(
        TextSummary::from_text(&"🦀".repeat(65)),
        TextSummary::Deferred { bytes: 260 }
    );
    assert_eq!(
        TextSummary::from_text(""),
        TextSummary::Inline {
            text: String::new()
        }
    );
}

#[test]
fn graph_auxiliary_null_is_not_a_missing_field() {
    let page = TextPage {
        history_tree_id: "history".into(),
        history_root: "a".repeat(64),
        target: TextTarget::History {
            field: HistoryField::Provenance,
        },
        total_bytes: 0,
        offset: 0,
        text: None,
        next_offset: None,
    };
    let value = json_codec::to_value(&page).unwrap();
    assert_eq!(value["total_bytes"], "0");
    assert_eq!(value["text"], json!(null));
    assert_eq!(
        json_codec::from_value::<TextPage>(value.clone()).unwrap(),
        page
    );
    for field in ["text", "next_offset"] {
        let mut missing = value.clone();
        missing.as_object_mut().unwrap().remove(field);
        assert!(json_codec::from_value::<TextPage>(missing).is_err());
    }
}
