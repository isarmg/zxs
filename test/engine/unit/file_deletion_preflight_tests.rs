use super::*;
use crate::{
    BranchKind, BranchState, Commit, Config, FileMetadataKind, FileMetadataNode, FilePathNode,
    FileVersion, NamespaceEntryKind, OperationId,
};
use std::collections::BTreeMap;

fn id(prefix: &str, value: u32) -> String {
    format!("{prefix}_{value:032x}")
}

fn operation() -> OperationId {
    id("op", 1).parse().unwrap()
}

fn entry(path: &str, entry_id: u32, file_id: u32, version_id: u32) -> Entry {
    Entry {
        entry_id: id("entry", entry_id),
        path: name_policy::PortablePath::parse(path).unwrap(),
        kind: NamespaceEntryKind::File,
        file_id: id("file", file_id),
        version_id: id("ver", version_id),
        size: 4,
    }
}

fn body(file: u32, version: u32) -> FileVersion {
    FileVersion {
        history_tree_id: id("hist", 1),
        version_id: id("ver", version),
        file_id: id("file", file),
        creation_primary_slot_id: "P01".into(),
        logical_size: 4,
        version_sequence: u64::from(version),
        content_hash: "a".repeat(64).parse().unwrap(),
        checksum_chunk_bytes: 262144,
        checksum_manifest_hash: "b".repeat(64),
        creation_name: "Alpha.txt".into(),
        storage_name: "v000001_20261106T000000Z_aaaaaaaa.txt".into(),
        created_at: "2026-11-06T00:00:00Z".into(),
        source_version_ids: vec![],
        created_as_full_checkpoint: true,
    }
}

struct Fixture {
    state: State,
    entries: Vec<Entry>,
    repository_id: HistoryRepositoryId,
    file_id: HistoryFileId,
    path: name_policy::LookupPath,
}
impl Fixture {
    fn new() -> Self {
        let repository_id = HistoryRepositoryId::parse(&id("repo", 1)).unwrap();
        let repository = Repository {
            repository_id: repository_id.as_str().into(),
            name: "disk-root".into(),
            history_tree_id: id("hist", 1),
            main_branch_id: id("br", 100),
            primary_slot_id: "P01".into(),
            target_copies: 1,
            name_policy: "portable".into(),
            unicode_version: "17.0.0".into(),
            smb_enabled: true,
            smb_binding_generation: 1,
            file_manager_root: true,
            directory_deletions: std::collections::BTreeMap::new(),
        };
        let mut state = State {
            sequence: 20,
            ..State::default()
        };
        state
            .repositories
            .insert(repository.repository_id.clone(), repository);
        for version in [body(1, 1), body(1, 2), body(2, 3)] {
            state.versions.insert(version.version_id.clone(), version);
        }
        state.file_path_nodes.insert(
            id("fnode", 2),
            FilePathNode {
                node_id: id("fnode", 2),
                history_tree_id: id("hist", 1),
                file_id: id("file", 1),
                version_id: id("ver", 2),
                parent_node_ids: vec![id("ver", 1)],
                branch_id: id("br", 2),
                path: "Alpha.txt".into(),
                previous_path: "Old.txt".into(),
                sequence: 3,
                created_at: 1,
                operation_id: operation(),
                author: "admin".into(),
            },
        );
        state.file_metadata_nodes.insert(
            id("fnode", 3),
            FileMetadataNode {
                node_id: id("fnode", 3),
                history_tree_id: id("hist", 1),
                file_id: id("file", 1),
                version_id: id("ver", 2),
                parent_node_ids: vec![id("fnode", 2)],
                branch_id: id("br", 3),
                kind: FileMetadataKind::Tags,
                tags: ["retained".into()].into(),
                sequence: 4,
                created_at: 1,
                operation_id: operation(),
                author: "admin".into(),
            },
        );
        for (number, status, node) in [
            (1, BranchState::Active, None),
            (2, BranchState::Archived, Some(id("fnode", 2))),
            (3, BranchState::Migrated, Some(id("fnode", 3))),
            (4, BranchState::Merged, None),
        ] {
            let version = if number == 2 || number == 3 { 2 } else { 1 };
            let commit = Commit {
                commit_id: id("commit", number),
                history_tree_id: id("hist", 1),
                kind: "file_content".into(),
                parent_commit_ids: vec![],
                tree_id: id("tree", number),
                sequence: u64::from(number),
                operation_id: operation(),
                author: "admin".into(),
                message: "retained".into(),
                entries: vec![entry("Alpha.txt", 1, 1, version)],
                namespace_deletions: None,
                pending_file_versions: BTreeMap::new(),
                provenance: None,
            };
            let branch = Branch {
                branch_id: id("br", number),
                history_tree_id: id("hist", 1),
                name: format!("retained-{number}"),
                head_commit_id: commit.commit_id.clone(),
                fork_commit_id: commit.commit_id.clone(),
                branch_kind: BranchKind::File,
                file_id: Some(id("file", 1)),
                file_node_id: node,
                pending_file_versions: if number == 3 {
                    [(id("file", 1), id("ver", 2))].into()
                } else {
                    BTreeMap::new()
                },
                generation: 7,
                state: status,
            };
            state.commits.insert(commit.commit_id.clone(), commit);
            state.branches.insert(branch.branch_id.clone(), branch);
        }
        state.file_branch_aliases.insert(id("br", 4), id("br", 1));
        let directory = Entry {
            entry_id: id("entry", 10),
            path: name_policy::PortablePath::parse("copies").unwrap(),
            kind: NamespaceEntryKind::Directory,
            file_id: String::new(),
            version_id: String::new(),
            size: 0,
        };
        let mut entries = vec![
            entry("Alpha.txt", 1, 1, 1),
            entry("B.txt", 2, 2, 3),
            directory,
            entry("copies/alpha.txt", 3, 1, 2),
        ];
        entries.sort_by(|left, right| left.lookup_key().cmp(right.lookup_key()));
        Self {
            state,
            entries,
            repository_id,
            file_id: HistoryFileId::parse(&id("file", 1)).unwrap(),
            path: name_policy::LookupPath::parse("ALPHA.txt").unwrap(),
        }
    }

