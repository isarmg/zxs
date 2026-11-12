//! Source-only regressions for the deterministic joint recipe. These do not
//! assert physical enrollment, three-scope installation or usable admission.
use super::*;
use std::fs;

struct Fixture {
    _temp: tempfile::TempDir,
    source: Engine,
    current: Engine,
    target: Config,
    recipe: ForeignAdmissionRecipe,
}
fn save(engine: &mut Engine, name: &str, copies: u32) {
    let repository = engine
        .create_repository(
            &RepositoryName::parse(name).unwrap(),
            "P01",
            copies,
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "document.txt", name.as_bytes())
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: String::new(),
            request_digest: None,
        })
        .unwrap();
}
fn fixture() -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let original = Config::development([
        temp.path().join("source-primary"),
        temp.path().join("source-backup"),
    ])
    .unwrap();
    let mut source = Engine::initialize(&temp.path().join("source-system"), original).unwrap();
    save(&mut source, "source-one", 2);
    save(&mut source, "source-two", 2);
    let mut current = Engine::initialize(
        &temp.path().join("current-system"),
        Config::development([temp.path().join("current-disk")]).unwrap(),
    )
    .unwrap();
    save(&mut current, "existing", 1);
    let mut added = Config::development([
        temp.path().join("new-primary"),
        source.config().disks[1].root.clone(),
    ])
    .unwrap()
    .disks;
    added[0].slot_id = "P02".into();
    added[1].slot_id = "B02".into();
    added[1].upstream_slot_id = Some("P02".into());
    let mut target = current.config().clone();
    target.disks.extend(added);
    let names = current
        .capture_foreign_identity_target(ForeignIdentityTargetRequest {
            proposed_configuration: target.clone(),
            operation_id: OperationId::generate().unwrap(),
            request_digest: "a".repeat(64).parse().unwrap(),
            chain_digest: "b".repeat(64).parse().unwrap(),
            registration_proposal_digest: "c".repeat(64).parse().unwrap(),
        })
        .unwrap();
    let preview = crate::foreign_import::tests::inspect(&source.config().disks[1].root).unwrap();
    let recipe = current
        .capture_foreign_admission_recipe(&names, &preview)
        .unwrap();
    Fixture {
        _temp: temp,
        source,
        current,
        target,
        recipe,
    }
}
#[test]
fn joint_recipe_round_trip_recomputes_the_same_full_state_without_importing_credentials_or_receipts()
 {
    let fixture = fixture();
    let base = fixture.current.state();
    let source = fixture.source.state();
    let fence = base.sequence + 1;
    let before = canonical_json(base).unwrap();
    let next = fixture
        .recipe
        .joint_state(base, fixture.current.config(), &fixture.target, fence)
        .unwrap();
    let decoded: ForeignAdmissionRecipe =
        decode_strict(&canonical_json(&fixture.recipe).unwrap()).unwrap();
    assert_eq!(
        next,
        decoded
            .joint_state(base, fixture.current.config(), &fixture.target, fence)
            .unwrap()
    );
    assert_eq!(next.sequence, fence + source.sequence + 1);
    assert_eq!(
        next.repositories.len(),
        base.repositories.len() + source.repositories.len()
    );
    assert_eq!(
        next.versions.len(),
        base.versions.len() + source.versions.len()
    );
    assert_eq!(next.operations, base.operations);
    assert_eq!(next.operation_principals, base.operation_principals);
    assert_eq!(next.control_auth, base.control_auth);
    assert_eq!(next.events, base.events);
    assert_eq!(canonical_json(base).unwrap(), before);
    for (id, version) in next
        .versions
        .iter()
        .filter(|(id, _)| !base.versions.contains_key(*id))
    {
        assert!(version.version_sequence > fence);
        assert!(
            source
                .versions
                .values()
                .any(|old| old.content_hash == version.content_hash
                    && old.checksum_manifest_hash == version.checksum_manifest_hash)
        );
        let placement = &next.placements[id];
        assert_eq!(placement.primary_slot_id, "P02");
        assert_eq!(placement.route_slot_ids, ["P02", "B02"]);
        assert_eq!(placement.target_copies, 2);
        assert_eq!(placement.topology_generation, base.topology_generation + 1);
    }
    for commit in next
        .commits
        .values()
        .filter(|commit| !base.commits.contains_key(&commit.commit_id))
    {
        assert!(commit.author.starts_with("foreignactor_"));
        assert!(!base.operations.contains_key(&commit.operation_id));
        assert!(!source.operations.contains_key(&commit.operation_id));
    }
}
#[test]
fn changed_current_state_or_mapping_cannot_rebind_a_cold_recipe() {
    let fixture = fixture();
    let mut base = fixture.current.state().clone();
    base.business_generation += 1;
    assert!(
        fixture
            .recipe
            .joint_state(
                &base,
                fixture.current.config(),
                &fixture.target,
                base.sequence + 1
            )
            .is_err()
    );
    let mut changed = fixture.recipe.clone();
    changed.identities.mapping_digest = "0".repeat(64);
    assert!(
        changed
            .joint_state(
                fixture.current.state(),
                fixture.current.config(),
                &fixture.target,
                base.sequence + 1
            )
            .is_err()
    );
}
#[test]
fn recipe_cannot_change_original_state_or_target_route_and_sequence_does_not_wrap() {
    let fixture = fixture();
    let mut recipe = fixture.recipe.clone();
    recipe.source.0.business_generation += 1;
    assert!(
        recipe
            .verify_contract(
                fixture.current.config(),
                &fixture.target,
                recipe.identities.baseline.operation_id
            )
            .is_err()
    );
    let mut target = fixture.target.clone();
    target.disks.last_mut().unwrap().upstream_slot_id = Some("P01".into());
    assert!(
        fixture
            .recipe
            .verify_contract(
                fixture.current.config(),
                &target,
                fixture.recipe.identities.baseline.operation_id
            )
            .is_err()
    );
    assert_eq!(
        fixture.recipe.first_sequence(u64::MAX).unwrap_err().code(),
        ErrorCode::ResourceLimit
    );
}
#[test]
fn unsupported_lifecycle_is_rejected_without_removing_its_rows() {
    let mut fixture = fixture();
    let id = fixture
        .recipe
        .source
        .0
        .versions
        .keys()
        .next()
        .unwrap()
        .clone();
    fixture
        .recipe
        .source
        .0
        .content_states
        .insert(id.clone(), ContentState::Retired);
    fixture.recipe.identities.original.state_hash =
        object_hash("foreign-original-state", &fixture.recipe.source.0).unwrap();
    assert_eq!(
        fixture.recipe.require_source().unwrap_err().code(),
        ErrorCode::UnsupportedOperation
    );
    assert_eq!(
        fixture.recipe.source.0.content_states[&id],
        ContentState::Retired
    );
}
#[test]
fn capture_does_not_replace_the_foreign_physical_seal() {
    let fixture = fixture();
    let path = fixture.source.config().disks[1].root.join("disk.json");
    let before = fs::read(&path).unwrap();
    let next = fixture
        .recipe
        .joint_state(
            fixture.current.state(),
            fixture.current.config(),
            &fixture.target,
            fixture.current.state().sequence + 1,
        )
        .unwrap();
    assert!(next.histories.len() > fixture.current.state().histories.len());
    assert_eq!(fs::read(&path).unwrap(), before);
    assert!(!fixture.target.disks[1].root.exists());
}

