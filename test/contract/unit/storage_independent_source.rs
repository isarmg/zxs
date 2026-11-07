use super::*;
use json_codec::{Value, json};

fn hash(n: u64) -> String {
    format!("{n:064x}")
}
fn identity(name: &str, role: &str) -> Value {
    json!({"origin":{"pool_id":"original_pool","disk_id":name,"slot_id":name,
        "binding_generation":"7"},"incarnation":hash(9),"role":role})
}
fn checkpoint(name: &str, role: &str, publication: Option<u64>, sequence: &str) -> Value {
    json!({"identity":identity(name,role),"publication":publication.map(hash),"sequence":sequence})
}
fn sample() -> Value {
    json!({
        "protocol":"independent_source_observation",
        "node":{"root":checkpoint("tertiary","tertiary",Some(90),"1"),
            "identity":identity("primary","primary"),"pins":[hash(1),hash(2)]},
        "observations":[{"checkpoint":checkpoint("primary","primary",Some(3),"3"),
            "status":{"kind":"satisfies_pinned_checkpoints"}}],
        "physical_candidates":1,
        "status":{"kind":"incomplete_observation","matching_candidates":1}
    })
}
fn read(value: Value) -> Result<IndependentSourceObservation, DecodeObservationError> {
    decode_observation(&json_codec::to_vec(&value).unwrap())
}
fn roundtrip(value: Value) -> Value {
    json_codec::to_value(read(value).unwrap()).unwrap()
}
fn reject(value: Value) {
    assert!(read(value).is_err());
}

#[test]
fn independent_observation_roundtrip_preserves_full_width_sequence_and_identity() {
    let mut value = sample();
    value["node"]["root"]["sequence"] = "18446744073709551615".into();
    value["observations"][0]["checkpoint"]["sequence"] = "9007199254740993".into();
    let parsed = read(value.clone()).unwrap();
    assert_eq!(parsed.node().root.sequence, u64::MAX);
    assert_eq!(
        parsed.observations()[0].checkpoint.sequence,
        9007199254740993
    );
    assert_eq!(json_codec::to_value(parsed).unwrap(), value);
}

#[test]
fn independent_tokens_do_not_require_legacy_pool_or_slot_identifier_formats() {
    let value = sample();
    assert_eq!(roundtrip(value.clone()), value);
    let longest = "A".repeat(96);
    for text in ["D01", "pool-name", "_", longest.as_str()] {
        assert_eq!(IndependentToken::parse(text).unwrap().as_str(), text);
    }
}

#[test]
fn alias_rows_are_preserved_without_counting_them_as_distinct_matching_disks() {
    let mut value = sample();
    let row = value["observations"][0].clone();
    value["observations"].as_array_mut().unwrap().push(row);
    let parsed = read(value.clone()).unwrap();
    assert_eq!(parsed.observations().len(), 2);
    assert_eq!(parsed.physical_candidates(), 1);
    assert_eq!(
        parsed.status(),
        BatchStatus::IncompleteObservation {
            matching_candidates: 1
        }
    );
    assert_eq!(json_codec::to_value(parsed).unwrap(), value);
}

#[test]
fn two_matching_clones_remain_ambiguous_and_unselected() {
    let mut value = sample();
    let row = value["observations"][0].clone();
    value["observations"].as_array_mut().unwrap().push(row);
    value["physical_candidates"] = 2.into();
    value["status"] = json!({"kind":"ambiguous","matching_candidates":2});
    assert_eq!(
        read(value).unwrap().status(),
        BatchStatus::Ambiguous {
            matching_candidates: 2
        }
    );
}

#[test]
fn zero_or_one_matching_candidate_never_becomes_unique_or_absent() {
    let mut value = sample();
    assert_eq!(
        read(value.clone()).unwrap().status(),
        BatchStatus::IncompleteObservation {
            matching_candidates: 1
        }
    );
    value["observations"][0]["checkpoint"]["identity"]["origin"]["pool_id"] = "another_pool".into();
    value["observations"][0]["status"] = json!({"kind":"identity_mismatch"});
    value["status"]["matching_candidates"] = 0.into();
    assert_eq!(
        read(value).unwrap().status(),
        BatchStatus::IncompleteObservation {
            matching_candidates: 0
        }
    );
}

#[test]
fn empty_input_is_not_observed_only_with_zero_physical_candidates() {
    let mut value = sample();
    value["observations"] = json!([]);
    value["physical_candidates"] = 0.into();
    value["status"] = json!({"kind":"not_observed"});
    assert_eq!(
        read(value.clone()).unwrap().status(),
        BatchStatus::NotObserved
    );
    value["physical_candidates"] = 1.into();
    reject(value);
}

