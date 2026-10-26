//! Disk admission rejects malformed histories before split can use internal invariants.
use super::*;

fn fixture() -> (tempfile::TempDir, Engine, Repository, String, String) {
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
        .create_repository("split", "P01", 3, "admin", &new_id("op"))
        .unwrap();
    let mut commits = Vec::new();
    for bytes in [b"first".as_slice(), b"second".as_slice()] {
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "admin")
            .unwrap();
        engine
            .stage_bytes(&workspace.workspace_id, "file.txt", bytes)
            .unwrap();
        commits.push(
            engine
                .commit_workspace(CommitRequest {
                    workspace_id: workspace.workspace_id,
                    operation_id: new_id("op"),
                    principal: "admin".into(),
                    expected_head: workspace.base_head,
                    fencing_token: workspace.fencing_token,
                    message: "fixture".into(),
                    request_digest: None,
                })
                .unwrap()
                .commit_id
                .unwrap(),
        );
    }
    (
        temporary,
        engine,
        repo,
        commits[0].clone(),
        commits[1].clone(),
    )
}

#[test]
fn snapshot_admission_rejects_broken_split_identities() {
    for corruption in [
        "missing_prefix",
        "foreign_prefix",
        "cyclic_prefix",
        "cut_identity",
        "descendant_identity",
        "version_identity",
        "version_file",
        "reused_version_file",
        "branch_identity",
    ] {
        let (_temporary, mut engine, repo, cut, last) = fixture();
        let plan = engine
            .create_split_plan(&repo.history_tree_id, &cut, "admin")
            .unwrap();
        let prefix = engine.state.commits[&cut].parent_commit_ids[0].clone();
        match corruption {
            "missing_prefix" => {
                std::sync::Arc::make_mut(&mut engine.state)
                    .commits
                    .remove(&prefix);
            }
            "foreign_prefix" => {
                std::sync::Arc::make_mut(&mut engine.state)
                    .commits
                    .get_mut(&prefix)
                    .unwrap()
                    .history_tree_id = "foreign".into();
            }
            "cyclic_prefix" => {
                std::sync::Arc::make_mut(&mut engine.state)
                    .commits
                    .get_mut(&prefix)
                    .unwrap()
                    .parent_commit_ids = vec![cut.clone()];
            }
            "cut_identity" => {
                std::sync::Arc::make_mut(&mut engine.state)
                    .commits
                    .get_mut(&cut)
                    .unwrap()
                    .commit_id = "missing".into();
            }
            "descendant_identity" => {
                std::sync::Arc::make_mut(&mut engine.state)
                    .commits
                    .get_mut(&last)
                    .unwrap()
                    .commit_id = "missing".into();
            }
            "version_identity" | "version_file" => {
                let version = engine.state.commits[&cut]
                    .entries
                    .iter()
                    .find(|entry| entry.kind == NamespaceEntryKind::File)
                    .unwrap()
                    .version_id
                    .clone();
                let version = std::sync::Arc::make_mut(&mut engine.state)
                    .versions
                    .get_mut(&version)
                    .unwrap();
                if corruption == "version_identity" {
                    version.version_id = "missing".into();
                } else {
                    version.file_id = "missing".into();
                }
            }
            "reused_version_file" => {
                let version = engine.state.commits[&cut]
                    .entries
                    .iter()
                    .find(|entry| entry.kind == NamespaceEntryKind::File)
                    .unwrap()
                    .version_id
                    .clone();
                let entry = std::sync::Arc::make_mut(&mut engine.state)
                    .commits
                    .get_mut(&last)
                    .unwrap()
                    .entries
                    .iter_mut()
                    .find(|entry| entry.kind == NamespaceEntryKind::File)
                    .unwrap();
                entry.version_id = version;
                entry.file_id = "missing".into();
            }
            "branch_identity" => {
                std::sync::Arc::make_mut(&mut engine.state)
                    .branches
                    .get_mut(&repo.main_branch_id)
                    .unwrap()
                    .branch_id = "missing".into();
            }
            _ => unreachable!(),
        }
        let before = canonical_json(engine.state()).unwrap();
        let raw: crate::history_package::RawState = decode_strict(&before).unwrap();
        let error = State::try_from(raw).unwrap_err();
        assert_eq!(
            error.code(),
            storage_contract::ErrorCode::StateInvalid,
            "{corruption}"
        );
        assert_eq!(
            canonical_json(engine.state()).unwrap(),
            before,
            "{corruption}"
        );
        assert_eq!(engine.plan(&plan.plan_id).unwrap().state.as_str(), "ready");
    }
}