    fn target(&self) -> FileDeletionTarget<'_> {
        FileDeletionTarget {
            repository_id: &self.repository_id,
            file_id: &self.file_id,
            path: &self.path,
        }
    }

    fn capture(
        &self,
        maximum: usize,
        cancel: &HistoryWindowCancellation,
    ) -> Result<FileDeletionPreflight<'_>> {
        preflight::capture(
            &self.state,
            &self.state.repositories[self.repository_id.as_str()],
            &self.entries,
            self.target(),
            maximum,
            cancel,
        )
    }

    fn initialized(&mut self, directory: String) {
        self.state.directory_git_repositories.insert(
            directory.clone(),
            storage_contract::directory_git::DirectoryGitRepository {
                repository_id: self.repository_id.as_str().into(),
                directory_id: directory,
                initialized_sequence: 1,
                main_branch_id: id("dbr", 1),
                current_branch_id: id("dbr", 1),
                branches: BTreeMap::new(),
            },
        );
    }
}

#[test]
fn retains_every_branch_state_and_deduplicates_only_the_head_edges() {
    let fixture = Fixture::new();
    let before = fixture.state.clone();
    let read = fixture
        .capture(4, &HistoryWindowCancellation::new())
        .unwrap();
    assert_eq!(read.file_id(), id("file", 1));
    assert_eq!(read.sequence(), 20);
    assert_eq!(
        read.repository().repository_id,
        fixture.repository_id.as_str()
    );
    assert_eq!(read.branches().len(), 4);
    assert_eq!(
        read.branches()
            .iter()
            .map(|head| head.branch().state)
            .collect::<Vec<_>>(),
        [
            BranchState::Active,
            BranchState::Archived,
            BranchState::Migrated,
            BranchState::Merged
        ]
    );
    for head in read.branches() {
        assert_eq!(head.branch(), &before.branches[&head.branch().branch_id]);
    }
    assert_eq!(read.branches()[3].alias_of(), Some(id("br", 1).as_str()));
    assert_eq!(read.branches()[3].node_id(), id("ver", 1));
    let mut expected = vec![id("ver", 1), id("fnode", 2), id("fnode", 3)];
    expected.sort();
    assert_eq!(read.parent_node_ids(), expected);
    assert_eq!(
        read.occurrences()
            .iter()
            .map(|entry| entry.path())
            .collect::<Vec<_>>(),
        ["Alpha.txt", "copies/alpha.txt"]
    );
    assert_eq!(fixture.state, before);
}