#[test]
fn undone_deletion_history_is_not_silently_reduced_to_only_the_live_graph() {
    use storage_contract::history_window::{HistoryFileId, HistoryRepositoryId};
    let mut fixture = fixture();
    let version = fixture
        .source
        .state()
        .versions
        .values()
        .next()
        .unwrap()
        .clone();
    let repository = fixture.source.state().histories[&version.history_tree_id]
        .repository_id
        .clone();
    let file_id = HistoryFileId::parse(&version.file_id).unwrap();
    let repository_id = HistoryRepositoryId::parse(&repository).unwrap();
    let path = name_policy::LookupPath::parse("document.txt").unwrap();
    let deleted = OperationId::generate().unwrap();
    fixture
        .source
        .soft_delete_file(DeleteFileRequest {
            target: FileDeletionTarget {
                repository_id: &repository_id,
                file_id: &file_id,
                path: &path,
            },
            principal: "admin",
            operation_id: deleted,
        })
        .unwrap();
    let node = fixture
        .source
        .file_deletion(&version.file_id)
        .unwrap()
        .node_id
        .clone();
    let undone = OperationId::generate().unwrap();
    fixture
        .source
        .undo_file_deletion(UndoFileDeletionRequest {
            file_id: &file_id,
            deletion_node_id: &node,
            principal: "admin",
            operation_id: undone,
        })
        .unwrap();
    assert!(fixture.source.state().file_deletions.is_empty());
    let before = canonical_json(fixture.source.state()).unwrap();
    let baseline = &fixture.recipe.identities.baseline;
    let names = fixture
        .current
        .capture_foreign_identity_target(ForeignIdentityTargetRequest {
            proposed_configuration: fixture.target.clone(),
            operation_id: baseline.operation_id,
            request_digest: baseline.request_digest.parse().unwrap(),
            chain_digest: baseline.chain_digest.parse().unwrap(),
            registration_proposal_digest: baseline.registration_proposal_digest.parse().unwrap(),
        })
        .unwrap();
    let preview =
        crate::foreign_import::tests::inspect(&fixture.source.config().disks[1].root).unwrap();
    assert_eq!(
        fixture
            .current
            .capture_foreign_admission_recipe(&names, &preview)
            .unwrap_err()
            .code(),
        ErrorCode::UnsupportedOperation
    );
    assert_eq!(canonical_json(fixture.source.state()).unwrap(), before);
    assert!(fixture.source.state().operations.contains_key(&deleted));
    assert!(fixture.source.state().operations.contains_key(&undone));
}

