//! Published maintenance retries must resume cleanup, not repeat publication.
use super::*;
use std::{io::Read, path::PathBuf};

fn save(engine: &mut Engine, branch: &str, bytes: &[u8]) -> String {
    let workspace = engine.create_workspace(branch, "admin").unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "file.txt", bytes)
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "cleanup fixture".into(),
            request_digest: None,
        })
        .unwrap()
        .commit_id
        .unwrap()
}

fn fixture() -> (
    tempfile::TempDir,
    Engine,
    Repository,
    String,
    String,
    String,
) {
    let temporary = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temporary.path().join("system"),
        Config::development([
            temporary.path().join("p"),
            temporary.path().join("b"),
            temporary.path().join("c"),
        ]),
    )
    .unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("cleanup").unwrap(),
            "P01",
            3,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let cut = save(&mut engine, &repo.main_branch_id, b"old body");
    let first = engine.list_tree(&repo.main_branch_id).unwrap()[0]
        .version_id
        .clone();
    save(&mut engine, &repo.main_branch_id, b"latest body");
    let last = engine.list_tree(&repo.main_branch_id).unwrap()[0]
        .version_id
        .clone();
    (temporary, engine, repo, cut, first, last)
}

fn blocked_cleanup(engine: &Engine, version: &str) -> (PathBuf, Vec<PathBuf>) {
    let body = &engine.state.versions[version];
    let directories = engine.state.placements[version]
        .route_slot_ids
        .iter()
        .map(|slot| version_dir(engine.disk(slot).unwrap(), &body.history_tree_id, version))
        .collect::<Vec<_>>();
    let payloads = directories
        .iter()
        .map(|directory| crate::container_storage::content_path(directory, body).unwrap())
        .collect();
    let blocker = directories[0].join("purged.json");
    // A failed local tombstone write occurs after logical publication.
    fs::create_dir(&blocker).unwrap();
    (blocker, payloads)
}

#[test]
fn published_maintenance_retry_finishes_cleanup_without_republishing() {
    for kind in ["purge", "split"] {
        let (_temporary, mut engine, repo, cut, first, last) = fixture();
        let plan = if kind == "purge" {
            engine
                .create_purge_plan(&repo.history_tree_id, vec![first.clone()], "admin")
                .unwrap()
        } else {
            engine
                .create_split_plan(&repo.history_tree_id, &cut, "admin")
                .unwrap()
        };
        let (blocker, payloads) = blocked_cleanup(&engine, &first);
        let operation = storage_contract::OperationId::generate();
        assert_eq!(
            engine
                .commit_plan(&plan.plan_id, "admin", operation)
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::UnsafePath,
            "{kind}"
        );
        let receipt = engine.operation(operation).unwrap();
        assert_eq!(
            receipt.commit_state,
            storage_contract::CommitState::Published
        );
        assert_eq!(
            engine.plan(&plan.plan_id).unwrap().state.as_str(),
            "committed"
        );
        assert!(payloads.iter().all(|path| path.exists()));
        let published = canonical_json(engine.state()).unwrap();

        // A receipt collision must never gain permission to run cleanup.
        for (id, actor) in [(new_id("plan"), "admin"), (plan.plan_id.clone(), "other")] {
            assert_eq!(
                engine
                    .commit_plan(&id, actor, operation)
                    .unwrap_err()
                    .code(),
                storage_contract::ErrorCode::IdempotencyKeyReused
            );
        }
        assert_eq!(
            engine
                .commit_plan(&plan.plan_id, "admin", operation)
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::UnsafePath,
            "a retry must not report cleanup success while the fault remains: {kind}"
        );
        fs::remove_dir(&blocker).unwrap();
        let retried = engine
            .commit_plan(&plan.plan_id, "admin", operation)
            .unwrap();
        assert_eq!(retried.result, receipt.result);
        assert_eq!(retried.request_digest, receipt.request_digest);
        assert_eq!(canonical_json(engine.state()).unwrap(), published);
        assert!(payloads.iter().all(|path| !path.exists()), "{kind}");
        assert!(blocker.is_file(), "cleanup writes the proven tombstone");
        let retained = if kind == "purge" {
            last.as_str()
        } else {
            receipt.result["version_mapping"][&last].as_str().unwrap()
        };
        let mut content = engine.open_content(retained).unwrap();
        let mut bytes = Vec::new();
        content.file.read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"latest body");
        assert_eq!(
            engine
                .commit_plan(&plan.plan_id, "admin", operation)
                .unwrap()
                .result,
            receipt.result
        );
    }
}

#[test]
fn purge_retry_preserves_active_workspace_and_respects_detached_preparation() {
    let (_temporary, mut engine, repo, _cut, first, _last) = fixture();
    let plan = engine
        .create_purge_plan(&repo.history_tree_id, vec![first.clone()], "admin")
        .unwrap();
    let (blocker, payloads) = blocked_cleanup(&engine, &first);
    let operation = storage_contract::OperationId::generate();
    assert!(
        engine
            .commit_plan(&plan.plan_id, "admin", operation)
            .is_err()
    );
    fs::remove_dir(&blocker).unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "admin")
        .unwrap();
    let staged = engine
        .stage_bytes(&workspace.workspace_id, "file.txt", b"unsaved body")
        .unwrap();
    let before = canonical_json(&engine.workspace(&workspace.workspace_id).unwrap()).unwrap();
    let preparation = engine
        .prepare_workspace_commit(
            CommitRequest {
                workspace_id: workspace.workspace_id.clone(),
                operation_id: storage_contract::OperationId::generate(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "unrelated pending write".into(),
                request_digest: None,
            },
            None,
        )
        .unwrap();
    assert_eq!(
        engine
            .commit_plan(&plan.plan_id, "admin", operation)
            .unwrap_err()
            .code(),
        storage_contract::ErrorCode::CommitPreparationBusy
    );
    assert!(payloads.iter().all(|path| path.exists()));
    drop(preparation);
    engine
        .commit_plan(&plan.plan_id, "admin", operation)
        .unwrap();
    assert!(payloads.iter().all(|path| !path.exists()));
    assert_eq!(
        canonical_json(&engine.workspace(&workspace.workspace_id).unwrap()).unwrap(),
        before
    );
    assert_eq!(fs::read(&staged.source_path).unwrap(), b"unsaved body");
}