#[test]
fn empty_tertiary_root_keeps_its_original_role_and_unpinned_status() {
    let mut value = sample();
    let empty = checkpoint("tertiary", "tertiary", None, "0");
    value["node"]["root"] = empty.clone();
    value["node"]["identity"] = empty["identity"].clone();
    value["node"]["pins"] = json!([]);
    value["observations"][0] = json!({"checkpoint":empty,"status":{"kind":"unpinned_node"}});
    value["status"]["matching_candidates"] = 0.into();
    assert_eq!(
        read(value).unwrap().node().identity.role,
        IndependentRole::Tertiary
    );
}

#[test]
fn missing_pin_reasons_are_retained_without_claiming_body_availability() {
    let mut value = sample();
    value["observations"][0]["checkpoint"]["publication"] = hash(1).into();
    value["observations"][0]["checkpoint"]["sequence"] = "1".into();
    value["observations"][0]["status"] =
        json!({"kind":"missing_pinned_checkpoints","missing":[hash(2)]});
    value["status"]["matching_candidates"] = 0.into();
    assert_eq!(roundtrip(value.clone()), value);
}

#[test]
fn empty_candidate_checkpoint_must_report_every_required_pin_missing() {
    let mut value = sample();
    value["observations"][0]["checkpoint"] = checkpoint("primary", "primary", None, "0");
    value["observations"][0]["status"] =
        json!({"kind":"missing_pinned_checkpoints","missing":[hash(1),hash(2)]});
    value["status"]["matching_candidates"] = 0.into();
    assert!(read(value.clone()).is_ok());
    value["observations"][0]["status"]["missing"] = json!([hash(2)]);
    reject(value);
}

#[test]
fn changing_any_full_identity_component_requires_identity_mismatch() {
    for field in [
        "pool_id",
        "disk_id",
        "slot_id",
        "binding_generation",
        "incarnation",
        "role",
    ] {
        let mut value = sample();
        let candidate = &mut value["observations"][0]["checkpoint"]["identity"];
        match field {
            "binding_generation" => candidate["origin"][field] = "8".into(),
            "incarnation" => candidate[field] = hash(10).into(),
            "role" => candidate[field] = "secondary".into(),
            _ => candidate["origin"][field] = "different".into(),
        }
        reject(value.clone());
        value["observations"][0]["status"] = json!({"kind":"identity_mismatch"});
        value["status"]["matching_candidates"] = 0.into();
        assert!(read(value).is_ok(), "{field}");
    }
}

#[test]
fn status_must_agree_with_identity_and_nonempty_pin_requirements() {
    for status in [
        json!({"kind":"identity_mismatch"}),
        json!({"kind":"unpinned_node"}),
    ] {
        let mut value = sample();
        value["observations"][0]["status"] = status;
        value["status"]["matching_candidates"] = 0.into();
        reject(value);
    }
    let mut value = sample();
    value["observations"][0]["checkpoint"]["sequence"] = "1".into();
    reject(value);
}

#[test]
fn impossible_physical_and_matching_counts_are_rejected() {
    for (physical, matching, kind) in [
        (0, 1, "incomplete_observation"),
        (2, 1, "incomplete_observation"),
        (1, 0, "incomplete_observation"),
        (1, 2, "ambiguous"),
        (1, 1, "ambiguous"),
        (1, 2, "incomplete_observation"),
        (128, 128, "ambiguous"),
    ] {
        let mut value = sample();
        value["physical_candidates"] = physical.into();
        value["status"] = json!({"kind":kind,"matching_candidates":matching});
        reject(value);
    }
    let mut value = sample();
    let row = value["observations"][0].clone();
    value["observations"].as_array_mut().unwrap().push(row);
    value["physical_candidates"] = 2.into();
    reject(value);
}

#[test]
fn node_pins_are_canonical_unique_and_bounded() {
    for pins in [
        json!([]),
        json!([hash(2), hash(1)]),
        json!([hash(1), hash(1)]),
        json!((0..=MAX_NODE_PINS as u64).map(hash).collect::<Vec<_>>()),
    ] {
        let mut value = sample();
        value["node"]["pins"] = pins;
        reject(value);
    }
}

