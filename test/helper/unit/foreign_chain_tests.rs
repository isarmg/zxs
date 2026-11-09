use super::*;
use json_codec::json;

fn lower(level: u32, slot: &str, upstream: Option<&str>) -> LowerDirectory {
    LowerDirectory {
        root_path: format!("/zxs/data/disks/{slot}"), slot_id: slot.into(), level,
        upstream_slot_id: upstream.map(str::to_owned), metadata_member: true,
    }
}
#[test]
fn tertiary_requires_both_real_lower_levels_in_one_complete_ordered_chain() {
    let first = lower(1, "primary", None);
    let second = lower(2, "secondary", Some("primary"));
    assert!(complete_chain(3, "tertiary", "secondary", &[&first, &second]).is_ok());
    assert!(complete_chain(3, "tertiary", "primary", &[&first]).is_err());
    assert!(complete_chain(3, "tertiary", "secondary", &[&second]).is_err());
    assert!(complete_chain(3, "tertiary", "secondary", &[&second, &first]).is_err());
    assert!(complete_chain(2, "secondary", "primary", &[&first]).is_ok());
    assert!(complete_chain(1, "source", "primary", &[]).is_err());
    assert!(complete_chain(4, "source", "secondary", &[&first, &second]).is_err());
}
#[test]
fn chain_links_and_slots_cannot_skip_collide_or_use_placeholders() {
    let first = lower(1, "primary", None);
    let mut second = lower(2, "secondary", None);
    assert!(complete_chain(3, "tertiary", "secondary", &[&first, &second]).is_err());
    second.upstream_slot_id = Some("later-primary".into());
    assert!(complete_chain(3, "tertiary", "secondary", &[&first, &second]).is_err());
    second.upstream_slot_id = Some("primary".into());
    assert!(complete_chain(3, "PRIMARY", "secondary", &[&first, &second]).is_err());
    assert!(complete_chain(3, "../source", "secondary", &[&first, &second]).is_err());
    assert!(complete_chain(3, "tertiary", "unselected", &[&first, &second]).is_err());
    assert!(layout::names(&[first.root_path.clone(), first.root_path.clone()]).is_err());
}
#[test]
fn engine_slot_policy_rejects_invalid_source_and_lower_names() {
    let first = lower(1, "primary", None);
    for invalid in ["", "../source", "has space", "source/child", "源"] {
        assert!(complete_chain(2, invalid, "primary", &[&first]).is_err());
        let invalid_first = lower(1, invalid, None);
        assert!(complete_chain(2, "secondary", invalid, &[&invalid_first]).is_err());
    }
}
#[test]
fn foreign_input_requires_explicit_complete_fields_and_rejects_authority_overrides() {
    let mut input = json!({"service_user":"zxs","operation_id":"op_11111111111111111111111111111111",
        "source_root":"/zxs/data/disks/foreign","source_slot_id":"secondary",
        "source_upstream_slot_id":"primary","source_metadata_member":true,
        "lower":[{"root_path":"/zxs/data/disks/primary","slot_id":"primary","level":1,
            "upstream_slot_id":null,"metadata_member":true}],"external_writers_stopped":false});
    assert!(json_codec::from_value::<Input>(input.clone()).is_ok());
    assert!(preview(json_codec::from_value(input.clone()).unwrap()).unwrap_err()
        .to_string().contains("stop ZXS"));
    input["admit_before_lower_ready"] = true.into();
    assert!(json_codec::from_value::<Input>(input).is_err());
    assert!(request_digest("../original").is_err());
    assert!(request_digest(&"A".repeat(64)).is_err());
}
#[test]
fn only_all_exact_published_capsules_can_form_material_completion() {
    assert!(!Report { targets: vec![] }.all_published());
    let target = |level, progress| TargetProgress { level, path: format!("/selected/{level}"),
        capsule_digest: "a".repeat(64), progress };
    let mut report = Report { targets: vec![target(1, Progress::PublishedCapsule), target(2, Progress::Reserved)] };
    assert!(!report.all_published());
    report.targets[1].progress = Progress::PublishedCapsule;
    assert!(report.all_published());
    report.targets[0].progress = Progress::Unstarted;
    assert!(!report.all_published());
}
#[test]
fn original_selector_never_bypasses_stopped_acknowledgment() {
    let input = Original { service_user: "zxs".into(),
        operation_id: "op_11111111111111111111111111111111".parse().unwrap(),
        request_digest: "a".repeat(64), external_writers_stopped: false };
    assert!(original(input).unwrap_err().to_string().contains("stop ZXS"));
}
