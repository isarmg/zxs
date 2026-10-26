//! Authenticated disk bytes are raw until complete reference admission succeeds.
use super::*;
use crate::journal::{load_snapshot, store_snapshot};

#[derive(Clone, Copy, Debug)]
enum Corruption {
    RepositoryKey,
    HistoryKey,
    MissingRepository,
    MissingHistory,
    MissingMain,
    MissingHead,
    MissingParent,
    Cycle,
    MultipleParents,
    OrphanCommit,
    OrphanContentState,
    MissingPlacement,
    OrphanFileSelection,
    OrphanFileNote,
    OrphanDirectorySelection,
    OrphanContext,
    OrphanVersionNote,
}

#[test]
fn stored_snapshots_reject_broken_reference_closure_before_engine_admission() {
    use Corruption::*;
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 3);
    save(&mut engine, &repo.main_branch_id, "file.txt", b"body");
    let original = engine.state.as_ref().clone();
    let branch = &original.branches[&repo.main_branch_id];
    let head = branch.head_commit_id.clone();
    let root = original.histories[&repo.history_tree_id]
        .root_commit_id
        .clone();
    let version = original.commits[&head]
        .entries
        .iter()
        .find(|entry| entry.kind == NamespaceEntryKind::File)
        .unwrap()
        .version_id
        .clone();
    for corruption in [
        RepositoryKey,
        HistoryKey,
        MissingRepository,
        MissingHistory,
        MissingMain,
        MissingHead,
        MissingParent,
        Cycle,
        MultipleParents,
        OrphanCommit,
        OrphanContentState,
        MissingPlacement,
        OrphanFileSelection,
        OrphanFileNote,
        OrphanDirectorySelection,
        OrphanContext,
        OrphanVersionNote,
    ] {
        let mut state = original.clone();
        match corruption {
            RepositoryKey => {
                state
                    .repositories
                    .get_mut(&repo.repository_id)
                    .unwrap()
                    .repository_id = new_id("repo")
            }
            HistoryKey => {
                state
                    .histories
                    .get_mut(&repo.history_tree_id)
                    .unwrap()
                    .history_tree_id = new_id("history")
            }
            MissingRepository => {
                state.repositories.remove(&repo.repository_id);
            }
            MissingHistory => {
                state.histories.remove(&repo.history_tree_id);
            }
            MissingMain => {
                state
                    .repositories
                    .get_mut(&repo.repository_id)
                    .unwrap()
                    .main_branch_id = new_id("br")
            }
            MissingHead => {
                state
                    .branches
                    .get_mut(&repo.main_branch_id)
                    .unwrap()
                    .head_commit_id = new_id("commit")
            }
            MissingParent => {
                state.commits.get_mut(&head).unwrap().parent_commit_ids = vec![new_id("commit")]
            }
            Cycle => state.commits.get_mut(&head).unwrap().parent_commit_ids = vec![head.clone()],
            MultipleParents => {
                state.commits.get_mut(&head).unwrap().parent_commit_ids =
                    vec![root.clone(), root.clone()]
            }
            OrphanCommit => {
                let mut commit = state.commits[&head].clone();
                commit.commit_id = new_id("commit");
                commit.history_tree_id = new_id("history");
                state.commits.insert(commit.commit_id.clone(), commit);
            }
            OrphanContentState => {
                state
                    .content_states
                    .insert(new_id("ver"), ContentState::Available);
            }
            MissingPlacement => {
                state.placements.remove(&version);
            }
            OrphanFileSelection => {
                state.file_selections.insert(new_id("file"), new_id("br"));
            }
            OrphanFileNote => {
                state.file_notes.insert(new_id("file"), "note".into());
            }
            OrphanDirectorySelection => {
                state
                    .directory_selections
                    .insert(new_id("entry"), new_id("dbr"));
            }
            OrphanContext => {
                let context = FileContext {
                    node_id: None,
                    selection_generation: 0,
                    branch_id: repo.main_branch_id.clone(),
                    inherited_from: None,
                };
                state
                    .file_contexts
                    .insert(format!("{}/file.txt", new_id("repo")), context);
            }
            OrphanVersionNote => {
                state.version_notes.insert(new_id("ver"), "note".into());
            }
        }
        // The malformed graph has valid object hashes: this exercises reference
        // admission rather than merely rejecting a changed page checksum.
        let hash = store_snapshot(&fixture.config.disks, &state).unwrap();
        let error = load_snapshot(&fixture.config.disks[0], &hash).unwrap_err();
        assert_eq!(
            error.code(),
            ErrorCode::StateInvalid,
            "{corruption:?}: {error}"
        );
        assert!(
            crate::publication_evidence::verified_snapshot(&fixture.config.disks, &hash).is_err(),
            "{corruption:?}"
        );
    }
    let mut sparse = original.clone();
    sparse.content_states.remove(&version);
    let hash = store_snapshot(&fixture.config.disks, &sparse).unwrap();
    let admitted = load_snapshot(&fixture.config.disks[0], &hash).unwrap();
    assert_eq!(
        admitted, sparse,
        "Available is an intentional sparse default, not a missing reference"
    );
    assert!(!admitted.content_states.contains_key(&version));
    assert_eq!(store_snapshot(&[], &admitted).unwrap(), hash);
    drop(engine);
    let reopened = Engine::open(&fixture.root).unwrap();
    assert_eq!(reopened.state.as_ref(), &original);
}

#[test]
fn merge_ancestry_still_rejects_user_supplied_foreign_or_missing_endpoints() {
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let repo = repository(&mut engine, 3);
    let head = engine.state.branches[&repo.main_branch_id]
        .head_commit_id
        .clone();
    let error = engine
        .add_merge_link(
            &repo.main_branch_id,
            &new_id("commit"),
            &repo.main_branch_id,
            &head,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::CommitNotFound);
    let error = engine
        .add_merge_link(
            &new_id("br"),
            &head,
            &repo.main_branch_id,
            &head,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap_err();
    assert_eq!(error.code(), ErrorCode::BranchNotFound);
}

#[test]
fn stored_snapshot_rejects_file_identity_reused_across_histories() {
    let fixture = Fixture::new();
    let mut engine = fixture.initialize();
    let first = repository(&mut engine, 3);
    save(&mut engine, &first.main_branch_id, "first.txt", b"first");
    let second = engine
        .create_repository(
            "second",
            "P01",
            3,
            "admin",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    save(&mut engine, &second.main_branch_id, "second.txt", b"second");
    let mut state = engine.state.as_ref().clone();
    let first_file = state
        .versions
        .values()
        .find(|version| version.history_tree_id == first.history_tree_id)
        .unwrap()
        .file_id
        .clone();
    let second_version = state
        .versions
        .values()
        .find(|version| version.history_tree_id == second.history_tree_id)
        .unwrap()
        .version_id
        .clone();
    state.versions.get_mut(&second_version).unwrap().file_id = first_file;
    let hash = store_snapshot(&fixture.config.disks, &state).unwrap();
    let error = load_snapshot(&fixture.config.disks[0], &hash).unwrap_err();
    assert_eq!(error.code(), ErrorCode::StateInvalid);
    assert_eq!(
        error.message(),
        "snapshot stable file identity crosses histories"
    );
}