#[test]
fn exact_pin_budget_is_accepted_without_claiming_graph_membership() {
    let mut value = sample();
    value["node"]["root"]["publication"] = hash(u64::MAX).into();
    value["node"]["pins"] = json!((1..=MAX_NODE_PINS as u64).map(hash).collect::<Vec<_>>());
    value["observations"] = json!([]);
    value["physical_candidates"] = 0.into();
    value["status"] = json!({"kind":"not_observed"});
    assert_eq!(read(value).unwrap().node().pins.len(), MAX_NODE_PINS);
}

#[test]
fn ancestor_nodes_cannot_invent_same_or_higher_roles_or_conflicting_origins() {
    for change in 0..4 {
        let mut value = sample();
        match change {
            0 => value["node"]["identity"]["role"] = "tertiary".into(),
            1 => {
                value["node"]["identity"]["origin"] =
                    value["node"]["root"]["identity"]["origin"].clone()
            }
            2 => value["node"]["root"] = checkpoint("tertiary", "tertiary", None, "0"),
            3 => {
                value["node"]["identity"] = value["node"]["root"]["identity"].clone();
                value["node"]["pins"] = json!([hash(90), hash(91)]);
            }
            _ => unreachable!(),
        }
        reject(value);
    }
}

#[test]
fn root_node_retains_exactly_its_captured_publication() {
    let mut value = sample();
    value["node"]["identity"] = value["node"]["root"]["identity"].clone();
    value["node"]["pins"] = json!([hash(90)]);
    value["observations"] = json!([]);
    value["physical_candidates"] = 0.into();
    value["status"] = json!({"kind":"not_observed"});
    assert!(read(value.clone()).is_ok());
    value["node"]["pins"] = json!([hash(91)]);
    reject(value);
}

#[test]
fn checkpoint_null_is_required_and_agrees_with_zero_sequence() {
    for root in [true, false] {
        for change in 0..3 {
            let mut value = sample();
            let point = if root {
                &mut value["node"]["root"]
            } else {
                &mut value["observations"][0]["checkpoint"]
            };
            match change {
                0 => {
                    point.as_object_mut().unwrap().remove("publication");
                }
                1 => point["publication"] = Value::Null,
                2 => point["sequence"] = "0".into(),
                _ => unreachable!(),
            }
            reject(value);
        }
    }
}

#[test]
fn malformed_independent_tokens_are_rejected_at_the_scalar_boundary() {
    for value in [
        "".to_owned(),
        "x".repeat(97),
        "pool/child".into(),
        "磁盘".into(),
        "a b".into(),
        "a\0b".into(),
    ] {
        assert_eq!(
            IndependentToken::parse(&value),
            Err(InvalidObservation::Token)
        );
        let mut wire = sample();
        wire["node"]["identity"]["origin"]["pool_id"] = value.into();
        reject(wire);
    }
}

#[test]
fn binding_generation_is_canonical_nonzero_decimal_text() {
    for invalid in [
        json!(0),
        json!(7),
        json!(null),
        json!("0"),
        json!("07"),
        json!("+7"),
        json!("-7"),
        json!(" 7"),
        json!("18446744073709551616"),
    ] {
        let mut value = sample();
        value["node"]["identity"]["origin"]["binding_generation"] = invalid;
        reject(value);
    }
    assert_eq!(
        BindingGeneration::new(0),
        Err(InvalidObservation::Generation)
    );
    assert_eq!(BindingGeneration::new(u64::MAX).unwrap().get(), u64::MAX);
}

#[test]
fn checkpoint_sequence_cannot_be_a_lossy_number_or_noncanonical_text() {
    for invalid in [
        json!(3),
        json!(null),
        json!("03"),
        json!("+3"),
        json!("3 "),
        json!("18446744073709551616"),
    ] {
        let mut value = sample();
        value["observations"][0]["checkpoint"]["sequence"] = invalid;
        reject(value);
    }
}

#[test]
fn digest_scalars_reject_uppercase_lengths_and_foreign_domains() {
    for invalid in [
        "a".repeat(63),
        "a".repeat(65),
        "A".repeat(64),
        "z".repeat(64),
        format!("sha256:{}", hash(1)),
    ] {
        assert_eq!(
            PublicationId::parse(&invalid),
            Err(InvalidObservation::Digest)
        );
        assert_eq!(
            Incarnation::parse(&invalid),
            Err(InvalidObservation::Digest)
        );
    }
    assert_eq!(PublicationId::parse(&hash(1)).unwrap().as_str(), hash(1));
    assert_eq!(Incarnation::parse(&hash(1)).unwrap().as_str(), hash(1));
}