#[test]
fn real_path_facts_are_rejected_before_foreign_capture_changes_original_bytes() {
    fn bytes(
        path: &std::path::Path,
        files: &mut std::collections::BTreeMap<std::path::PathBuf, Vec<u8>>,
    ) {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                bytes(&entry.path(), files);
            } else if kind.is_file() {
                // SQLite shared-memory reader locks are not authoritative bytes.
                if !entry.file_name().to_string_lossy().ends_with("-shm") {
                    files.insert(entry.path(), fs::read(entry.path()).unwrap());
                }
            }
        }
    }
    let mut fixture = fixture();
    let repository = fixture.source.state().repositories.values().next().unwrap().clone();
    let renamed = fixture.source.file_namespace(
        &repository.repository_id,
        NamespaceOperation::parse(
            NamespaceOperationKind::Rename, "document.txt", Some("renamed.txt"),
        ).unwrap(),
        "admin",
        OperationId::generate().unwrap(),
    ).unwrap();
    assert_eq!(crate::persist::path_changes::read(&renamed).unwrap().len(), 1);
    let source_before = canonical_json(fixture.source.state()).unwrap();
    let current_before = canonical_json(fixture.current.state()).unwrap();
    let mut roots: Vec<_> = fixture.source.config().disks.iter()
        .map(|disk| disk.root.clone()).collect();
    roots.push(fixture.source.root.clone());
    roots.push(fixture.current.root.clone());
    let mut before = std::collections::BTreeMap::new();
    for root in &roots {
        bytes(root, &mut before);
    }
    let baseline = &fixture.recipe.identities.baseline;
    let names = fixture.current.capture_foreign_identity_target(ForeignIdentityTargetRequest {
        proposed_configuration: fixture.target.clone(),
        operation_id: baseline.operation_id,
        request_digest: baseline.request_digest.parse().unwrap(),
        chain_digest: baseline.chain_digest.parse().unwrap(),
        registration_proposal_digest: baseline.registration_proposal_digest.parse().unwrap(),
    }).unwrap();
    let preview =
        crate::foreign_import::tests::inspect(&fixture.source.config().disks[1].root).unwrap();
    let error = fixture.current.capture_foreign_admission_recipe(&names, &preview).unwrap_err();
    assert_eq!(error.code(), ErrorCode::UnsupportedOperation);
    assert!(matches!(error, Error::PathChange(crate::PathChangeError::ForeignSource)));
    assert_eq!(canonical_json(fixture.source.state()).unwrap(), source_before);
    assert_eq!(canonical_json(fixture.current.state()).unwrap(), current_before);
    let mut after = std::collections::BTreeMap::new();
    for root in &roots {
        bytes(root, &mut after);
    }
    assert_eq!(after, before);
    assert_eq!(fixture.source.state().operations[&renamed.operation_id], renamed);
    assert!(!fixture.target.disks[1].root.exists());
}