#[test]
fn equal_content_of_another_file_does_not_enter_this_preflight() {
    let mut fixture = Fixture::new();
    let before = fixture
        .capture(4, &HistoryWindowCancellation::new())
        .unwrap()
        .parent_node_ids()
        .to_vec();
    let mut other = fixture.state.branches[&id("br", 1)].clone();
    other.branch_id = id("br", 5);
    other.file_id = Some(id("file", 2));
    other.file_node_id = Some(id("ver", 3));
    fixture
        .state
        .branches
        .insert(other.branch_id.clone(), other);
    fixture.state.sequence += 1;
    let read = fixture
        .capture(4, &HistoryWindowCancellation::new())
        .unwrap();
    assert_eq!(read.branches().len(), 4);
    assert_eq!(read.parent_node_ids(), before);
    assert!(
        read.occurrences()
            .iter()
            .all(|entry| entry.file_id == id("file", 1))
    );
}

#[test]
fn a_same_content_path_cannot_substitute_for_the_stable_file() {
    let mut fixture = Fixture::new();
    fixture.path = name_policy::LookupPath::parse("B.txt").unwrap();
    let before = fixture.state.clone();
    assert_eq!(
        fixture
            .capture(4, &HistoryWindowCancellation::new())
            .unwrap_err()
            .code(),
        ErrorCode::FileNotFound
    );
    assert_eq!(fixture.state, before);
}

#[test]
fn no_file_branch_is_an_explicit_gap_not_a_synthetic_main() {
    let mut fixture = Fixture::new();
    fixture.state.branches.clear();
    fixture.state.file_branch_aliases.clear();
    let before = fixture.state.clone();
    assert_eq!(
        fixture
            .capture(4, &HistoryWindowCancellation::new())
            .unwrap_err()
            .code(),
        ErrorCode::FileBranchRequired
    );
    assert_eq!(fixture.state, before);
}

#[test]
fn named_and_initialized_directory_repositories_are_not_file_deletions() {
    let cancel = HistoryWindowCancellation::new();
    let mut named = Fixture::new();
    named
        .state
        .repositories
        .get_mut(named.repository_id.as_str())
        .unwrap()
        .file_manager_root = false;
    assert_eq!(
        named.capture(4, &cancel).unwrap_err().code(),
        ErrorCode::FileBranchScope
    );
    let mut root = Fixture::new();
    root.initialized(root.repository_id.as_str().into());
    assert_eq!(
        root.capture(4, &cancel).unwrap_err().code(),
        ErrorCode::FileBranchScope
    );
    let mut nested = Fixture::new();
    nested.initialized(id("entry", 10));
    // The selected occurrence is outside copies. Its second alias is not.
    assert_eq!(
        nested.capture(4, &cancel).unwrap_err().code(),
        ErrorCode::FileBranchScope
    );
}

#[test]
fn foreign_retained_branches_and_alias_targets_are_never_silently_omitted() {
    let cancel = HistoryWindowCancellation::new();
    let mut foreign = Fixture::new();
    foreign
        .state
        .branches
        .get_mut(&id("br", 3))
        .unwrap()
        .history_tree_id = id("hist", 2);
    assert_eq!(
        foreign.capture(4, &cancel).unwrap_err().code(),
        ErrorCode::FileBranchScope
    );
    let mut alias = Fixture::new();
    let mut other = alias.state.branches[&id("br", 1)].clone();
    other.branch_id = id("br", 5);
    other.file_id = Some(id("file", 2));
    alias.state.branches.insert(other.branch_id.clone(), other);
    alias
        .state
        .file_branch_aliases
        .insert(id("br", 4), id("br", 5));
    assert_eq!(
        alias.capture(4, &cancel).unwrap_err().code(),
        ErrorCode::FileBranchScope
    );
}