#[test]
fn protocol_and_role_are_exact_strings_not_external_enum_objects() {
    for invalid in [
        json!("foreign_preview"),
        json!(null),
        json!({"independent_source_observation":null}),
    ] {
        let mut value = sample();
        value["protocol"] = invalid;
        reject(value);
    }
    for invalid in [json!("level_1"), json!(1), json!({"primary":null})] {
        let mut value = sample();
        value["node"]["identity"]["role"] = invalid;
        reject(value);
    }
}

#[test]
fn surplus_fields_are_rejected_at_every_nested_object_boundary() {
    for path in [
        "",
        "/node",
        "/node/root",
        "/node/root/identity",
        "/node/root/identity/origin",
        "/node/identity",
        "/node/identity/origin",
        "/observations/0",
        "/observations/0/checkpoint",
        "/observations/0/checkpoint/identity",
        "/observations/0/checkpoint/identity/origin",
        "/status",
    ] {
        let mut value = sample();
        value
            .pointer_mut(path)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("writable".into(), true.into());
        reject(value);
    }
}

#[test]
fn tagged_unit_statuses_cannot_hide_online_or_write_fields() {
    for kind in [
        "identity_mismatch",
        "unpinned_node",
        "satisfies_pinned_checkpoints",
    ] {
        let mut value = sample();
        value["observations"][0]["status"] = json!({"kind":kind,"online":true});
        reject(value);
        assert!(
            json_codec::from_value::<CandidateStatus>(json!({"kind":kind,"online":true})).is_err()
        );
    }
    assert!(
        json_codec::from_value::<BatchStatus>(json!({"kind":"not_observed","writable":true}))
            .is_err()
    );
}

#[test]
fn unique_found_online_and_authentication_states_are_not_candidate_results() {
    for kind in [
        "unique",
        "observed_unregistered",
        "not_found",
        "online",
        "unauthorized",
        "session_expired",
    ] {
        let mut value = sample();
        value["status"] = json!({"kind":kind});
        reject(value);
    }
    reject(json!({"status":401,"error":{"code":"unauthorized"}}));
}

#[test]
fn duplicate_json_keys_are_rejected_before_field_admission() {
    let text = json_codec::to_string(&sample()).unwrap();
    for malformed in [
        text.replacen(
            "\"physical_candidates\":1",
            "\"physical_candidates\":1,\"physical_candidates\":1",
            1,
        ),
        text.replacen(
            "\"kind\":\"satisfies_pinned_checkpoints\"",
            "\"kind\":\"satisfies_pinned_checkpoints\",\"kind\":\"satisfies_pinned_checkpoints\"",
            1,
        ),
    ] {
        assert_ne!(malformed, text);
        assert!(matches!(
            decode_observation(malformed.as_bytes()),
            Err(DecodeObservationError::JsonBoundary(_))
        ));
    }
}

#[test]
fn response_byte_budget_is_checked_before_json_parsing() {
    assert!(matches!(
        decode_observation(&vec![b' '; MAX_OBSERVATION_BYTES + 1]),
        Err(DecodeObservationError::ResponseBudget)
    ));
}

#[test]
fn exact_byte_budget_allows_valid_json_with_trailing_whitespace() {
    let mut bytes = json_codec::to_vec(&sample()).unwrap();
    assert!(bytes.len() < MAX_OBSERVATION_BYTES);
    bytes.resize(MAX_OBSERVATION_BYTES, b' ');
    assert!(decode_observation(&bytes).is_ok());
}

#[test]
fn candidate_budget_is_enforced_by_decode_and_public_constructor() {
    let mut value = sample();
    let row = value["observations"][0].clone();
    value["observations"] = json!(vec![row.clone(); MAX_CANDIDATES]);
    let admitted = read(value.clone()).unwrap();
    assert_eq!(admitted.observations().len(), MAX_CANDIDATES);
    value["observations"].as_array_mut().unwrap().push(row);
    reject(value);
    assert!(matches!(
        IndependentSourceObservation::new(
            admitted.node().clone(),
            vec![admitted.observations()[0].clone(); MAX_CANDIDATES + 1],
            1,
            admitted.status()
        ),
        Err(InvalidObservation::CandidateBudget)
    ));
}

