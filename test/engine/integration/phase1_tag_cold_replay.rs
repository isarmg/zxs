//! Phase-one native saves and permanent tag receipts across cold reopens.
//! Every mutation and content read uses the production Engine on temporary roots.
use std::{collections::BTreeSet, io::Read};
use storage_engine::*;

const PATH: &str = "资料/报告.txt";

struct Fixture {
    root: tempfile::TempDir,
    engine: Engine,
    repository: Repository,
}
struct Saved {
    entry: Entry,
    request: CommitRequest,
    receipt: OperationResult,
}
#[derive(Clone)]
struct Labels {
    file: String,
    target: FileTagTarget,
    add: Vec<String>,
    remove: Vec<String>,
    operation: OperationId,
}
impl Labels {
    fn submit(&self, engine: &mut Engine) -> Result<OperationResult> {
        engine.update_file_node_tags(FileNodeTagsUpdateRequest {
            file_id: &self.file,
            targets: std::slice::from_ref(&self.target),
            add: &self.add,
            remove: &self.remove,
            location: None,
            principal: "admin",
            operation_id: self.operation,
        })
    }
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let config =
            Config::independent_roots(Mode::Development, [root.path().join("primary")]).unwrap();
        let mut engine = Engine::initialize(&root.path().join("system"), config).unwrap();
        let repository = engine.create_disk_root("D01", "admin", op()).unwrap();
        Self {
            root,
            engine,
            repository,
        }
    }
    fn reopen(self) -> Self {
        let Self {
            root,
            engine,
            repository,
        } = self;
        drop(engine);
        let engine = Engine::open(&root.path().join("system")).unwrap();
        Self {
            root,
            engine,
            repository,
        }
    }
    fn entry(&self) -> Entry {
        self.engine
            .namespace_tree(&self.repository.main_branch_id)
            .unwrap()
            .get(PATH)
            .unwrap()
            .clone()
    }
    fn save(&mut self, bytes: &[u8]) -> Saved {
        let w = self
            .engine
            .create_workspace(&self.repository.main_branch_id, "admin")
            .unwrap();
        self.engine
            .stage_bytes(&w.workspace_id, PATH, bytes)
            .unwrap();
        let request = CommitRequest {
            workspace_id: w.workspace_id,
            operation_id: op(),
            principal: "admin".into(),
            expected_head: w.base_head,
            fencing_token: w.fencing_token,
            message: "phase-one cold tag replay".into(),
            request_digest: None,
        };
        let receipt = self.engine.commit_workspace(request.clone()).unwrap();
        Saved {
            entry: self.entry(),
            request,
            receipt,
        }
    }
    fn labels(&self, source: &str, add: &[&str], remove: &[&str]) -> Labels {
        let context = self
            .engine
            .file_context(&self.repository.repository_id, PATH)
            .unwrap();
        Labels {
            file: self.entry().file_id,
            target: FileTagTarget {
                expected_node_id: self.engine.file_branch_head(&context.branch_id).unwrap(),
                branch_id: context.branch_id,
                source_node_id: source.into(),
            },
            add: add.iter().map(|value| (*value).into()).collect(),
            remove: remove.iter().map(|value| (*value).into()).collect(),
            operation: op(),
        }
    }
    fn body(&self, version: &str) -> Vec<u8> {
        let mut opened = self.engine.open_content(version).unwrap();
        let mut bytes = Vec::new();
        opened.file.read_to_end(&mut bytes).unwrap();
        bytes
    }
    fn assert_current(&self, entry: &Entry, head: &str, tags: &[&str]) {
        assert_eq!(self.entry(), *entry);
        let context = self
            .engine
            .file_context(&self.repository.repository_id, PATH)
            .unwrap();
        assert_eq!(
            self.engine.file_branch_head(&context.branch_id).unwrap(),
            head
        );
        assert_eq!(
            context.node_id.as_deref().unwrap_or(&entry.version_id),
            head
        );
        assert_eq!(
            self.engine.file_node_tags(&entry.file_id, head).unwrap(),
            tag_set(tags)
        );
    }
}
fn op() -> OperationId {
    OperationId::generate().unwrap()
}
fn tag_set(values: &[&str]) -> BTreeSet<String> {
    values.iter().map(|value| (*value).into()).collect()
}
fn node(receipt: &OperationResult) -> String {
    receipt.result["nodes"][0]["node_id"]
        .as_str()
        .unwrap()
        .into()
}
fn snapshot(engine: &Engine) -> json_codec::Value {
    json_codec::to_value(engine.state()).unwrap()
}