#[test]
fn retained_head_and_its_body_must_belong_to_the_same_history_scope() {
    let cancel = HistoryWindowCancellation::new();
    let mut node = Fixture::new();
    node.state
        .file_path_nodes
        .get_mut(&id("fnode", 2))
        .unwrap()
        .history_tree_id = id("hist", 2);
    assert_eq!(
        node.capture(4, &cancel).unwrap_err().code(),
        ErrorCode::FileBranchScope
    );
    let mut body = Fixture::new();
    let mut foreign = body.state.versions[&id("ver", 2)].clone();
    foreign.history_tree_id = id("hist", 2);
    body.state
        .versions
        .insert(foreign.version_id.clone(), foreign);
    assert_eq!(
        body.capture(4, &cancel).unwrap_err().code(),
        ErrorCode::FileBranchScope
    );
}

#[test]
fn an_alias_keeps_its_own_head_and_missing_targets_are_errors() {
    let cancel = HistoryWindowCancellation::new();
    let mut fixture = Fixture::new();
    fixture
        .state
        .branches
        .get_mut(&id("br", 4))
        .unwrap()
        .file_node_id = Some(id("fnode", 2));
    let read = fixture.capture(4, &cancel).unwrap();
    assert_eq!(read.branches()[3].node_id(), id("fnode", 2));
    assert_ne!(read.branches()[3].node_id(), read.branches()[0].node_id());
    drop(read);
    fixture
        .state
        .file_branch_aliases
        .insert(id("br", 4), id("br", 999));
    assert_eq!(
        fixture.capture(4, &cancel).unwrap_err().code(),
        ErrorCode::StateInvalid
    );
}

#[test]
fn complete_branch_and_alias_budgets_reject_instead_of_truncating() {
    let cancel = HistoryWindowCancellation::new();
    let mut fixture = Fixture::new();
    assert_eq!(
        fixture.capture(3, &cancel).unwrap_err().code(),
        ErrorCode::ResourceLimit
    );
    for number in 4..=6 {
        fixture
            .entries
            .push(entry(&format!("more-{number}"), number, 1, 1));
    }
    fixture
        .entries
        .sort_by(|left, right| left.lookup_key().cmp(right.lookup_key()));
    assert_eq!(
        fixture.capture(4, &cancel).unwrap_err().code(),
        ErrorCode::ResourceLimit
    );
    assert_eq!(
        fixture.capture(0, &cancel).unwrap_err().code(),
        ErrorCode::ResourceLimit
    );
}

#[test]
fn cancelled_preflight_is_read_only() {
    let fixture = Fixture::new();
    let before = fixture.state.clone();
    let cancel = HistoryWindowCancellation::new();
    cancel.cancel();
    assert_eq!(
        fixture.capture(4, &cancel).unwrap_err().code(),
        ErrorCode::OperationAborted
    );
    assert_eq!(fixture.state, before);
}

fn commit(engine: &mut Engine, workspace: crate::Workspace) {
    engine
        .commit_workspace(crate::CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: OperationId::generate().unwrap(),
            principal: "admin".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "preflight regression".into(),
            request_digest: None,
        })
        .unwrap();
}

#[test]
fn real_engine_preflight_preserves_prepared_other_file_write_and_reopens() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("system");
    let mut engine = Engine::initialize(
        &root,
        Config::development([temp.path().join("disk")]).unwrap(),
    )
    .unwrap();
    let repository = engine
        .create_disk_root("P01", "admin", OperationId::generate().unwrap())
        .unwrap();
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "A.txt", b"same")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "B.txt", b"same")
        .unwrap();
    commit(&mut engine, workspace);
    let original = engine
        .namespace_tree(&repository.main_branch_id)
        .unwrap()
        .get("A.txt")
        .unwrap()
        .clone();
    let repository_id = HistoryRepositoryId::parse(&repository.repository_id).unwrap();
    let file_id = HistoryFileId::parse(&original.file_id).unwrap();
    let path = name_policy::LookupPath::parse("a.TXT").unwrap();
    let target = FileDeletionTarget {
        repository_id: &repository_id,
        file_id: &file_id,
        path: &path,
    };
    let cancel = HistoryWindowCancellation::new();
    let initial = engine.file_deletion_preflight(target, &cancel).unwrap();
    assert_eq!(initial.branches().len(), 1);
    assert_eq!(
        initial.parent_node_ids(),
        std::slice::from_ref(&original.version_id)
    );
    drop(initial);
    engine
        .create_file_branch(
            &original.file_id,
            &original.version_id,
            &crate::BranchName::parse("review").unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "B.txt", b"new B")
        .unwrap();
    let before = engine.state().clone();
    let read = engine.file_deletion_preflight(target, &cancel).unwrap();
    assert_eq!(read.branches().len(), 2);
    assert_eq!(
        read.parent_node_ids(),
        std::slice::from_ref(&original.version_id)
    );
    assert_eq!(read.occurrences().len(), 1);
    assert_eq!(engine.state(), &before);
    drop(read);
    commit(&mut engine, workspace);
    assert_eq!(
        engine
            .namespace_tree(&repository.main_branch_id)
            .unwrap()
            .get("A.txt"),
        Some(&original)
    );
    let other = engine
        .namespace_tree(&repository.main_branch_id)
        .unwrap()
        .get("B.txt")
        .unwrap()
        .clone();
    assert_ne!(other.file_id, original.file_id);
    assert_eq!(other.size, 5);
    drop(engine);
    let engine = Engine::open(&root).unwrap();
    let read = engine.file_deletion_preflight(target, &cancel).unwrap();
    assert_eq!(read.parent_node_ids(), [original.version_id]);
    assert_eq!(read.branches().len(), 2);
    assert_eq!(read.occurrences()[0].entry_id, original.entry_id);
}

