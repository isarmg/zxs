//! Real registered root, genuine FILE branch, Workspace planner and original
//! decision. No handmade positive origin, fabricated FD or synthetic Candidate.
use super::*;
use crate::{Config, Engine};
use crate::engine::MutationPlan;
use name_policy::Name;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use storage_contract::{BranchName, Mode};

const PATH: &str = "parent/report.txt";

fn operation() -> OperationId { OperationId::generate().unwrap() }

fn staged(engine: &mut Engine, branch: &str, body: &[u8]) -> (Workspace, CommitRequest) {
    let workspace = engine.create_workspace(branch, "admin").unwrap();
    engine.stage_bytes(&workspace.workspace_id, PATH, body).unwrap();
    let req = CommitRequest {
        workspace_id: workspace.workspace_id.clone(), operation_id: operation(),
        principal: "admin".into(), expected_head: workspace.base_head.clone(),
        fencing_token: workspace.fencing_token, message: "ordinary explicit Workspace".into(),
        request_digest: None,
    };
    (engine.workspace(&workspace.workspace_id).unwrap(), req)
}

fn none(result: &OperationResult) {
    assert_eq!(result.result[FIELD], json_codec::json!({"kind":"no_such_creation","source":null}));
}

fn fixture(selected: bool) -> (tempfile::TempDir, Engine, String, String) {
    let tmp = tempfile::tempdir().unwrap();
    let system = tmp.path().join("system");
    let disk = tmp.path().join("primary");
    for path in [&system, &disk] {
        fs::create_dir(path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let config = Config::independent_roots(
        Mode::Development, [(Name::parse("Primary").unwrap(), disk)],
    ).unwrap();
    let slot = config.disks[0].slot_id.clone();
    let mut engine = Engine::initialize(&system, config).unwrap();
    let repo = engine.create_disk_root(&slot, "admin", operation()).unwrap();
    let (_, req) = staged(&mut engine, &repo.main_branch_id, b"directory source");
    none(&engine.commit_workspace(req).unwrap());
    let entry = engine.namespace_tree(&repo.main_branch_id).unwrap().get(PATH).unwrap().clone();
    let branch = engine.create_file_branch(&entry.file_id, &entry.version_id,
        &BranchName::parse("explicit editor").unwrap(), "admin", operation())
        .unwrap().branch_id.unwrap();
    if selected {
        engine.select_file_branch(&entry.file_id, &branch, "admin", operation()).unwrap();
    }
    // A branch checkpoint that materializes its borrowed base is not E.
    let (_, req) = staged(&mut engine, &branch, b"branch now independently materialized");
    none(&engine.commit_workspace(req).unwrap());
    (tmp, engine, branch, entry.file_id)
}

#[test]
fn native_original_binds_real_occurrence_single_event_and_principal_after_reopen() {
    let (temporary, mut engine, branch, file) = fixture(true);
    let (workspace, req) = staged(&mut engine, &branch, b"genuine changed bytes");
    let actor: Principal = "admin".parse().unwrap();
    let original = engine.commit_workspace(req.clone()).unwrap();
    let verified = validate_retained(engine.state(), &original, &actor).unwrap().unwrap();
    assert_eq!(verified.target.file.to_string(), file);
    assert_eq!(verified.target.path.as_str(), PATH);
    assert_eq!(verified.ancestors.len(), 1);
    assert_eq!(verified.operation, req.operation_id);
    assert_eq!(verified.sequence.get(), original.sequence);
    assert_eq!(original.result[FIELD]["kind"], "manual_file");
    assert_eq!(original.result[FIELD]["source"]["command"]["kind"], "native");
    assert_eq!(original.result[FIELD]["source"]["request"]["workspace_id"], workspace.workspace_id);
    assert_eq!(original.result[FIELD]["binding"]["published_at"], original.result["timestamp"]);
    assert_eq!(engine.commit_workspace(req.clone()).unwrap(), original);
    let (_, same) = staged(&mut engine, &branch, b"genuine changed bytes");
    none(&engine.commit_workspace(same).unwrap());
    drop(engine);
    let reopened = Engine::open(&temporary.path().join("system")).unwrap();
    let receipt = reopened.operation_record(req.operation_id).unwrap().into_result();
    assert_eq!(receipt, original);
    assert_eq!(validate_retained(reopened.state(), &receipt, &actor).unwrap().unwrap().node,
        verified.node);
}

#[test]
fn complete_before_after_and_real_prepared_body_reject_one_field_substitutions() {
    let (_tmp, mut engine, branch, _) = fixture(false);
    let (_, req) = staged(&mut engine, &branch, b"real independent body");
    let plan = engine.plan_workspace_commit_inner(req, None).unwrap();
    let MutationPlan::Prepared { next, prepared, result } = plan.mutation else {
        panic!("real new Workspace plan");
    };
    let actor: Principal = "admin".parse().unwrap();
    let prior = || engine.planning_context().snapshot_receipts().map_err(prior_error);
    assert!(validate_prepared(true, engine.state(), &next, &result, &actor, &prepared, prior)
        .unwrap().is_some());
    assert!(validate_prepared(true, engine.state(), &next, &result, &actor, &prepared,
        || Err(Error::OriginalReceipt(OriginalReceiptError::Prefix))).is_err());
    assert!(validate_prepared(false, engine.state(), &next, &result, &actor, &prepared, prior).is_err());
    assert!(validate_prepared(true, engine.state(), &next, &result, &actor, &[], prior).is_err());
    let mut forged = result.clone();
    forged.result[FIELD] = json_codec::json!({"kind":"no_such_creation","source":null});
    let mut with_forged = (*next).clone();
    with_forged.operations.insert(forged.operation_id, forged.clone());
    assert!(validate_prepared(true, engine.state(), &with_forged, &forged, &actor, &[], prior).is_err());
    for which in ["omitted", "none", "operation", "base", "parent", "node",
        "actor", "wire", "generation", "protection", "timestamp", "placement"] {
        let mut altered = result.clone();
        let mut changed = (*next).clone();
        match which {
            "omitted" => { altered.result.as_object_mut().unwrap().remove(FIELD); }
            "none" => altered.result[FIELD] = json_codec::json!({"kind":"no_such_creation","source":null}),
            "operation" => altered.result[FIELD]["binding"]["operation"] = json_codec::json!(operation()),
            "base" => altered.result[FIELD]["binding"]["base_node"] = altered.result[FIELD]["binding"]["node"].clone(),
            "parent" => altered.result[FIELD]["binding"]["ancestors"] = json_codec::json!([]),
            "node" => altered.result[FIELD]["binding"]["node"] = altered.result[FIELD]["binding"]["base_node"].clone(),
            "actor" => altered.result[FIELD]["source"]["request"]["principal"] = json_codec::json!("someone-else"),
            "wire" => altered.result[FIELD]["source"]["command"] = json_codec::json!({"kind":"wire","body":{}}),
            "generation" => altered.result[FIELD]["source"]["files"][0]["generation"] = json_codec::json!("987654321"),
            "protection" => altered.result["version_protection"] = json_codec::json!([]),
            "timestamp" => altered.result[FIELD]["binding"]["published_at"] = json_codec::json!("0"),
            "placement" => {
                changed.placements.get_mut(&prepared[0].version.version_id).unwrap().target_copies = 2;
            }
            _ => unreachable!(),
        }
        changed.operations.insert(altered.operation_id, altered.clone());
        assert!(validate_prepared(true, engine.state(), &changed, &altered, &actor, &prepared, prior)
            .is_err(), "accepted {which}");
    }
}

#[test]
fn wire_uses_its_real_sealed_manifest_and_cannot_be_relabelled_native() {
    let (_temporary, mut engine, branch, _) = fixture(false);
    let (workspace, _) = staged(&mut engine, &branch, b"distinct public wire body");
    let sealed = engine.seal_workspace(&workspace.workspace_id, "admin").unwrap();
    let mut body = json_codec::to_value(&sealed).unwrap();
    body["request_digest"] = crate::wire::commit_request_digest(&workspace.workspace_id, &body).unwrap().into();
    let envelope = crate::PublicCommitEnvelope::parse(&workspace.workspace_id, body).unwrap();
    let request = CommitRequest {
        workspace_id: workspace.workspace_id, operation_id: sealed.operation_id,
        principal: "admin".into(), expected_head: sealed.expected_head,
        fencing_token: sealed.fencing_token, message: sealed.message, request_digest: None,
    };
    let actor: Principal = "admin".parse().unwrap();
    let detached = engine.prepare_workspace_commit(request, Some(envelope)).unwrap().execute().unwrap();
    let original = engine.finish_workspace_commit(detached).unwrap();
    assert!(validate_retained(engine.state(), &original, &actor).unwrap().is_some());
    assert_eq!(original.result[FIELD]["source"]["command"]["kind"], "wire");
    let mut altered = original.clone();
    altered.result[FIELD]["source"]["command"] = json_codec::json!({"kind":"native"});
    assert!(validate_retained(engine.state(), &altered, &actor).is_err());
    assert!(validate_retained(engine.state(), &original, &"stranger".parse().unwrap()).is_err());
}


#[test]
fn actual_escaped_envelope_counts_the_last_byte_including_nested_wire_fields() {
    let value = json_codec::json!({"source":{"message":"編輯\n\"\\","wire":{"path":"母/報告"}}});
    let exact = storage_contract::canonical_json_value(&value).unwrap().len();
    assert!(bounded(&value, exact).is_ok());
    assert_eq!(bounded(&value, exact - 1).unwrap_err().code(), storage_contract::ErrorCode::ResourceLimit);
}