#[test]
fn snapshot_admission_rejects_broken_crossing_merge_endpoints() {
    for corruption in [
        "prefix_branch",
        "prefix_commit",
        "moved_branch",
        "foreign_branch",
        "foreign_commit",
        "link_identity",
    ] {
        let (_temporary, mut engine, repo, cut, _) = fixture();
        let prefix = engine.state.commits[&cut].parent_commit_ids[0].clone();
        let outside = engine
            .create_branch(&prefix, "prefix", "admin", &new_id("op"))
            .unwrap();
        let _plan = engine
            .create_split_plan(&repo.history_tree_id, &cut, "admin")
            .unwrap();
        let mut link = MergeLink {
            merge_link_id: new_id("link"),
            history_tree_id: repo.history_tree_id.clone(),
            source_branch_id: repo.main_branch_id.clone(),
            source_commit_id: cut,
            target_branch_id: outside.branch_id.clone(),
            target_commit_id: prefix.clone(),
            author: "admin".into(),
            operation_id: new_id("op"),
        };
        match corruption {
            "prefix_branch" => link.target_branch_id = "missing".into(),
            "prefix_commit" => link.target_commit_id = "missing".into(),
            "moved_branch" => link.source_branch_id = "missing".into(),
            "foreign_branch" => {
                std::sync::Arc::make_mut(&mut engine.state)
                    .branches
                    .get_mut(&outside.branch_id)
                    .unwrap()
                    .history_tree_id = "foreign".into();
            }
            "foreign_commit" => {
                // Keep the real prefix intact so this specifically tests link validation.
                let mut foreign = engine.state.commits[&prefix].clone();
                foreign.commit_id = new_id("commit");
                foreign.history_tree_id = "foreign".into();
                link.target_commit_id = foreign.commit_id.clone();
                std::sync::Arc::make_mut(&mut engine.state)
                    .commits
                    .insert(foreign.commit_id.clone(), foreign);
            }
            "link_identity" => {}
            _ => unreachable!(),
        }
        let key = link.merge_link_id.clone();
        if corruption == "link_identity" {
            link.merge_link_id = "missing".into();
        }
        std::sync::Arc::make_mut(&mut engine.state)
            .merge_links
            .insert(key, link);
        let before = canonical_json(engine.state()).unwrap();
        let raw: crate::history_package::RawState = decode_strict(&before).unwrap();
        let error = State::try_from(raw).unwrap_err();
        assert_eq!(
            error.code(),
            storage_contract::ErrorCode::StateInvalid,
            "{corruption}"
        );
        assert_eq!(
            canonical_json(engine.state()).unwrap(),
            before,
            "{corruption}"
        );
    }
}

#[test]
fn split_generation_overflow_does_not_wrap_or_publish_migration() {
    for counter in ["branch", "history"] {
        let (_temporary, mut engine, repo, cut, _) = fixture();
        let plan = engine
            .create_split_plan(&repo.history_tree_id, &cut, "admin")
            .unwrap();
        if counter == "branch" {
            std::sync::Arc::make_mut(&mut engine.state)
                .branches
                .get_mut(&repo.main_branch_id)
                .unwrap()
                .generation = u64::MAX;
        } else {
            std::sync::Arc::make_mut(&mut engine.state)
                .histories
                .get_mut(&repo.history_tree_id)
                .unwrap()
                .generation = u64::MAX;
        }
        let before = canonical_json(engine.state()).unwrap();
        let error = engine
            .commit_plan(&plan.plan_id, "admin", &new_id("op"))
            .unwrap_err();
        assert_eq!(
            error.code(),
            storage_contract::ErrorCode::ResourceLimit,
            "{counter}"
        );
        assert_eq!(canonical_json(engine.state()).unwrap(), before, "{counter}");
        assert_eq!(engine.plan(&plan.plan_id).unwrap().state.as_str(), "ready");
    }
}

#[test]
fn absent_plan_is_not_found_below_an_existing_system_root() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("system");
    let engine =
        Engine::initialize(&root, Config::development([temporary.path().join("disk")])).unwrap();
    let id = new_id("plan");
    assert_eq!(
        engine.plan(&id).unwrap_err().code(),
        storage_contract::ErrorCode::PlanNotFound
    );
    private_dir(&root.join("plans")).unwrap();
    assert_eq!(
        engine.plan(&id).unwrap_err().code(),
        storage_contract::ErrorCode::PlanNotFound
    );
    assert_eq!(
        engine.plan("../escape").unwrap_err().code(),
        storage_contract::ErrorCode::InvalidId
    );
}

