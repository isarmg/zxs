use super::*;
use json_codec::json;

pub(super) fn page() -> GraphPage {
    GraphPage {
        history: HistorySummary {
            history_tree_id: "history-one".into(),
            repository_id: "repo-one".into(),
            root_commit_id: "commit-one".into(),
            main_branch_id: None,
            generation: 1,
            state: storage_contract::HistoryState::Active,
            provenance: None,
        },
        history_root: "a".repeat(64),
        items: GraphItems::Branches(Vec::new()),
        next_cursor: None,
    }
}
fn admit(page: &GraphPage) -> Result<GraphPage, api::Error> {
    wire::main_page(
        200,
        &json_codec::to_vec(page).unwrap(),
        "history-one",
        GraphCollection::Branches,
        Some(&"a".repeat(64)),
    )
}
#[test]
fn graph_main_response_binds_history_root_collection_and_bounded_cursor() {
    assert!(admit(&page()).is_ok());
    let mut wrong = page();
    wrong.history.history_tree_id = "other".into();
    assert!(admit(&wrong).is_err());
    let mut wrong = page();
    wrong.history_root = "b".repeat(64);
    assert!(admit(&wrong).is_err());
    let mut wrong = page();
    wrong.items = GraphItems::Commits(Vec::new());
    assert!(admit(&wrong).is_err());
    let mut wrong = page();
    wrong.next_cursor = Some("cursor".into());
    assert!(admit(&wrong).is_err());
    let mut wrong = page();
    wrong.history.provenance = Some(TextSummary::Inline {
        text: "x".repeat(257),
    });
    assert!(admit(&wrong).is_err());
    let mut wrong = page();
    wrong.history.provenance = Some(TextSummary::Deferred { bytes: 256 });
    assert!(admit(&wrong).is_err());
}
#[test]
fn graph_direct_decode_rejects_duplicate_unknown_and_missing_nullable_fields() {
    let original = json_codec::to_string(&page()).unwrap();
    let duplicate = original.replacen(
        "\"generation\":\"1\"",
        "\"generation\":\"1\",\"generation\":\"2\"",
        1,
    );
    assert_ne!(original, duplicate);
    assert!(
        wire::main_page(
            200,
            duplicate.as_bytes(),
            "history-one",
            GraphCollection::Branches,
            None
        )
        .is_err()
    );
    let mut value = json_codec::to_value(page()).unwrap();
    value["extra"] = json!(true);
    assert!(
        wire::main_page(
            200,
            &json_codec::to_vec(&value).unwrap(),
            "history-one",
            GraphCollection::Branches,
            None
        )
        .is_err()
    );
    value.as_object_mut().unwrap().remove("extra");
    value.as_object_mut().unwrap().remove("next_cursor");
    assert!(
        wire::main_page(
            200,
            &json_codec::to_vec(&value).unwrap(),
            "history-one",
            GraphCollection::Branches,
            None
        )
        .is_err()
    );
}
#[test]
fn graph_direct_decode_retains_wire_error_source_and_http_error_identity() {
    use std::error::Error as _;
    let error =
        wire::main_page(200, b"{", "history-one", GraphCollection::Branches, None).unwrap_err();
    assert_eq!(error.code, storage_contract::ErrorCode::InvalidResponse);
    assert!(error.source().is_some());
    let body = json!({"error":{"code":"HISTORY_CHANGED","message":"changed","retryable":false,"request_id":"request-one"}});
    let error = wire::main_page(
        409,
        &json_codec::to_vec(&body).unwrap(),
        "history-one",
        GraphCollection::Branches,
        None,
    )
    .unwrap_err();
    assert_eq!(error.status, 409);
    assert_eq!(error.code, storage_contract::ErrorCode::HistoryChanged);
    assert_eq!(error.request_id.as_deref(), Some("request-one"));
    let oversized = vec![b' '; storage_contract::client_protocol::MAX_HISTORY_PAGE_BYTES + 1];
    assert!(
        wire::main_page(
            200,
            &oversized,
            "history-one",
            GraphCollection::Branches,
            None
        )
        .is_err()
    );
}
fn text_page(text: Option<String>, total: u64, next: Option<u64>) -> TextPage {
    TextPage {
        history_tree_id: "history-one".into(),
        history_root: "a".repeat(64),
        target: TextTarget::History {
            field: HistoryField::Provenance,
        },
        total_bytes: total,
        offset: 0,
        text,
        next_offset: next,
    }
}
fn text_admit(page: &TextPage) -> Result<Detail, api::Error> {
    wire::detail(
        200,
        &json_codec::to_vec(page).unwrap(),
        "history-one",
        &"a".repeat(64),
        &DetailRequest::Text {
            target: TextTarget::History {
                field: HistoryField::Provenance,
            },
            offset: 0,
        },
    )
}
#[test]
fn graph_text_preserves_absence_empty_and_utf8_byte_continuation() {
    for value in [None, Some(String::new())] {
        let page = text_page(value.clone(), 0, None);
        let Detail::Text(admitted) = text_admit(&page).unwrap() else {
            panic!("text response");
        };
        assert_eq!(admitted.text, value);
    }
    assert!(text_admit(&text_page(Some("🦀".into()), 8, Some(4))).is_ok());
    assert!(text_admit(&text_page(Some("🦀".into()), 8, Some(1))).is_err());
    assert!(text_admit(&text_page(Some(String::new()), 8, Some(0))).is_err());
    assert!(text_admit(&text_page(None, 8, None)).is_err());
    let mut wrong = text_page(Some("x".into()), 1, None);
    wrong.history_root = "b".repeat(64);
    assert!(text_admit(&wrong).is_err());
    let mut wrong = text_page(Some("x".into()), 1, None);
    wrong.target = TextTarget::Commit {
        id: "commit".into(),
        field: CommitField::Author,
    };
    assert!(text_admit(&wrong).is_err());
}
#[test]
fn graph_pending_requires_sorted_unique_records_and_exact_continuation() {
    let target = PendingTarget::Branch {
        id: "branch".into(),
    };
    let mut page = PendingPage {
        history_tree_id: "history-one".into(),
        history_root: "a".repeat(64),
        target: target.clone(),
        records: vec![
            PendingRecord {
                file_id: "file-b".into(),
                version_id: "ver".into(),
            },
            PendingRecord {
                file_id: "file-c".into(),
                version_id: "ver".into(),
            },
        ],
        next_after: Some("file-c".into()),
    };
    let request = DetailRequest::Pending {
        target,
        after: Some("file-a".into()),
    };
    let admit = |page: &PendingPage| {
        wire::detail(
            200,
            &json_codec::to_vec(page).unwrap(),
            "history-one",
            &"a".repeat(64),
            &request,
        )
    };
    assert!(admit(&page).is_ok());
    page.next_after = Some("file-z".into());
    assert!(admit(&page).is_err());
    page.next_after = None;
    page.records.swap(0, 1);
    assert!(admit(&page).is_err());
    page.records.swap(0, 1);
    page.records[0].file_id = "file-a".into();
    assert!(admit(&page).is_err());
}
#[test]
fn graph_deletions_preserve_order_duplicates_timestamp_and_bounded_offsets() {
    let mut page: DeletionsPage = json_codec::from_value(json!({
        "history_tree_id":"history-one", "history_root":"a".repeat(64), "commit_id":"commit", "timestamp":"7",
        "total":"3", "offset":"0", "records":[{"path":"a","kind":"file"},{"path":"a","kind":"file"}], "next_offset":"2"
    })).unwrap();
    let request = DetailRequest::Deletions {
        commit: "commit".into(),
        offset: 0,
    };
    let admit = |page: &DeletionsPage| {
        wire::detail(
            200,
            &json_codec::to_vec(page).unwrap(),
            "history-one",
            &"a".repeat(64),
            &request,
        )
    };
    let Detail::Deletions(valid) = admit(&page).unwrap() else {
        panic!("deletions response");
    };
    assert_eq!(valid.records, page.records);
    assert_eq!(valid.timestamp, Some(7));
    page.next_offset = Some(1);
    assert!(admit(&page).is_err());
    page.next_offset = Some(2);
    page.timestamp = None;
    assert!(admit(&page).is_err());
}
#[test]
fn graph_paths_encode_identifiers_and_root_bound_auxiliary_queries() {
    assert_eq!(
        main_path("history /", GraphCollection::MergeLinks, Some("cursor+ /")),
        "/api/history-trees/history%20%2F/graph?collection=merge_links&limit=50&cursor=cursor%2B%20%2F"
    );
    let request = DetailRequest::Text {
        target: TextTarget::Commit {
            id: "id /".into(),
            field: CommitField::Message,
        },
        offset: 4,
    };
    assert_eq!(
        detail_path("h", "root", &request),
        "/api/history-trees/h/graph/commits/id%20%2F/fields/message?history_root=root&offset=4&limit=16384"
    );
    let request = DetailRequest::Pending {
        target: PendingTarget::Branch { id: "b".into() },
        after: Some("file /".into()),
    };
    assert_eq!(
        detail_path("h", "root", &request),
        "/api/history-trees/h/graph/branches/b/pending?history_root=root&limit=128&after=file%20%2F"
    );
}
#[test]
fn graph_replacing_page_and_detail_releases_previous_allocations() {
    let mut model = Model::new("history-one".into());
    let old: Snapshot<GraphPage> = page().into();
    let weak = old.weak();
    model.page = Some(old);
    model.page = Some(page().into());
    assert!(weak.upgrade().is_none());
    let old: Snapshot<Detail> = Detail::Text(text_page(Some("first".into()), 5, None)).into();
    let weak = old.weak();
    model.detail = Some(old);
    model.detail = Some(Detail::Text(text_page(Some("second".into()), 6, None)).into());
    assert!(weak.upgrade().is_none());
    let main_ticket = model.main_read.ticket;
    let detail_ticket = model.detail_read.ticket;
    model.cancel();
    assert_ne!(model.main_read.ticket, main_ticket);
    assert_ne!(model.detail_read.ticket, detail_ticket);
}
#[test]
fn graph_root_change_keeps_page_readable_but_discards_detail_and_requires_reload() {
    let mut model = Model::new("history-one".into());
    model.page = Some(page().into());
    model.detail = Some(Detail::Text(text_page(None, 0, None)).into());
    model.reject(
        storage_contract::client_protocol::ResponseError::new(
            409,
            storage_contract::ErrorCode::HistoryChanged,
            "changed",
        )
        .into(),
    );
    assert!(model.stale);
    assert!(model.page.is_some());
    assert!(model.detail.is_none());
}

#[test]
fn graph_obsolete_reads_cannot_hide_current_session_expiry_or_revoke_new_login() {
    Owner::new().with(|| {
        let state = RwSignal::new(State {
            authenticated: true,
            session_epoch: 3,
            ..Default::default()
        });
        let model = RwSignal::new(Model::new("history-one".into()));
        model.update(Model::cancel);
        let expired: Result<(), api::Error> =
            Err(storage_contract::client_protocol::ResponseError::new(
                401,
                storage_contract::ErrorCode::HttpError,
                "expired",
            )
            .into());
        assert!(!accepts(state, model, 2, Ticket::Main(0), &expired));
        assert!(state.with_untracked(State::session_active));
        assert!(!accepts(state, model, 3, Ticket::Detail(0), &Ok(())));
        assert!(!accepts(state, model, 3, Ticket::Main(0), &expired));
        assert!(!state.with_untracked(State::session_active));
    });
}
