//! Transfer boundary cases; no browser storage, clipboard or network.
use super::*;

const ORIGIN: &str = "https://zxs.example:8443";
fn record() -> Record {
    Record {
        format: Format::Request, principal: "admin".into(),
        operation: format!("op_{}", "a".repeat(32)).parse().unwrap(),
        file_id: HistoryFileId::parse(&format!("file_{}", "b".repeat(32))).unwrap(),
        history_root: HistoryRoot::parse(&"c".repeat(64)).unwrap(),
        version_ids: vec![HistoryVersionId::parse(&format!("ver_{}", "d".repeat(32))).unwrap()],
        label: "private-name.txt".into(),
    }
}
fn state() -> State {
    let mut state = State { authenticated: true, principal: "admin".into(), session_epoch: 4, ..Default::default() };
    state.management.storage = crate::management::Storage::Ready;
    state
}
fn encoded() -> String { Transfer::encode(ORIGIN.into(), record()).unwrap() }

#[test]
fn transfer_round_trip_keeps_exact_original_intent_without_success_or_secrets() {
    let bytes = encoded();
    let parsed = Transfer::decode(&bytes, ORIGIN, "admin").unwrap();
    assert_eq!(parsed.record, record());
    assert_eq!(parsed.origin, ORIGIN);
    let restored = parsed.record.restored_attempt(4).unwrap();
    assert!(restored.uncertain && restored.restored && !restored.retry_confirmed);
    assert!(restored.cleanup.is_none() && restored.separation.is_none());
    assert!(!restored.retry_allowed(&state()));
    for forbidden in ["password", "token", "cleanup", "published", "body"] { assert!(!bytes.contains(forbidden)); }
}

#[test]
fn transfer_cannot_retarget_scheme_host_port_or_account() {
    let bytes = encoded();
    for origin in ["http://zxs.example:8443", "https://other.example:8443", "https://zxs.example", "https://zxs.example:8443/path"] {
        assert!(Transfer::decode(&bytes, origin, "admin").is_err());
    }
    assert!(Transfer::decode(&bytes, ORIGIN, "other-admin").is_err());
}

#[test]
fn transfer_rejects_unknown_fields_duplicate_fields_arrays_and_extra_authority() {
    let bytes = encoded();
    for (field, value) in [
        ("format", json!("old-retirement-transfer")),
        ("completed", json!(true)),
        ("record", json!([])),
    ] {
        let mut changed: Value = json_codec::from_str(&bytes).unwrap(); changed[field] = value;
        assert!(Transfer::decode(&changed.to_string(), ORIGIN, "admin").is_err());
    }
    let duplicate = bytes.replacen("{", "{\"origin\":\"https://zxs.example:8443\",", 1);
    assert!(Transfer::decode(&duplicate, ORIGIN, "admin").is_err());
    assert!(Transfer::decode("[]", ORIGIN, "admin").is_err());
    let mut changed: Value = json_codec::from_str(&bytes).unwrap();
    changed["record"]["cleanup"] = json!("verified_absent");
    assert!(Transfer::decode(&changed.to_string(), ORIGIN, "admin").is_err());
}

#[test]
fn transfer_budget_and_original_selection_remain_strict() {
    assert!(Transfer::decode(&" ".repeat(MAX_TRANSFER_BYTES + 1), ORIGIN, "admin").is_err());
    let bytes = encoded();
    for value in [json!([]), json!([record().version_ids[0], record().version_ids[0]])] {
        let mut changed: Value = json_codec::from_str(&bytes).unwrap(); changed["record"]["version_ids"] = value;
        assert!(Transfer::decode(&changed.to_string(), ORIGIN, "admin").is_err());
    }
    let mut changed: Value = json_codec::from_str(&bytes).unwrap();
    changed["record"]["operation"] = json!("invented-operation");
    assert!(Transfer::decode(&changed.to_string(), ORIGIN, "admin").is_err());
}

#[test]
fn resume_only_mode_survives_transfer_and_never_authorizes_an_unknown_submit() {
    let mut original = record(); original.format = Format::ResumeOnly;
    let parsed = Transfer::decode(&Transfer::encode(ORIGIN.into(), original.clone()).unwrap(), ORIGIN, "admin").unwrap();
    assert_eq!(parsed.record, original);
    let mut restored = parsed.record.restored_attempt(4).unwrap(); restored.retry_confirmed = true;
    assert!(restored.resume_only);
    assert!(!restored.retry_allowed(&state()));
    assert!(!restored.dismiss_allowed(&state()));
}

#[test]
fn cached_final_id_observation_cannot_authenticate_newly_pasted_content() {
    let record = record();
    let owner = (4, "admin".into());
    let mut state = state();
    assert!(can_import(&state, &owner, &record));
    state.files.operations.push(FileOperation::unknown(&record.operation.to_string()));
    assert!(can_import(&state, &owner, &record));
    state.files.operations = vec![FileOperation::rejected(&record.operation.to_string())];
    assert!(!can_import(&state, &owner, &record));
    let receipt = json!({
        "operation_id": record.operation, "operation_kind": "file.nodes.retire", "request_digest": "e".repeat(64),
        "state": "completed", "commit_state": "PUBLISHED", "protection_at_publish": "NOT_APPLICABLE",
        "publication_sequence": "7", "result": {}, "error": null,
    });
    state.files.operations = vec![FileOperation::from_receipt(&record.operation.to_string(), &receipt)];
    assert!(!can_import(&state, &owner, &record));
}

#[test]
fn edited_and_previous_session_controls_return_before_storage_or_browser_access() {
    Owner::new().with(|| {
        let mut model = Import {
            owner: Some((4,"admin".into())), generation: 3,
            candidate: Some(Transfer::decode(&encoded(), ORIGIN, "admin").unwrap()), ..Default::default()
        };
        model.edit("changed".into());
        assert!(model.candidate.is_none());
        model.candidate = Some(Transfer::decode(&encoded(), ORIGIN, "admin").unwrap());
        let mut snapshot = state(); snapshot.session_epoch = 5;
        let state = RwSignal::new(snapshot); let model = RwSignal::new(model);
        accept(state, model, 3); // old input generation
        accept(state, model, 4); // old session
        assert!(export(state, 4, &record()).unwrap().is_none());
        assert!(model.with_untracked(|model| model.saved.is_none()));
        assert!(state.with_untracked(|s| s.files.node_retirement.attempt.is_none()));
    });
}

#[test]
fn recovery_text_uses_a_native_non_void_multiline_control() {
    use web_interface::html::{ElementType, Textarea};
    fn native_textarea<T: ElementType<Output = web_sys::HtmlTextAreaElement>>() {}
    native_textarea::<Textarea>();
    assert_eq!(Textarea::TAG, "textarea");
    assert!(!Textarea::SELF_CLOSING);
    assert!(Textarea::ESCAPE_CHILDREN);
    // Construct both supported controls without mounting a DOM on the host.
    let _export = view! { <textarea readonly rows="6" prop:value="original request"/> };
    let _import = view! {
        <textarea rows="6" maxlength=MAX_TRANSFER_BYTES autocomplete="off"
            spellcheck="false" prop:value=String::new()/>
    };
}
