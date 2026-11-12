use super::*;
use crate::*;
use storage_contract::file_container_tags::{Context, Tag, TagSet};
use storage_contract::Generation;

fn setup() -> (tempfile::TempDir, Engine, Repository) {
    let root = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &root.path().join("system"),
        Config::development([
            root.path().join("p"),
            root.path().join("b"),
            root.path().join("c"),
        ])
        .unwrap(),
    )
    .unwrap();
    let repo = engine
        .create_disk_root(
            "P01",
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    (root, engine, repo)
}
fn save(engine: &mut Engine, branch: &str, path: &str, bytes: &[u8]) -> Entry {
    let w = engine.create_workspace(branch, "admin").unwrap();
    engine.stage_bytes(&w.workspace_id, path, bytes).unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: w.workspace_id,
            operation_id: storage_contract::OperationId::generate().unwrap(),
            principal: "admin".into(),
            expected_head: w.base_head,
            fencing_token: w.fencing_token,
            message: "labels fixture".into(),
            request_digest: None,
        })
        .unwrap();
    engine
        .list_tree(branch)
        .unwrap()
        .into_iter()
        .find(|e| e.path.as_str() == path)
        .unwrap()
}

fn request(file: &Entry, repository: &Repository, generation: u64, tags: &[&str]) -> ReplaceCommand {
    ReplaceCommand {
        file_id: file.file_id.parse().unwrap(),
        context: Context {
            repository_id: repository.repository_id.parse().unwrap(),
            path: name_policy::PortablePath::parse(file.path()).unwrap(),
        },
        expected_generation: Generation::new(generation),
        tags: TagSet::new(tags.iter().map(|tag| Tag::parse((*tag).into()).unwrap())).unwrap(),
    }
}

#[test]
fn container_tags_replace_cas_noop_replay_and_reopen_without_graph_changes() {
    let (root, mut engine, repository) = setup();
    let file = save(&mut engine, &repository.main_branch_id, "a.txt", b"first");
    let principal = "admin".parse().unwrap();
    let command = request(&file, &repository, 0, &["review", "草稿"]);
    let mut read = ReadRequest {
        file_id: command.file_id,
        context: command.context.clone(),
    };
    assert!(engine.file_container_tags(&read).unwrap().tags.is_empty());
    let before = engine.state().clone();
    let operation = OperationId::generate().unwrap();
    let receipt = engine
        .replace_container_tags(&command, &principal, operation)
        .unwrap();
    let after = engine.state();
    assert_eq!(before.versions, after.versions);
    assert_eq!(before.branches, after.branches);
    assert_eq!(before.commits, after.commits);
    assert_eq!(before.directory_trees, after.directory_trees);
    assert_eq!(before.file_metadata_nodes, after.file_metadata_nodes);
    assert_eq!(
        engine.file_container_tags(&read).unwrap().generation,
        Generation::new(1),
    );
    assert_eq!(
        engine.replace_container_tags(&command, &principal, operation).unwrap(),
        receipt,
    );
    assert_eq!(
        engine
            .replace_container_tags(&command, &principal, OperationId::generate().unwrap())
            .unwrap_err()
            .code(),
        ErrorCode::HeadConflict,
    );
    let noop = request(&file, &repository, 1, &["review", "草稿"]);
    let noop_receipt = engine
        .replace_container_tags(&noop, &principal, OperationId::generate().unwrap())
        .unwrap();
    let (replacement, _, _) = crate::persist::container_tags::decode(&noop_receipt).unwrap();
    assert!(!replacement.changed);
    assert_eq!(replacement.generation, Generation::new(1));
    assert_eq!(
        engine
            .replace_container_tags(&noop, &principal, operation)
            .unwrap_err()
            .code(),
        ErrorCode::IdempotencyKeyReused,
    );
    save(&mut engine, &repository.main_branch_id, "a.txt", b"second");
    assert_eq!(engine.file_container_tags(&read).unwrap().tags, command.tags);
    engine
        .file_namespace(
            &repository.repository_id,
            NamespaceOperation::parse(
                NamespaceOperationKind::Rename,
                "a.txt",
                Some("renamed.txt"),
            )
            .unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    assert_eq!(
        engine.replace_container_tags(&command, &principal, operation).unwrap(),
        receipt,
    );
    read.context.path = name_policy::PortablePath::parse("renamed.txt").unwrap();
    drop(engine);
    let mut engine = Engine::open(&root.path().join("system")).unwrap();
    assert_eq!(
        engine.file_container_tags(&read).unwrap().generation,
        Generation::new(1),
    );
    assert_eq!(
        engine.replace_container_tags(&command, &principal, operation).unwrap(),
        receipt,
    );
}

#[test]
fn container_tags_rejects_wrong_context_and_generation_exhaustion() {
    let (_root, mut engine, repository) = setup();
    let file = save(&mut engine, &repository.main_branch_id, "a.txt", b"first");
    let mut command = request(&file, &repository, 0, &["review"]);
    command.context.path = name_policy::PortablePath::parse("missing.txt").unwrap();
    assert_eq!(
        plan(engine.state(), None, &command).unwrap_err().code(),
        ErrorCode::FileNotFound,
    );
    command.context.path = name_policy::PortablePath::parse("a.txt").unwrap();
    command.expected_generation = Generation::new(u64::MAX);
    let mut state = engine.state().clone();
    state.container_tags.insert(
        command.file_id,
        ContainerTags::new(TagSet::empty(), Generation::new(u64::MAX)),
    );
    assert_eq!(
        plan(&state, None, &command).unwrap_err().code(),
        ErrorCode::ResourceLimit,
    );
}
