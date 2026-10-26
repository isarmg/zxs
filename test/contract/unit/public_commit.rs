use super::*;

const BODY: &str = r#"{"branch_id":"branch","expected_head":"head","expected_topology_generation":"0007","fencing_token":"18446744073709551615","file_placement":{"entry":{"primary_slot_id":"P01","target_copies":2}},"message":"目录 😀","operation_id":"op_00000000000000000000000000000000","repository_id":"repo","request_digest":"claimed","requested_protection":"strict","sealed_manifest_hash":"manifest","workspace_id":"workspace"}"#;

#[test]
fn parsed_public_envelope_keeps_exact_canonical_evidence_and_full_width_fence() {
    let body: Value = json_codec::from_str(BODY).unwrap();
    let envelope = PublicCommitEnvelope::parse("workspace", body.clone()).unwrap();
    assert_eq!(envelope.fencing_token(), u64::MAX);
    assert_eq!(
        envelope.digest(),
        "a7f3db98ee95b3ad298d9410b821050dd7f183d1c0dd8a2a22d3cba5ae8b6ecb"
    );
    assert_eq!(envelope.expected_topology_generation(), "0007");
    assert_eq!(envelope.claimed_digest(), "claimed");
    // Topology spelling is a domain precondition, not silently normalized by decoding.
    assert_eq!(
        envelope.digest(),
        crate::request_digest(
            "POST",
            "/api/workspaces/{workspace}/commit",
            json!({"workspace":"workspace"}),
            json!({}),
            &body
        )
        .unwrap()
    );
    assert_eq!(
        crate::canonical_json(&envelope.into_body()).unwrap(),
        BODY.as_bytes()
    );
}

#[test]
fn public_envelope_boundary_rejects_shapes_but_retains_semantic_preconditions() {
    let body: Value = json_codec::from_str(BODY).unwrap();
    for (field, value, code) in [
        ("operation_id", json!(""), ErrorCode::InvalidRequest),
        (
            "operation_id",
            json!("op_0000000000000000000000000000000A"),
            ErrorCode::InvalidId,
        ),
        ("fencing_token", json!(1), ErrorCode::InvalidRequest),
        ("fencing_token", json!("01"), ErrorCode::InvalidRequest),
        (
            "fencing_token",
            json!("18446744073709551616"),
            ErrorCode::InvalidRequest,
        ),
        (
            "file_placement",
            json!({"entry":{"primary_slot_id":"P01","target_copies":2,"extra":true}}),
            ErrorCode::InvalidRequest,
        ),
    ] {
        let mut invalid = body.clone();
        invalid[field] = value;
        assert_eq!(
            PublicCommitEnvelope::parse("workspace", invalid)
                .unwrap_err()
                .code(),
            code
        );
    }
    let mut semantic = body;
    semantic["requested_protection"] = "unsupported".into();
    semantic["file_placement"]["entry"]["target_copies"] = 0.into();
    let envelope = PublicCommitEnvelope::parse("workspace", semantic).unwrap();
    assert_eq!(envelope.requested_protection(), "unsupported");
    assert_eq!(envelope.file_placement()["entry"].target_copies, 0);
}

#[test]
fn typed_seal_serialization_keeps_the_frozen_public_bytes() {
    let seal = SealedWorkspace {
        operation_id: "op_00000000000000000000000000000000".parse().unwrap(),
        workspace_id: "workspace".into(),
        repository_id: "repo".into(),
        branch_id: "branch".into(),
        expected_head: "head".into(),
        fencing_token: u64::MAX,
        message: "目录 😀".into(),
        requested_protection: "strict",
        file_placement: BTreeMap::from([(
            "entry".into(),
            FilePlacement {
                primary_slot_id: "P01".into(),
                target_copies: 2,
            },
        )]),
        expected_topology_generation: u64::MAX,
        sealed_manifest_hash: "manifest".into(),
    };
    let expected = r#"{"branch_id":"branch","expected_head":"head","expected_topology_generation":"18446744073709551615","fencing_token":"18446744073709551615","file_placement":{"entry":{"primary_slot_id":"P01","target_copies":2}},"message":"目录 😀","operation_id":"op_00000000000000000000000000000000","repository_id":"repo","requested_protection":"strict","sealed_manifest_hash":"manifest","workspace_id":"workspace"}"#;
    assert_eq!(crate::canonical_json(&seal).unwrap(), expected.as_bytes());
}