fn fork_preflight_branch(engine: &mut Engine, file: &str, node: &str, name: &str) -> String {
    let body = engine.file_node_version(file, node).unwrap();
    engine
        .create_file_branch_from_node(
            file,
            &body,
            node,
            &crate::BranchName::parse(name).unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap()
        .branch_id
        .unwrap()
}

fn tag_preflight_branch(engine: &mut Engine, file: &str, branch: &str, tag: &str) -> String {
    let head = engine.file_branch_head(branch).unwrap();
    engine
        .update_file_node_tags(crate::FileNodeTagsUpdateRequest {
            file_id: file,
            targets: &[crate::FileTagTarget {
                branch_id: branch.into(),
                source_node_id: head.clone(),
                expected_node_id: head,
            }],
            add: &[tag.into()],
            remove: &[],
            location: None,
            principal: "admin",
            operation_id: OperationId::generate().unwrap(),
        })
        .unwrap()
        .result["nodes"][0]["node_id"]
        .as_str()
        .unwrap()
        .into()
}

struct ExpectedHead {
    branch: String,
    node: String,
    kind: FileDeletionNodeKind,
    alias: Option<String>,
}

fn expected_head(
    branch: &str,
    node: &str,
    kind: FileDeletionNodeKind,
    alias: Option<&str>,
) -> ExpectedHead {
    ExpectedHead {
        branch: branch.into(),
        node: node.into(),
        kind,
        alias: alias.map(str::to_owned),
    }
}

fn assert_preflight_heads(
    engine: &Engine,
    target: FileDeletionTarget<'_>,
    body: &str,
    expected: &[ExpectedHead],
) {
    let cancel = HistoryWindowCancellation::new();
    let read = engine.file_deletion_preflight(target, &cancel).unwrap();
    assert_eq!(read.branches().len(), expected.len());
    for wanted in expected {
        let retained = read
            .branches()
            .iter()
            .find(|head| head.branch().branch_id == wanted.branch)
            .unwrap();
        assert_eq!(retained.node_id(), wanted.node);
        assert_eq!(retained.head().node_id(), wanted.node);
        assert_eq!(retained.head().kind(), wanted.kind);
        assert_eq!(retained.head().body().version_id, body);
        assert_eq!(retained.head().body().file_id, target.file_id.as_str());
        assert!(std::ptr::eq(
            retained.head().body(),
            &engine.state().versions[body]
        ));
        assert_eq!(retained.alias_of(), wanted.alias.as_deref());
    }
    assert_eq!(
        read.parent_node_ids()
            .iter()
            .map(String::as_str)
            .collect::<std::collections::BTreeSet<_>>(),
        expected
            .iter()
            .map(|wanted| wanted.node.as_str())
            .collect::<std::collections::BTreeSet<_>>(),
    );
    assert_eq!(read.occurrences().len(), 1);
    assert_eq!(read.occurrences()[0].path(), "renamed.txt");
}

fn merge_preflight_branches(
    engine: &mut Engine,
    file: &str,
    survivor: &str,
    consumed: &str,
    chosen: &str,
) -> String {
    let inputs = [survivor, consumed].map(|branch| crate::FileMergeInput {
        branch_id: branch.into(),
        expected_node_id: engine.file_branch_head(branch).unwrap(),
    });
    engine
        .merge_file_branches(crate::FileBranchMergeRequest {
            file_id: file,
            inputs: &inputs,
            chosen_node_id: chosen,
            surviving_branch_id: survivor,
            location: None,
            principal: "admin",
            operation_id: OperationId::generate().unwrap(),
        })
        .unwrap()
        .result["node_id"]
        .as_str()
        .unwrap()
        .into()
}

fn create_distinct_preflight_heads(
    engine: &mut Engine,
    repository: &str,
    original: &Entry,
) -> Vec<ExpectedHead> {
    let survivor = engine.file_context(repository, "A.txt").unwrap().branch_id;
    let content = fork_preflight_branch(engine, &original.file_id, &original.version_id, "content");
    let first_tag = tag_preflight_branch(engine, &original.file_id, &survivor, "first");
    let tagged = fork_preflight_branch(engine, &original.file_id, &first_tag, "tagged");
    let rename = engine
        .file_namespace(
            repository,
            crate::NamespaceOperation::parse(
                crate::NamespaceOperationKind::Rename,
                "A.txt",
                Some("renamed.txt"),
            )
            .unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap()
        .result["file_node_ids"][0]
        .as_str()
        .unwrap()
        .to_owned();
    let renamed = fork_preflight_branch(engine, &original.file_id, &rename, "renamed");
    let second_tag = tag_preflight_branch(engine, &original.file_id, &survivor, "second");
    let consumed = fork_preflight_branch(engine, &original.file_id, &second_tag, "consumed");
    let versions = engine.state().versions.len();
    let merged = merge_preflight_branches(
        engine,
        &original.file_id,
        &survivor,
        &consumed,
        &original.version_id,
    );
    assert_eq!(engine.state().versions.len(), versions);
    assert_eq!(
        engine.state().branches[&consumed].state,
        crate::BranchState::Merged
    );
    assert_ne!(merged, second_tag);
    vec![
        expected_head(
            &content,
            &original.version_id,
            FileDeletionNodeKind::Content,
            None,
        ),
        expected_head(&tagged, &first_tag, FileDeletionNodeKind::Tags, None),
        expected_head(&renamed, &rename, FileDeletionNodeKind::Rename, None),
        expected_head(&survivor, &merged, FileDeletionNodeKind::Merge, None),
        expected_head(
            &consumed,
            &second_tag,
            FileDeletionNodeKind::Tags,
            Some(&survivor),
        ),
    ]
}

#[test]
fn real_engine_preflight_keeps_all_node_kinds_and_merged_alias_heads_distinct_from_body() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("system");
    let mut engine = Engine::initialize(
        &root,
        Config::development([temp.path().join("disk")]).unwrap(),
    )
    .unwrap();
    let repository = engine
        .create_disk_root("P01", "admin", OperationId::generate().unwrap())
        .unwrap();
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "A.txt", b"same")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "B.txt", b"same")
        .unwrap();
    commit(&mut engine, workspace);
    let original = engine
        .namespace_tree(&repository.main_branch_id)
        .unwrap()
        .get("A.txt")
        .unwrap()
        .clone();
    let expected =
        create_distinct_preflight_heads(&mut engine, &repository.repository_id, &original);
    let repository_id = HistoryRepositoryId::parse(&repository.repository_id).unwrap();
    let file_id = HistoryFileId::parse(&original.file_id).unwrap();
    let path = name_policy::LookupPath::parse("RENAMED.txt").unwrap();
    let target = FileDeletionTarget {
        repository_id: &repository_id,
        file_id: &file_id,
        path: &path,
    };
    let before = engine.state().clone();
    assert_preflight_heads(&engine, target, &original.version_id, &expected);
    assert_eq!(engine.state(), &before);
    let workspace = engine
        .create_workspace(&repository.main_branch_id, "admin")
        .unwrap();
    engine
        .stage_bytes(
            &workspace.workspace_id,
            "B.txt",
            b"other file still writable",
        )
        .unwrap();
    commit(&mut engine, workspace);
    assert_preflight_heads(&engine, target, &original.version_id, &expected);
    drop(engine);
    let engine = Engine::open(&root).unwrap();
    assert_preflight_heads(&engine, target, &original.version_id, &expected);
}
