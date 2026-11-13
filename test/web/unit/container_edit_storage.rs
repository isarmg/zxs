use super::*;
use super::super::tests::{preparation, prepared};

fn record(value: &CommitPreparation, format: Format) -> Record {
    let (key, bytes) = encode(value, format).unwrap();
    decode(&key, &value.principal, &bytes).unwrap()
}

#[test]
fn original_intent_recovers_exact_scope_without_claiming_publication() {
    let intent = preparation();
    let restored = assemble(vec![record(&intent, Format::OriginalIntent)], 99)
        .unwrap().unwrap();
    assert_eq!(restored.target.request, intent.target.request);
    assert_eq!(restored.target.entry, intent.target.entry);
    assert_eq!(restored.operation, intent.operation);
    assert_eq!(restored.generation, intent.generation);
    assert_eq!(restored.draft, intent.draft);
    assert_eq!(restored.principal, intent.principal);
    assert_eq!(restored.target.epoch, 99);
    assert!(restored.envelope.is_none());
    assert!(!restored.commit_started);
    assert!(restored.settled.is_none());
}

#[test]
fn frozen_envelope_and_partial_cleanup_recover_original_body_never_reprepare() {
    let intent = prepared();
    for records in [
        vec![record(&intent, Format::OriginalIntent), record(&intent, Format::FrozenEnvelope)],
        vec![record(&intent, Format::FrozenEnvelope)],
    ] {
        let restored = assemble(records, 99).unwrap().unwrap();
        assert_eq!(restored.envelope, intent.envelope);
        assert_eq!(restored.operation, intent.operation);
        assert_eq!(restored.target.request, intent.target.request);
        assert!(restored.commit_started);
        assert!(!restored.in_flight);
        assert!(restored.settled.is_none());
    }
}

#[test]
fn records_reject_other_principal_key_oversize_and_local_completion_claims() {
    let intent = preparation();
    let (key, bytes) = encode(&intent, Format::OriginalIntent).unwrap();
    let other = Principal::new("bob".into()).unwrap();
    assert!(decode(&key, &other, &bytes).is_err());
    assert!(decode("another-key", &intent.principal, &bytes).is_err());
    assert!(decode(&key, &intent.principal, &" ".repeat(MAX_RECORD_BYTES + 1)).is_err());
    let mut modified: json_codec::Value = json_codec::from_str(&bytes).unwrap();
    modified["settled"] = json_codec::json!("published");
    assert!(decode(&key, &intent.principal, &modified.to_string()).is_err());
}

#[test]
fn conflicting_intents_duplicate_records_and_changed_envelopes_fail_closed() {
    let intent = prepared();
    let mut changed = intent.clone();
    changed.target.request.context.path = name_policy::PortablePath::parse("other.txt").unwrap();
    assert!(assemble(vec![
        record(&intent, Format::OriginalIntent), record(&changed, Format::OriginalIntent),
    ], 99).is_err());
    assert!(assemble(vec![
        record(&intent, Format::OriginalIntent), record(&intent, Format::OriginalIntent),
    ], 99).is_err());
    let (key, bytes) = encode(&intent, Format::FrozenEnvelope).unwrap();
    let mut modified: json_codec::Value = json_codec::from_str(&bytes).unwrap();
    modified["envelope"]["container_draft_commit"]["generation"] = json_codec::json!("3");
    assert!(decode(&key, &intent.principal, &modified.to_string()).is_err());
}