#[test]
fn old_save_and_tag_receipts_do_not_rewind_newer_content_after_cold_open() {
    let mut f = Fixture::new();
    let first_bytes = (0..=255).cycle().take(262144 + 17).collect::<Vec<u8>>();
    let first = f.save(&first_bytes);
    let add = f.labels(&first.entry.version_id, &["keep", "review"], &[]);
    let added = add.submit(&mut f.engine).unwrap();
    let first_tag = node(&added);
    let first_tag_record = f.engine.state().file_metadata_nodes[&first_tag].clone();
    let second = f.save(b"short replacement\0with bytes\xff");
    assert_eq!(second.entry.file_id, first.entry.file_id);
    assert_ne!(second.entry.version_id, first.entry.version_id);
    assert_eq!(
        f.engine
            .file_node_parents(&first.entry.file_id, &second.entry.version_id)
            .unwrap(),
        vec![first_tag.clone()]
    );
    assert_eq!(
        f.engine
            .file_node_tags(&first.entry.file_id, &second.entry.version_id)
            .unwrap(),
        tag_set(&["keep", "review"])
    );
    let remove = f.labels(&second.entry.version_id, &["final"], &["review", "review"]);
    let removed = remove.submit(&mut f.engine).unwrap();
    let head = node(&removed);
    f.assert_current(&second.entry, &head, &["final", "keep"]);
    let versions = f.engine.state().versions.len();
    let metadata = f.engine.state().file_metadata_nodes.len();
    let mut f = f.reopen();
    f.assert_current(&second.entry, &head, &["final", "keep"]);
    let before = snapshot(&f.engine);
    // All four requests refer to their exact original encoding and old baselines.
    for saved in [&first, &second] {
        assert_eq!(
            f.engine.commit_workspace(saved.request.clone()).unwrap(),
            saved.receipt
        );
        assert_eq!(snapshot(&f.engine), before);
    }
    for (request, receipt) in [(&add, &added), (&remove, &removed)] {
        assert_eq!(request.submit(&mut f.engine).unwrap(), *receipt);
        assert_eq!(snapshot(&f.engine), before);
    }
    let mut changed_save = first.request.clone();
    changed_save.message.push_str(" changed");
    assert_eq!(
        f.engine.commit_workspace(changed_save).unwrap_err().code(),
        ErrorCode::IdempotencyKeyReused
    );
    assert_eq!(snapshot(&f.engine), before);
    let mut changed_remove = remove.clone();
    changed_remove.remove.pop();
    assert_eq!(
        changed_remove.submit(&mut f.engine).unwrap_err().code(),
        ErrorCode::IdempotencyKeyReused
    );
    assert_eq!(snapshot(&f.engine), before);
    assert_eq!(f.engine.state().versions.len(), versions);
    assert_eq!(f.engine.state().file_metadata_nodes.len(), metadata);
    assert_eq!(
        f.engine.state().file_metadata_nodes[&first_tag],
        first_tag_record
    );
    assert_eq!(f.body(&first.entry.version_id), first_bytes);
    assert_eq!(
        f.body(&second.entry.version_id),
        b"short replacement\0with bytes\xff"
    );
    let f = f.reopen();
    f.assert_current(&second.entry, &head, &["final", "keep"]);
    assert_eq!(
        f.engine.state().file_metadata_nodes[&first_tag],
        first_tag_record
    );
}

#[test]
fn noop_tag_receipts_remain_permanent_after_another_save_and_two_cold_opens() {
    let mut f = Fixture::new();
    let first = f.save(b"initial");
    let seed = f.labels(&first.entry.version_id, &["ready"], &[]);
    let seeded = seed.submit(&mut f.engine).unwrap();
    let head = node(&seeded);
    let noop = f.labels(&head, &["ready", "ready"], &["absent", "absent"]);
    let metadata = f.engine.state().file_metadata_nodes.len();
    let versions = f.engine.state().versions.len();
    let operations = f.engine.state().operations.len();
    let sequence = f.engine.state().sequence;
    let receipt = noop.submit(&mut f.engine).unwrap();
    assert_eq!(receipt.result["nodes"][0]["no_change"], true);
    assert_eq!(node(&receipt), head);
    assert_eq!(receipt.result["content_versions_created"], "0");
    assert_eq!(f.engine.state().file_metadata_nodes.len(), metadata);
    assert_eq!(f.engine.state().versions.len(), versions);
    assert_eq!(f.engine.state().operations.len(), operations + 1);
    assert_eq!(f.engine.state().sequence, sequence + 1);
    f.assert_current(&first.entry, &head, &["ready"]);
    let mut f = f.reopen();
    let before = snapshot(&f.engine);
    assert_eq!(noop.submit(&mut f.engine).unwrap(), receipt);
    assert_eq!(snapshot(&f.engine), before);
    let second = f.save(b"new body after no-op label receipt");
    assert_eq!(second.entry.file_id, first.entry.file_id);
    assert_eq!(
        f.engine
            .file_node_parents(&first.entry.file_id, &second.entry.version_id)
            .unwrap(),
        vec![head]
    );
    f.assert_current(&second.entry, &second.entry.version_id, &["ready"]);
    let mut f = f.reopen();
    let before = snapshot(&f.engine);
    assert_eq!(noop.submit(&mut f.engine).unwrap(), receipt);
    assert_eq!(snapshot(&f.engine), before);
    let mut changed_encoding = noop.clone();
    changed_encoding.add.pop();
    assert_eq!(
        changed_encoding.submit(&mut f.engine).unwrap_err().code(),
        ErrorCode::IdempotencyKeyReused
    );
    assert_eq!(snapshot(&f.engine), before);
    let mut stale_fresh = noop;
    stale_fresh.operation = op();
    assert_eq!(
        stale_fresh.submit(&mut f.engine).unwrap_err().code(),
        ErrorCode::FileBranchConflict
    );
    assert!(
        !f.engine
            .state()
            .operations
            .contains_key(&stale_fresh.operation)
    );
    assert_eq!(snapshot(&f.engine), before);
    assert_eq!(f.engine.state().file_metadata_nodes.len(), metadata);
    assert_eq!(f.engine.state().versions.len(), versions + 1);
    f.assert_current(&second.entry, &second.entry.version_id, &["ready"]);
    assert_eq!(f.body(&first.entry.version_id), b"initial");
    assert_eq!(
        f.body(&second.entry.version_id),
        b"new body after no-op label receipt"
    );
}