#[test]
fn plan_lookup_preserves_io_corruption_limits_and_link_rejections() {
    use std::os::unix::fs::symlink;
    for case in [
        "json",
        "oversized",
        "leaf-link",
        "leaf-directory",
        "leaf-fifo",
        "parent-link",
        "parent-file",
        "hard-link",
        "missing-root",
        "linked-root",
    ] {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("system");
        let engine =
            Engine::initialize(&root, Config::development([temporary.path().join("disk")]))
                .unwrap();
        let id = new_id("plan");
        let plans = root.join("plans");
        private_dir(&plans).unwrap();
        let file = plans.join(format!("{id}.json"));
        let expected = match case {
            "json" => {
                fs::write(&file, b"{").unwrap();
                storage_contract::ErrorCode::InvalidJson
            }
            "oversized" => {
                fs::File::create(&file)
                    .unwrap()
                    .set_len(MAX_METADATA as u64 + 1)
                    .unwrap();
                storage_contract::ErrorCode::ResourceLimit
            }
            "leaf-link" => {
                symlink(temporary.path().join("absent"), &file).unwrap();
                storage_contract::ErrorCode::UnsafePath
            }
            "leaf-directory" => {
                fs::create_dir(&file).unwrap();
                storage_contract::ErrorCode::UnsafePath
            }
            "leaf-fifo" => {
                rustix::fs::mkfifoat(
                    rustix::fs::CWD,
                    &file,
                    rustix::fs::Mode::RUSR | rustix::fs::Mode::WUSR,
                )
                .unwrap();
                storage_contract::ErrorCode::UnsafePath
            }
            "parent-link" => {
                fs::remove_dir(&plans).unwrap();
                symlink(temporary.path().join("absent"), &plans).unwrap();
                storage_contract::ErrorCode::UnsafePath
            }
            "parent-file" => {
                fs::remove_dir(&plans).unwrap();
                fs::write(&plans, b"not a directory").unwrap();
                storage_contract::ErrorCode::UnsafePath
            }
            "hard-link" => {
                let other = temporary.path().join("other.json");
                fs::write(&other, b"{}").unwrap();
                fs::hard_link(&other, &file).unwrap();
                storage_contract::ErrorCode::UnsafePath
            }
            "missing-root" => {
                fs::rename(&root, temporary.path().join("moved-system")).unwrap();
                storage_contract::ErrorCode::IoError
            }
            "linked-root" => {
                let moved = temporary.path().join("moved-system");
                fs::rename(&root, &moved).unwrap();
                symlink(&moved, &root).unwrap();
                storage_contract::ErrorCode::UnsafePath
            }
            _ => unreachable!(),
        };
        let error = engine.plan(&id).unwrap_err();
        if case == "linked-root" {
            fs::remove_file(&root).unwrap();
        }
        if matches!(case, "missing-root" | "linked-root") {
            fs::rename(temporary.path().join("moved-system"), &root).unwrap();
        }
        assert_eq!(error.code(), expected, "{case}: {error}");
    }
}

#[test]
fn plan_lookup_keeps_persisted_identity_and_digest_validation() {
    let temporary = tempfile::tempdir().unwrap();
    let root = temporary.path().join("system");
    let mut engine =
        Engine::initialize(&root, Config::development([temporary.path().join("disk")])).unwrap();
    let expected = engine
        .save_plan(
            "split",
            "admin",
            json!({"history_tree_id":new_id("history")}),
        )
        .unwrap();
    std::sync::Arc::make_mut(&mut engine.state)
        .plans
        .remove(&expected.plan_id);
    assert_eq!(
        canonical_json(&engine.plan(&expected.plan_id).unwrap()).unwrap(),
        canonical_json(&expected).unwrap()
    );
    let path = root
        .join("plans")
        .join(format!("{}.json", expected.plan_id));
    for field in ["plan_id", "digest"] {
        let mut malformed = json_codec::to_value(&expected).unwrap();
        malformed[field] = "incorrect".into();
        write_json(&path, &malformed, true).unwrap();
        assert_eq!(
            engine.plan(&expected.plan_id).unwrap_err().code(),
            storage_contract::ErrorCode::PlanInvalid,
            "{field}"
        );
    }
}