#[test]
fn display_pin_reference_budget_does_not_silently_truncate_a_response() {
    let mut value = sample();
    let pins = (1..=400).map(hash).collect::<Vec<_>>();
    value["node"]["pins"] = json!(pins);
    let row = json!({"checkpoint":checkpoint("primary","primary",None,"0"),
        "status":{"kind":"missing_pinned_checkpoints","missing":pins}});
    value["observations"] = json!(vec![row.clone(); 24]);
    value["physical_candidates"] = 24.into();
    value["status"]["matching_candidates"] = 0.into();
    assert!(
        read(value.clone()).is_ok(),
        "400 + 24 * 400 is exactly 10,000"
    );
    value["observations"].as_array_mut().unwrap().push(row);
    value["physical_candidates"] = 25.into();
    reject(value);
}

#[test]
fn missing_pin_sets_must_be_nonempty_canonical_subsets_without_the_current_head() {
    for missing in [
        json!([]),
        json!([hash(2), hash(1)]),
        json!([hash(1), hash(1)]),
        json!([hash(99)]),
        json!([hash(3)]),
    ] {
        let mut value = sample();
        value["observations"][0]["status"] =
            json!({"kind":"missing_pinned_checkpoints","missing":missing});
        value["status"]["matching_candidates"] = 0.into();
        reject(value);
    }
    let mut value = sample();
    value["observations"][0]["checkpoint"]["publication"] = hash(2).into();
    value["observations"][0]["status"] =
        json!({"kind":"missing_pinned_checkpoints","missing":[hash(2)]});
    value["status"]["matching_candidates"] = 0.into();
    reject(value);
}

#[test]
fn sequence_must_cover_the_number_of_claimed_present_pins() {
    let mut value = sample();
    value["node"]["pins"] = json!([hash(1), hash(2), hash(3)]);
    value["observations"][0]["checkpoint"]["publication"] = hash(1).into();
    value["observations"][0]["checkpoint"]["sequence"] = "1".into();
    value["observations"][0]["status"] =
        json!({"kind":"missing_pinned_checkpoints","missing":[hash(3)]});
    value["status"]["matching_candidates"] = 0.into();
    reject(value.clone());
    value["observations"][0]["checkpoint"]["sequence"] = "2".into();
    assert!(read(value).is_ok());
}

#[test]
fn different_row_facts_cannot_be_claimed_as_one_physical_alias_group() {
    let mut value = sample();
    let mut other = value["observations"][0].clone();
    other["checkpoint"]["identity"]["origin"]["pool_id"] = "another_pool".into();
    other["status"] = json!({"kind":"identity_mismatch"});
    value["observations"].as_array_mut().unwrap().push(other);
    reject(value.clone());
    value["physical_candidates"] = 2.into();
    assert!(read(value).is_ok());

    let mut value = sample();
    let mut other = value["observations"][0].clone();
    other["checkpoint"]["publication"] = hash(4).into();
    other["checkpoint"]["sequence"] = "4".into();
    value["observations"].as_array_mut().unwrap().push(other);
    reject(value.clone());
    value["physical_candidates"] = 2.into();
    value["status"] = json!({"kind":"ambiguous","matching_candidates":2});
    assert!(read(value).is_ok());
}

#[test]
fn same_disk_token_is_rejected_only_for_adjacent_source_roles() {
    let mut value = sample();
    value["node"]["identity"]["origin"]["disk_id"] = "tertiary".into();
    value["observations"] = json!([]);
    value["physical_candidates"] = 0.into();
    value["status"] = json!({"kind":"not_observed"});
    assert!(
        read(value.clone()).is_ok(),
        "an indirect primary must not inherit an adjacent-edge rule"
    );
    value["node"]["identity"]["role"] = "secondary".into();
    reject(value.clone());
    value["node"]["identity"]["role"] = "primary".into();
    value["node"]["root"]["identity"]["role"] = "secondary".into();
    reject(value);
}

#[test]
fn legacy_preview_and_capability_payloads_are_not_independent_observations() {
    reject(
        json!({"source":{"path":"/old"},"identity":{"pool_id":"pool_legacy","disk_id":"disk_legacy","slot_id":"P01","binding_generation":"1"},
        "checkpoint":{"candidate_hash":hash(1),"publication_hash":hash(2),"sequence":"1","authority_hash":null,"unpublished_tail":false},"execution":"unavailable"}),
    );
    let output = roundtrip(sample());
    let keys = output
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect::<std::collections::BTreeSet<_>>();
    assert_eq!(
        keys,
        [
            "protocol",
            "node",
            "observations",
            "physical_candidates",
            "status"
        ]
        .into_iter()
        .collect()
    );
    // All fixture digests are symbolic. Structural admission above has not read
    // a disk, authenticated an HTTP session or proved any physical grouping.
}
