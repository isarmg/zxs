use super::*;
use crate::engine_backend::EngineBackend;
use std::{
    os::unix::fs::FileExt,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
};
use storage_engine::{CommitRequest, Config, Engine};

impl Session {
    pub(super) fn entries(&self) -> Result<Vec<Entry>, ApiError> {
        let tree = self.call(crate::BackendAction::Tree, json!({"branch_id":self.branch}))?;
        json_codec::from_value(
            tree.get("entries")
                .cloned()
                .ok_or_else(|| ApiError::invalid("missing tree entries"))?,
        )
        .map_err(ApiError::invalid_source)
    }
}

#[path = "admission_tests.rs"]
mod admission_tests;
#[path = "binding_tests.rs"]
mod binding_tests;
#[path = "close_tests.rs"]
mod close_tests;
#[path = "directory_tests.rs"]
mod directory_tests;
#[path = "namespace_tests.rs"]
mod namespace_tests;
#[path = "published_selection_tests.rs"]
mod published_selection_tests;
#[path = "reconciliation_tests.rs"]
mod reconciliation_tests;

const RECONCILIATION_SNAPSHOT: usize = 1;
const RECONCILIATION_CONTENT: usize = 2;

struct LostCommitReply {
    inner: EngineBackend,
    calls: Mutex<Vec<(crate::BackendAction, Value)>>,
    authorizations: Mutex<Vec<String>>,
    fail: AtomicBool,
    reconciliation_failure: AtomicUsize,
    uploads: AtomicUsize,
    revoked: AtomicBool,
    change_before_workspace: AtomicBool,
}
impl LostCommitReply {
    fn take_authorizations(&self) -> Vec<String> {
        std::mem::take(&mut *self.authorizations.lock().unwrap())
    }

    fn take_calls(&self) -> Vec<(crate::BackendAction, Value)> {
        std::mem::take(&mut *self.calls.lock().unwrap())
    }
}
impl Backend for LostCommitReply {
    fn operation(
        &self,
        identity: &Identity,
        operation_id: storage_contract::OperationId,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        self.inner.operation(identity, operation_id)
    }
    fn commit_workspace(
        &self,
        identity: &Identity,
        command: crate::CommitWorkspace,
    ) -> Result<storage_contract::OperationResult, ApiError> {
        let value = self.inner.commit_workspace(identity, command)?;
        if self.fail.swap(false, Ordering::SeqCst) {
            return Err(ApiError::new(
                ::http::StatusCode::SERVICE_UNAVAILABLE,
                storage_contract::ErrorCode::InjectedFailure,
                "injected after actual durable publication",
            ));
        }
        Ok(value)
    }

    fn snapshot_at(
        &self,
        identity: &crate::auth::Identity,
        branch: &str,
        commit: &str,
    ) -> Result<std::sync::Arc<storage_contract::Commit>, crate::ApiError> {
        if self
            .reconciliation_failure
            .compare_exchange(
                RECONCILIATION_SNAPSHOT,
                0,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .is_ok()
        {
            return Err(ApiError::new(
                ::http::StatusCode::SERVICE_UNAVAILABLE,
                storage_contract::ErrorCode::MetadataUnavailable,
                "injected one-shot published snapshot read failure",
            ));
        }
        self.inner.snapshot_at(identity, branch, commit)
    }

    fn authorize_current(
        &self,
        identity: &crate::auth::Identity,
        branch: &str,
    ) -> Result<(), crate::ApiError> {
        self.authorizations.lock().unwrap().push(branch.to_owned());
        self.inner.authorize_current(identity, branch)
    }

    fn read_directory(
        &self,
        identity: &crate::auth::Identity,
        request: crate::ReadDirectory<'_>,
    ) -> Result<crate::DirectorySnapshot, crate::ApiError> {
        self.inner.read_directory(identity, request)
    }

    fn authorize_workspace_selection(
        &self,
        identity: &crate::auth::Identity,
        workspace: &str,
        path: &name_policy::PortablePath,
    ) -> Result<(), crate::ApiError> {
        self.inner
            .authorize_workspace_selection(identity, workspace, path)
    }

    fn lookup_current(
        &self,
        identity: &crate::auth::Identity,
        branch: &str,
        path: &name_policy::LookupPath,
    ) -> Result<Option<storage_contract::Entry>, crate::ApiError> {
        self.inner.lookup_current(identity, branch, path)
    }

    fn lookup_at(
        &self,
        identity: &crate::auth::Identity,
        branch: &str,
        commit: &str,
        path: &name_policy::LookupPath,
    ) -> Result<Option<storage_contract::Entry>, crate::ApiError> {
        self.inner.lookup_at(identity, branch, commit, path)
    }

    fn call(
        &self,
        action: crate::BackendAction,
        identity: &Identity,
        input: Value,
    ) -> Result<Value, ApiError> {
        self.calls.lock().unwrap().push((action, input.clone()));
        if action == crate::BackendAction::Version && self.revoked.load(Ordering::SeqCst) {
            return Err(ApiError::forbidden());
        }
        if action == crate::BackendAction::CreateWorkspace
            && self.change_before_workspace.swap(false, Ordering::SeqCst)
        {
            let workspace: Workspace =
                json_codec::from_value(self.inner.call(action, identity, input.clone())?).unwrap();
            let source = tempfile::NamedTempFile::new().unwrap();
            fs::write(source.path(), b"new").unwrap();
            self.inner.upload(
                identity,
                &workspace.workspace_id,
                &new_id("entry"),
                &name_policy::PortablePath::parse("report.txt").unwrap(),
                &crate::UploadSource::from_file(source.reopen().unwrap(), u64::MAX)?,
            )?;
            self.inner.commit_workspace(
                identity,
                crate::CommitWorkspace {
                    workspace_id: workspace.workspace_id,
                    operation_id: OperationId::generate(),
                    expected_head: workspace.base_head,
                    fencing_token: workspace.fencing_token,
                    message: "concurrent update during SMB open".into(),
                    request_digest: None,
                },
            )?;
        }
        let value = self.inner.call(action, identity, input)?;
        Ok(value)
    }
    fn upload(
        &self,
        identity: &Identity,
        workspace: &str,
        item: &str,
        path: &name_policy::PortablePath,
        source: &crate::UploadSource,
    ) -> Result<Value, ApiError> {
        self.uploads.fetch_add(1, Ordering::SeqCst);
        self.inner.upload(identity, workspace, item, path, source)
    }
    fn content(&self, identity: &Identity, version: &str) -> Result<Content, ApiError> {
        if self
            .reconciliation_failure
            .compare_exchange(
                RECONCILIATION_CONTENT,
                0,
                Ordering::SeqCst,
                Ordering::SeqCst,
            )
            .is_ok()
        {
            return Err(ApiError::new(
                ::http::StatusCode::SERVICE_UNAVAILABLE,
                storage_contract::ErrorCode::MetadataUnavailable,
                "injected one-shot published content read failure",
            ));
        }
        self.inner.content(identity, version)
    }
}

fn fixture() -> (tempfile::TempDir, Arc<Shared>, Arc<LostCommitReply>, String) {
    let temp = tempfile::tempdir().unwrap();
    let config = Config::development([
        temp.path().join("p"),
        temp.path().join("b"),
        temp.path().join("c"),
    ]);
    let mut engine = Engine::initialize(&temp.path().join("state"), config).unwrap();
    let repo = engine
        .create_repository(
            "资料",
            "P01",
            3,
            "alice",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let workspace = engine
        .create_workspace(&repo.main_branch_id, "alice")
        .unwrap();
    engine
        .stage_bytes(&workspace.workspace_id, "report.txt", b"old")
        .unwrap();
    engine
        .commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_contract::OperationId::generate(),
            principal: "alice".into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "initial".into(),
            request_digest: None,
        })
        .unwrap();
    let auth = Arc::new(Mutex::new(
        AuthStore::initialize(&temp.path().join("auth"), "alice", "long-test-password").unwrap(),
    ));
    let spool = temp.path().join("spool");
    fs::create_dir(&spool).unwrap();
    fs::set_permissions(&spool, fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir(spool.join("empty-directory")).unwrap();
    let backend = Arc::new(LostCommitReply {
        inner: EngineBackend::new(engine),
        calls: Mutex::new(Vec::new()),
        authorizations: Mutex::new(Vec::new()),
        fail: AtomicBool::new(false),
        reconciliation_failure: AtomicUsize::new(0),
        uploads: AtomicUsize::new(0),
        revoked: AtomicBool::new(false),
        change_before_workspace: AtomicBool::new(false),
    });
    backend
        .inner
        .set_management_auth(crate::lock(&auth).reader())
        .unwrap();
    let budget = budget::Budget::recover(
        &spool,
        crate::SpoolLimits {
            bytes_per_account: 16 * 1024 * 1024,
            bytes_total: 64 * 1024 * 1024,
            recovery_reserve_bytes: 1,
        },
    )
    .unwrap();
    let shared = Arc::new(Shared {
        budget,
        max_file_bytes: 4096,
        authenticated_sessions: Mutex::new(0),
        backend: backend.clone(),
        username: "alice".into(),
        auth,
        spool,
        file_writers: Mutex::new(BTreeSet::new()),
        namespace_operations: Mutex::new(BTreeSet::new()),
    });
    (temp, shared, backend, repo.main_branch_id)
}
fn identity(shared: &Shared) -> Identity {
    shared.auth.lock().unwrap().sole_identity().unwrap()
}
fn repository(shared: &Shared) -> String {
    shared
        .backend
        .call(
            crate::BackendAction::Repositories,
            &identity(shared),
            json!({}),
        )
        .unwrap()["repositories"][0]["repository_id"]
        .as_str()
        .unwrap()
        .to_owned()
}

fn session(shared: Arc<Shared>, branch: &str) -> Session {
    Session {
        identity: identity(&shared),
        shared,
        branch: branch.into(),
        handles: BTreeMap::new(),
        next_handle: 1,
        writer_locks: BTreeSet::new(),
        max_file_bytes: 4096,
    }
}
fn open_file(session: &mut Session, writable: bool) -> (OpenResult, File) {
    let (result, fd) = session
        .open("report.txt".into(), writable, false, false, false, false)
        .unwrap();
    (
        json_codec::from_value(result).unwrap(),
        File::from(fd.unwrap()),
    )
}

fn select_pending_file(shared: &Shared, branch: &str) -> String {
    let identity = &identity(shared);
    let tree = shared
        .backend
        .call(
            crate::BackendAction::Tree,
            identity,
            json!({"branch_id":branch}),
        )
        .unwrap();
    let entry = tree["entries"]
        .as_array()
        .unwrap()
        .iter()
        .find(|entry| entry["path"] == "report.txt")
        .unwrap();
    let file = entry["file_id"].as_str().unwrap().to_owned();
    let created = shared.backend.call(crate::BackendAction::FileManagerBranch, identity, json!({
        "file_id":file,"source_version_id":entry["version_id"],"name":"draft","operation_id":new_id("op")
    })).unwrap();
    shared.backend.call(crate::BackendAction::FileManagerSelect, identity, json!({
        "file_id":file,"branch_id":created["result"]["branch_id"],"operation_id":new_id("op")
    })).unwrap();
    file
}

fn directory_fixture(shared: &Shared) -> (String, String, String) {
    let identity = &identity(shared);
    let repositories = shared
        .backend
        .call(crate::BackendAction::Repositories, identity, json!({}))
        .unwrap();
    let repository = repositories["repositories"][0]["repository_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let created = shared
        .backend
        .call(
            crate::BackendAction::FileManagerDirectoryBranches,
            identity,
            json!({
                "slot_id":"P01","path":"资料","name":"draft","operation_id":new_id("op")
            }),
        )
        .unwrap();
    let initial = shared
        .backend
        .call(
            crate::BackendAction::DefaultBranch,
            identity,
            json!({"repository_id":repository}),
        )
        .unwrap();
    (
        repository,
        initial["directory_branch_id"].as_str().unwrap().to_owned(),
        created["result"]["directory_branch_id"]
            .as_str()
            .unwrap()
            .to_owned(),
    )
}

fn select_directory(shared: &Shared, repository: &str, directory: &str) {
    shared.backend.call(crate::BackendAction::UpdateDefaultBranch, &identity(shared), json!({
        "repository_id":repository,"directory_branch_id":directory,"operation_id":new_id("op")
    })).unwrap();
}

#[test]
fn directory_default_switch_fences_open_and_already_published_writers() {
    let (_temp, shared, backend, branch) = fixture();
    let (repository, main, draft) = directory_fixture(&shared);
    let mut one = session(shared.clone(), &branch);
    let (opened, file) = open_file(&mut one, true);
    file.write_all_at(b"new", 0).unwrap();
    select_directory(&shared, &repository, &draft);
    let error = one.dirty(opened.handle).unwrap_err();
    assert_eq!(
        error.code,
        storage_contract::ErrorCode::DirectoryBranchConflict
    );
    assert_eq!(error.status, 409);
    let writer = match &one.handles[&opened.handle] {
        Handle::Write(w) => w,
        _ => unreachable!(),
    };
    assert!(writer.source.is_file());
    assert!(writer.source.with_extension("work.json").is_file());
    assert_eq!(backend.uploads.load(Ordering::SeqCst), 0);
    assert_eq!(
        one.find(&name_policy::LookupPath::parse("report.txt").unwrap())
            .unwrap()
            .unwrap()
            .size,
        3
    );
    one.close(opened.handle, true).unwrap();

    let (opened, file) = open_file(&mut one, true);
    file.write_all_at(b"own", 0).unwrap();
    one.dirty(opened.handle).unwrap();
    one.flush(opened.handle).unwrap();
    // Return through another root selection, then to this exact content.
    // Equal file tuples must still retain the old handle's destination.
    select_directory(&shared, &repository, &main);
    select_directory(&shared, &repository, &draft);
    file.write_all_at(b"bad", 0).unwrap();
    assert_eq!(
        one.dirty(opened.handle).unwrap_err().code,
        storage_contract::ErrorCode::DirectoryBranchConflict
    );
    assert_eq!(backend.uploads.load(Ordering::SeqCst), 1);
    let writer = match &one.handles[&opened.handle] {
        Handle::Write(w) => w,
        _ => unreachable!(),
    };
    assert!(writer.dirty && writer.source.is_file());
    one.close(opened.handle, true).unwrap();
    let current = one
        .find(&name_policy::LookupPath::parse("report.txt").unwrap())
        .unwrap()
        .unwrap();
    let content = backend.content(&one.identity, &current.version_id).unwrap();
    let mut bytes = [0; 3];
    content.file.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"own");
}

#[test]
fn namespace_move_transfers_only_selected_directory_views_and_keeps_read_handle() {
    let (_temp, shared, backend, branch) = fixture();
    let mut client = session(shared.clone(), &branch);
    for (path, bytes) in [
        ("a/report.txt", b"source".as_slice()),
        ("b/stay.txt", b"sibling".as_slice()),
    ] {
        let (opened, fd) = client
            .open(path.into(), true, true, false, false, false)
            .unwrap();
        let opened: OpenResult = json_codec::from_value(opened).unwrap();
        File::from(fd.unwrap()).write_all_at(bytes, 0).unwrap();
        client.dirty(opened.handle).unwrap();
        client.close(opened.handle, false).unwrap();
    }
    let identity = &identity(&shared);
    let repo = shared
        .backend
        .call(crate::BackendAction::Repositories, identity, json!({}))
        .unwrap()["repositories"][0]["repository_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let register = |path: &str, name: &str| {
        shared.backend.call(crate::BackendAction::FileManagerDirectoryBranches,identity,json!({
        "slot_id":"P01","path":format!("资料/{path}"),"name":name,"operation_id":new_id("op")
    })).unwrap()["result"]["directory_branch_id"].as_str().unwrap().to_owned()
    };
    let a_m = register("a", "m");
    let a_n = register("a", "n");
    let b_l = register("b", "l");
    let b_p = register("b", "p");
    for (path, name) in [("a", "m"), ("b", "l")] {
        shared.backend.call(crate::BackendAction::FileManagerDirectorySelect,identity,json!({
            "slot_id":"P01","path":format!("资料/{path}"),"name":name,"operation_id":new_id("op")
        })).unwrap();
    }
    let scope = |id: &str| {
        shared
            .backend
            .call(
                crate::BackendAction::DirectoryBranchScopes,
                identity,
                json!({"repository_id":repo}),
            )
            .unwrap()["directory_branches"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["directory_branch_id"] == id)
            .unwrap()
            .clone()
    };
    let m0 = scope(&a_m);
    let n0 = scope(&a_n);
    let l0 = scope(&b_l);
    let p0 = scope(&b_p);
    let source = client
        .find(&name_policy::LookupPath::parse("a/report.txt").unwrap())
        .unwrap()
        .unwrap();
    let (opened, fd) = client
        .open("a/report.txt".into(), false, false, false, false, false)
        .unwrap();
    let opened: OpenResult = json_codec::from_value(opened).unwrap();
    let reader = File::from(fd.unwrap());
    client
        .dispatch(Request::Namespace {
            operation_id: new_id("op"),
            kind: smb_protocol::protocol::NamespaceOperation::Rename,
            path: "a/report.txt".into(),
            destination: Some("b/report.txt".into()),
            replace: false,
        })
        .unwrap();
    assert!(
        client
            .find(&name_policy::LookupPath::parse("a/report.txt").unwrap())
            .unwrap()
            .is_none()
    );
    let moved = client
        .find(&name_policy::LookupPath::parse("b/report.txt").unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(moved.file_id, source.file_id);
    assert_eq!(moved.version_id, source.version_id);
    let m1 = scope(&a_m);
    let l1 = scope(&b_l);
    assert_ne!(m1["head_tree_id"], m0["head_tree_id"]);
    assert_ne!(l1["head_tree_id"], l0["head_tree_id"]);
    assert_eq!(scope(&a_n)["head_tree_id"], n0["head_tree_id"]);
    assert_eq!(scope(&b_p)["head_tree_id"], p0["head_tree_id"]);
    let mut bytes = [0; 6];
    reader.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"source");
    client.close(opened.handle, false).unwrap();
    let (opened, fd) = client
        .open("b/report.txt".into(), true, false, false, false, false)
        .unwrap();
    let opened: OpenResult = json_codec::from_value(opened).unwrap();
    File::from(fd.unwrap()).write_all_at(b"edited", 0).unwrap();
    client.dirty(opened.handle).unwrap();
    client.close(opened.handle, false).unwrap();
    assert_eq!(
        scope(&a_m)["head_tree_id"],
        m1["head_tree_id"],
        "source stopped tracking the moved file"
    );
    assert_ne!(scope(&b_l)["head_tree_id"], l1["head_tree_id"]);
    assert_eq!(scope(&a_n)["head_tree_id"], n0["head_tree_id"]);
    assert_eq!(scope(&b_p)["head_tree_id"], p0["head_tree_id"]);
    let previous = backend.content(identity, &source.version_id).unwrap();
    previous.file.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"source");
}

#[test]
fn newly_created_ancestor_selection_fences_an_existing_create_handle() {
    let (_temp, shared, _backend, branch) = fixture();
    let mut original = session(shared.clone(), &branch);
    let (opened, file) = original
        .open("future/new.txt".into(), true, true, false, false, false)
        .unwrap();
    let opened: OpenResult = json_codec::from_value(opened).unwrap();
    let file = File::from(file.unwrap());
    let mut creator = session(shared.clone(), &branch);
    let (other, other_fd) = creator
        .open("future/other.txt".into(), true, true, false, false, false)
        .unwrap();
    let other: OpenResult = json_codec::from_value(other).unwrap();
    File::from(other_fd.unwrap())
        .write_all_at(b"other", 0)
        .unwrap();
    creator.dirty(other.handle).unwrap();
    creator.close(other.handle, false).unwrap();
    shared
        .backend
        .call(
            crate::BackendAction::FileManagerDirectoryBranches,
            &identity(&shared),
            json!({
                "slot_id":"P01","path":"资料/future","name":"alternate","operation_id":new_id("op")
            }),
        )
        .unwrap();
    shared
        .backend
        .call(
            crate::BackendAction::FileManagerDirectorySelect,
            &identity(&shared),
            json!({
                "slot_id":"P01","path":"资料/future","name":"alternate","operation_id":new_id("op")
            }),
        )
        .unwrap();
    file.write_all_at(b"keep", 0).unwrap();
    assert_eq!(
        original.dirty(opened.handle).unwrap_err().code,
        storage_contract::ErrorCode::DirectoryBranchConflict
    );
    let writer = match &original.handles[&opened.handle] {
        Handle::Write(w) => w,
        _ => unreachable!(),
    };
    assert!(writer.source.is_file() && writer.dirty);
    assert!(
        original
            .find(&name_policy::LookupPath::parse("future/new.txt").unwrap())
            .unwrap()
            .is_none()
    );
    original.close(opened.handle, true).unwrap();
}

#[test]
fn nested_directory_switch_does_not_fence_an_unrelated_open_file() {
    let (_temp, shared, backend, branch) = fixture();
    let mut creator = session(shared.clone(), &branch);
    let (opened, file) = creator
        .open("nested/other.txt".into(), true, true, false, false, false)
        .unwrap();
    let opened: OpenResult = json_codec::from_value(opened).unwrap();
    File::from(file.unwrap()).write_all_at(b"other", 0).unwrap();
    creator.dirty(opened.handle).unwrap();
    creator.close(opened.handle, false).unwrap();
    let (_repository, _main, _draft) = directory_fixture(&shared);
    shared
        .backend
        .call(
            crate::BackendAction::FileManagerDirectoryBranches,
            &identity(&shared),
            json!({
                "slot_id":"P01","path":"资料/nested","name":"alternate","operation_id":new_id("op")
            }),
        )
        .unwrap();
    let mut one = session(shared.clone(), &branch);
    let (opened, file) = open_file(&mut one, true);
    shared
        .backend
        .call(
            crate::BackendAction::FileManagerDirectorySelect,
            &identity(&shared),
            json!({
                "slot_id":"P01","path":"资料/nested","name":"alternate","operation_id":new_id("op")
            }),
        )
        .unwrap();
    file.write_all_at(b"new", 0).unwrap();
    one.dirty(opened.handle).unwrap();
    one.close(opened.handle, false).unwrap();
    let current = one
        .find(&name_policy::LookupPath::parse("report.txt").unwrap())
        .unwrap()
        .unwrap();
    let content = backend.content(&one.identity, &current.version_id).unwrap();
    let mut bytes = [0; 3];
    content.file.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"new");
}

#[test]
fn unix_rpc_default_switch_preserves_read_fds_and_targets_new_writes() {
    let (_temp, shared, backend, branch) = fixture();
    let (repository, main, draft) = directory_fixture(&shared);
    select_directory(&shared, &repository, &draft);
    let mut initial = session(shared.clone(), &branch);
    let (opened, file) = open_file(&mut initial, true);
    file.write_all_at(b"new", 0).unwrap();
    initial.dirty(opened.handle).unwrap();
    initial.close(opened.handle, false).unwrap();

    let (client, server) = std::os::unix::net::UnixStream::pair().unwrap();
    let server_shared = shared.clone();
    let server = std::thread::spawn(move || {
        run_connection(FrameStream::new(server).unwrap(), server_shared).unwrap()
    });
    let mut wire = FrameStream::new(client).unwrap();
    let rpc = |wire: &mut FrameStream, request: Request| {
        wire.send(&request, None).unwrap();
        wire.receive::<Response>().unwrap()
    };
    let (hello, fd) = rpc(
        &mut wire,
        Request::Hello {
            protocol: PROTOCOL.into(),
            capabilities: REQUIRED_CAPABILITIES
                .iter()
                .map(|value| (*value).into())
                .collect(),
            username: "alice".into(),
            repository_id: repository.clone(),
        },
    );
    assert!(matches!(hello, Response::Ok { .. }) && fd.is_none());
    let open = || Request::Open {
        path: "report.txt".into(),
        writable: false,
        create: false,
        truncate: false,
        exclusive: false,
        write_through: false,
    };
    let (Response::Ok { result }, pinned) = rpc(&mut wire, open()) else {
        panic!("read open failed")
    };
    let pinned_open: OpenResult = json_codec::from_value(result).unwrap();
    let pinned = File::from(pinned.unwrap());
    let mut bytes = [0; 3];
    pinned.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"new");
    select_directory(&shared, &repository, &main);
    let (Response::Ok { result }, visible) = rpc(&mut wire, open()) else {
        panic!("read after switch failed")
    };
    let visible_open: OpenResult = json_codec::from_value(result).unwrap();
    File::from(visible.unwrap())
        .read_exact_at(&mut bytes, 0)
        .unwrap();
    assert_eq!(&bytes, b"old");
    pinned.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"new", "existing immutable read FD stays pinned");
    let (Response::Ok { result }, writable_fd) = rpc(
        &mut wire,
        Request::Open {
            path: "report.txt".into(),
            writable: true,
            create: false,
            truncate: false,
            exclusive: false,
            write_through: false,
        },
    ) else {
        panic!("write after switch failed")
    };
    let writable: OpenResult = json_codec::from_value(result).unwrap();
    let writable_fd = File::from(writable_fd.unwrap());
    writable_fd.write_all_at(b"smb", 0).unwrap();
    assert!(matches!(
        rpc(
            &mut wire,
            Request::Dirty {
                handle: writable.handle
            }
        )
        .0,
        Response::Ok { .. }
    ));
    assert!(matches!(
        rpc(
            &mut wire,
            Request::Close {
                handle: writable.handle
            }
        )
        .0,
        Response::Ok { .. }
    ));
    for handle in [pinned_open.handle, visible_open.handle] {
        assert!(matches!(
            rpc(&mut wire, Request::Close { handle }).0,
            Response::Ok { .. }
        ));
    }
    drop(wire);
    server.join().unwrap();
    let observer = session(shared.clone(), &branch);
    let entry = observer
        .find(&name_policy::LookupPath::parse("report.txt").unwrap())
        .unwrap()
        .unwrap();
    let content = backend
        .content(&observer.identity, &entry.version_id)
        .unwrap();
    content.file.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"smb");
    select_directory(&shared, &repository, &draft);
    let entry = observer
        .find(&name_policy::LookupPath::parse("report.txt").unwrap())
        .unwrap()
        .unwrap();
    backend
        .content(&observer.identity, &entry.version_id)
        .unwrap()
        .file
        .read_exact_at(&mut bytes, 0)
        .unwrap();
    assert_eq!(
        &bytes, b"new",
        "saving a different combination preserves the original frozen view"
    );
}

#[test]
fn unwritten_branch_reads_parent_and_materializes_only_on_dirty_save() {
    let (_temp, shared, backend, branch) = fixture();
    let file = select_pending_file(&shared, &branch);
    let history = || {
        shared
            .backend
            .call(
                crate::BackendAction::FileManagerFileHistory,
                &identity(&shared),
                json!({"file_id":file}),
            )
            .unwrap()
    };
    let before = history();
    assert_eq!(before["versions"].as_array().unwrap().len(), 1);
    let mut session = session(shared.clone(), &branch);
    assert_eq!(before["inherited"], true);
    assert!(before["own_version_id"].is_null());
    let (parent_read, parent_content) = open_file(&mut session, false);
    let mut parent_bytes = [0u8; 3];
    parent_content.read_exact_at(&mut parent_bytes, 0).unwrap();
    assert_eq!(&parent_bytes, b"old");
    let (opened, staged) = open_file(&mut session, true);
    assert_eq!(opened.size, 3);
    assert_eq!(
        opened.version_id.as_deref(),
        before["versions"][0]["version_id"].as_str()
    );
    assert_eq!(staged.metadata().unwrap().len(), 3);
    let mut copied = [0u8; 3];
    staged.read_exact_at(&mut copied, 0).unwrap();
    assert_eq!(&copied, b"old");
    session.close(opened.handle, false).unwrap();
    assert_eq!(history(), before);
    assert_eq!(backend.uploads.load(Ordering::SeqCst), 0);

    let (opened, staged) = open_file(&mut session, true);
    staged.write_all_at(b"unpublished", 0).unwrap();
    session.dirty(opened.handle).unwrap();
    session.close(opened.handle, true).unwrap();
    assert_eq!(
        history(),
        before,
        "aborting an actual write retains the inherited branch record"
    );

    let (opened, staged) = open_file(&mut session, true);
    staged.write_all_at(b"X", 0).unwrap();
    session.dirty(opened.handle).unwrap();
    session.close(opened.handle, false).unwrap();
    let after = history();
    assert_eq!(after["versions"].as_array().unwrap().len(), 2);
    let current = after["versions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|version| version["current"] == true)
        .unwrap();
    assert_eq!(
        current["content_hash"],
        blake3::hash(b"Xld").to_hex().as_str()
    );
    assert_eq!(
        current["source_version_ids"],
        json!([before["versions"][0]["version_id"]])
    );
    let (read, mut content) = open_file(&mut session, false);
    let mut bytes = Vec::new();
    content.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"Xld");
    assert_eq!(after["inherited"], false);
    assert_eq!(after["materialized"], true);
    assert_eq!(after["own_version_id"], current["version_id"]);
    parent_content.read_exact_at(&mut parent_bytes, 0).unwrap();
    assert_eq!(
        &parent_bytes, b"old",
        "the inherited read handle keeps the parent version"
    );
    session.close(parent_read.handle, false).unwrap();
    session.close(read.handle, false).unwrap();
}

#[test]
fn explicit_truncation_materializes_an_empty_pending_branch() {
    let (_temp, shared, _backend, branch) = fixture();
    let file = select_pending_file(&shared, &branch);
    let mut session = session(shared.clone(), &branch);
    let (opened, descriptor) = session
        .open("report.txt".into(), true, false, true, false, false)
        .unwrap();
    let opened: OpenResult = json_codec::from_value(opened).unwrap();
    drop(descriptor);
    session.close(opened.handle, false).unwrap();
    let history = shared
        .backend
        .call(
            crate::BackendAction::FileManagerFileHistory,
            &identity(&shared),
            json!({"file_id":file}),
        )
        .unwrap();
    assert_eq!(history["versions"].as_array().unwrap().len(), 2);
    let current = history["versions"]
        .as_array()
        .unwrap()
        .iter()
        .find(|version| version["current"] == true)
        .unwrap();
    assert_eq!(current["size"], "0");
    assert_eq!(current["content_hash"], blake3::hash(b"").to_hex().as_str());
    assert!(
        history["branches"]
            .as_array()
            .unwrap()
            .iter()
            .find(|branch| branch["current"] == true)
            .unwrap()["materialized"]
            .as_bool()
            .unwrap()
    );
}

#[test]
fn actual_core_flush_publishes_full_file_and_old_reader_keeps_its_version() {
    let (_temp, shared, backend, branch) = fixture();
    let mut one = session(shared.clone(), &branch);
    let (read, old) = open_file(&mut one, false);
    let (write, file) = open_file(&mut one, true);
    file.write_all_at(b"new", 0).unwrap();
    one.dirty(write.handle).unwrap();
    let mut bytes = [0u8; 3];
    old.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"old");
    let saved = one.flush(write.handle).unwrap();
    assert_eq!(saved["commit_state"], "PUBLISHED");
    old.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"old");
    let new_entry = one
        .entries()
        .unwrap()
        .into_iter()
        .find(|entry| entry.path == "report.txt")
        .unwrap();
    let content = backend
        .content(&one.identity, &new_entry.version_id)
        .unwrap();
    content.file.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"new");
    one.close(write.handle, false).unwrap();
    one.close(read.handle, false).unwrap();
}

#[test]
fn lost_commit_reply_reconciles_same_operation_without_restaging_or_second_version() {
    let (_temp, shared, backend, branch) = fixture();
    let mut one = session(shared, &branch);
    let (opened, file) = open_file(&mut one, true);
    file.write_all_at(b"new", 0).unwrap();
    one.dirty(opened.handle).unwrap();
    backend.fail.store(true, Ordering::SeqCst);
    assert_eq!(
        one.flush(opened.handle).unwrap_err().code,
        storage_contract::ErrorCode::InjectedFailure
    );
    let uploads = backend.uploads.load(Ordering::SeqCst);
    let saved = one.flush(opened.handle).unwrap();
    assert_eq!(saved["commit_state"], "PUBLISHED");
    assert_eq!(backend.uploads.load(Ordering::SeqCst), uploads);
    let entry = one
        .entries()
        .unwrap()
        .into_iter()
        .find(|entry| entry.path == "report.txt")
        .unwrap();
    let history = backend
        .call(
            crate::BackendAction::History,
            &one.identity,
            json!({"file_id":entry.file_id}),
        )
        .unwrap();
    assert_eq!(history["versions"].as_array().unwrap().len(), 2);
    one.close(opened.handle, false).unwrap();
}

#[test]
fn cross_session_writer_is_denied_and_last_close_commits() {
    let (_temp, shared, _backend, branch) = fixture();
    let mut one = session(shared.clone(), &branch);
    let mut two = session(shared, &branch);
    let (opened, file) = open_file(&mut one, true);
    let error = two
        .open("report.txt".into(), true, false, false, false, false)
        .unwrap_err();
    assert_eq!(error.code, storage_contract::ErrorCode::FileWriteBusy);
    file.write_all_at(b"end", 0).unwrap();
    one.dirty(opened.handle).unwrap();
    one.close(opened.handle, false).unwrap();
    let (next, _) = open_file(&mut two, true);
    two.close(next.handle, false).unwrap();
}

#[test]
fn different_files_publish_concurrently_and_reopen_workspace_after_unrelated_save() {
    let (_temp, shared, _backend, branch) = fixture();
    let mut one = session(shared.clone(), &branch);
    let mut two = session(shared, &branch);
    let (first, first_file) = open_file(&mut one, true);
    let (result, fd) = two
        .open("other.txt".into(), true, true, false, false, false)
        .unwrap();
    let second: OpenResult = json_codec::from_value(result).unwrap();
    let second_file = File::from(fd.unwrap());
    first_file.write_all_at(b"one", 0).unwrap();
    one.dirty(first.handle).unwrap();
    second_file.write_all_at(b"two", 0).unwrap();
    two.dirty(second.handle).unwrap();
    one.flush(first.handle).unwrap();
    two.flush(second.handle).unwrap();
    first_file.write_all_at(b"new", 0).unwrap();
    one.dirty(first.handle).unwrap();
    one.flush(first.handle).unwrap();
    let entries = one.entries().unwrap();
    assert_eq!(
        entries
            .iter()
            .filter(|e| e.kind == storage_contract::NamespaceEntryKind::File)
            .count(),
        2
    );
    let other = entries.iter().find(|e| e.path == "other.txt").unwrap();
    let content = one
        .shared
        .backend
        .content(&one.identity, &other.version_id)
        .unwrap();
    let mut bytes = [0u8; 3];
    content.file.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"two");
    one.close(first.handle, false).unwrap();
    two.close(second.handle, false).unwrap();
}
#[test]
fn namespace_locks_cover_only_affected_paths_and_case_only_replace_preserves_entry() {
    let (_temp, shared, _backend, branch) = fixture();
    let mut one = session(shared.clone(), &branch);
    let mut two = session(shared, &branch);
    let before = one
        .find(&name_policy::LookupPath::parse("report.txt").unwrap())
        .unwrap()
        .unwrap();
    two.dispatch(Request::Namespace {
        operation_id: new_id("op"),
        kind: smb_protocol::protocol::NamespaceOperation::Rename,
        path: "report.txt".into(),
        destination: Some("REPORT.txt".into()),
        replace: true,
    })
    .unwrap();
    let after = two
        .find(&name_policy::LookupPath::parse("REPORT.txt").unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(after.entry_id, before.entry_id);
    assert_eq!(after.version_id, before.version_id);
    assert_eq!(after.path, "REPORT.txt");
    let (opened, fd) = one
        .open("dir/file.txt".into(), true, true, false, false, false)
        .unwrap();
    let opened: OpenResult = json_codec::from_value(opened).unwrap();
    let file = File::from(fd.unwrap());
    assert_eq!(
        two.dispatch(Request::Namespace {
            operation_id: new_id("op"),
            kind: smb_protocol::protocol::NamespaceOperation::Rename,
            path: "dir".into(),
            destination: Some("moved".into()),
            replace: false
        })
        .unwrap_err()
        .code,
        storage_contract::ErrorCode::FileWriteBusy
    );
    two.dispatch(Request::Namespace {
        operation_id: new_id("op"),
        kind: smb_protocol::protocol::NamespaceOperation::Mkdir,
        path: "unrelated".into(),
        destination: None,
        replace: false,
    })
    .unwrap();
    file.write_all_at(b"bytes", 0).unwrap();
    one.dirty(opened.handle).unwrap();
    one.close(opened.handle, false).unwrap();
    assert!(
        two.find(&name_policy::LookupPath::parse("unrelated").unwrap())
            .unwrap()
            .is_some()
    );
    assert!(
        two.find(&name_policy::LookupPath::parse("dir/file.txt").unwrap())
            .unwrap()
            .is_some()
    );
    assert_eq!(
        two.dispatch(Request::Namespace {
            operation_id: new_id("op"),
            kind: smb_protocol::protocol::NamespaceOperation::Rename,
            path: "REPORT.txt".into(),
            destination: Some("unrelated".into()),
            replace: true
        })
        .unwrap_err()
        .code,
        storage_contract::ErrorCode::InvalidRequest
    );
    two.dispatch(Request::Namespace {
        operation_id: new_id("op"),
        kind: smb_protocol::protocol::NamespaceOperation::Mkdir,
        path: "after-failure".into(),
        destination: None,
        replace: false,
    })
    .unwrap();
}

#[test]
fn listing_uses_portable_lookup_keys_and_rejects_files_or_missing_directories() {
    let (_temp, shared, _backend, branch) = fixture();
    let mut session = session(shared, &branch);
    for path in ["Straße", "STRASSE/子目录", "Straße-other"] {
        session
            .dispatch(Request::Namespace {
                operation_id: new_id("op"),
                kind: smb_protocol::protocol::NamespaceOperation::Mkdir,
                path: path.into(),
                destination: None,
                replace: false,
            })
            .unwrap();
    }
    let (opened, fd) = session
        .open(
            "STRASSE/子目录/内容.txt".into(),
            true,
            true,
            false,
            false,
            false,
        )
        .unwrap();
    let opened: OpenResult = json_codec::from_value(opened).unwrap();
    let file = File::from(fd.unwrap());
    file.write_all_at(b"content", 0).unwrap();
    session.dirty(opened.handle).unwrap();
    session.close(opened.handle, false).unwrap();
    for (path, expected_name) in [("straße", "子目录"), ("strasse/子目录", "内容.txt")] {
        let (opened, _) = session
            .open(path.into(), false, false, false, false, false)
            .unwrap();
        let opened: OpenResult = json_codec::from_value(opened).unwrap();
        let (listed, _) = session
            .dispatch(Request::ReadDirectory {
                handle: opened.handle,
                restart: false,
            })
            .unwrap();
        let entries = listed["entries"].as_array().unwrap();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["name"], expected_name);
        assert_eq!(listed["more"], false);
        session.close(opened.handle, false).unwrap();
    }
    let (file, _) = session
        .open("REPORT.TXT".into(), false, false, false, false, false)
        .unwrap();
    let file: OpenResult = json_codec::from_value(file).unwrap();
    assert_eq!(
        session
            .dispatch(Request::ReadDirectory {
                handle: file.handle,
                restart: false
            })
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::NotDirectory
    );
    assert_eq!(
        session
            .open("absent".into(), false, false, false, false, false)
            .unwrap_err()
            .status,
        404
    );
}

#[test]
fn changed_bytes_after_lost_publication_reply_are_retained_for_a_second_save() {
    let (_temp, shared, backend, branch) = fixture();
    let mut one = session(shared, &branch);
    let (opened, file) = open_file(&mut one, true);
    file.write_all_at(b"new", 0).unwrap();
    one.dirty(opened.handle).unwrap();
    backend.fail.store(true, Ordering::SeqCst);
    assert_eq!(
        one.flush(opened.handle).unwrap_err().code,
        storage_contract::ErrorCode::InjectedFailure
    );
    file.write_all_at(b"end", 0).unwrap();
    assert_eq!(
        one.flush(opened.handle).unwrap_err().code,
        storage_contract::ErrorCode::WorkspaceRebaseRequired
    );
    one.dirty(opened.handle).unwrap();
    one.flush(opened.handle).unwrap();
    let current = one
        .find(&name_policy::LookupPath::parse("report.txt").unwrap())
        .unwrap()
        .unwrap();
    let content = backend.content(&one.identity, &current.version_id).unwrap();
    let mut bytes = [0u8; 3];
    content.file.read_exact_at(&mut bytes, 0).unwrap();
    assert_eq!(&bytes, b"end");
    one.close(opened.handle, false).unwrap();
}

#[test]
fn external_same_file_change_rejects_rebase_and_preserves_work_bytes() {
    let (temp, shared, backend, branch) = fixture();
    let mut one = session(shared, &branch);
    let (opened, file) = open_file(&mut one, true);
    file.write_all_at(b"one", 0).unwrap();
    one.dirty(opened.handle).unwrap();
    one.flush(opened.handle).unwrap();
    let workspace: Workspace = json_codec::from_value(
        backend
            .call(
                crate::BackendAction::CreateWorkspace,
                &one.identity,
                json!({"branch_id":branch}),
            )
            .unwrap(),
    )
    .unwrap();
    let source = temp.path().join("external.txt");
    fs::write(&source, b"api").unwrap();
    backend
        .upload(
            &one.identity,
            &workspace.workspace_id,
            &opened.entry_id,
            &name_policy::PortablePath::parse("report.txt").unwrap(),
            &crate::UploadSource::from_file(File::open(&source).unwrap(), u64::MAX).unwrap(),
        )
        .unwrap();
    backend
        .commit_workspace(
            &one.identity,
            crate::CommitWorkspace {
                workspace_id: workspace.workspace_id,
                operation_id: OperationId::generate(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "external app".into(),
                request_digest: None,
            },
        )
        .unwrap();
    file.write_all_at(b"end", 0).unwrap();
    assert_eq!(
        one.dirty(opened.handle).unwrap_err().code,
        storage_contract::ErrorCode::FileVersionConflict
    );
    let pending = match &one.handles[&opened.handle] {
        Handle::Write(writer) => writer.source.clone(),
        _ => panic!("write handle missing"),
    };
    assert_eq!(
        one.close(opened.handle, false).unwrap_err().code,
        storage_contract::ErrorCode::WorkspaceRebaseRequired
    );
    assert!(!one.handles.contains_key(&opened.handle));
    assert!(one.writer_locks.is_empty());
    drop(one);
    assert_eq!(fs::read(pending).unwrap(), b"end");
}

#[test]
fn already_open_reader_checks_current_account_before_each_read_request() {
    let (_temp, shared, backend, branch) = fixture();
    let mut one = session(shared, &branch);
    let (opened, _file) = open_file(&mut one, false);
    one.dispatch(Request::AuthorizeRead {
        handle: opened.handle,
    })
    .unwrap();
    backend.revoked.store(true, Ordering::SeqCst);
    assert_eq!(
        one.dispatch(Request::AuthorizeRead {
            handle: opened.handle
        })
        .unwrap_err()
        .code,
        storage_contract::ErrorCode::Forbidden
    );
    assert!(one.handles.contains_key(&opened.handle));
    one.close(opened.handle, false).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancellation_closes_blocked_rpc_sessions_and_removes_its_socket() {
    let (temp, shared, _backend, _branch) = fixture();
    let parent = temp.path().join("rpc");
    fs::create_dir(&parent).unwrap();
    fs::set_permissions(&parent, fs::Permissions::from_mode(0o700)).unwrap();
    let socket = parent.join("smb.sock");
    let server_socket = socket.clone();
    let server_spool = temp.path().join("management/smb-spool");
    let repository = repository(&shared);
    let username = shared.username.clone();
    let auth = shared.auth.clone();
    let backend = shared.backend.clone();
    let server = tokio::spawn(async move {
        serve(
            crate::SmbConfig {
                socket: server_socket,
                spool_dir: server_spool,
                allowed_uid: smb_protocol::protocol::effective_uid(),
                username,
                spool_limits: crate::SpoolLimits::default(),
            },
            auth,
            backend,
            4096,
        )
        .await
    });
    for _ in 0..100 {
        if socket.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    // Binding happens on a blocking startup worker. Only a successful Hello
    // proves serve owns the socket guard and has admitted this idle session.
    let mut stream = tokio::task::spawn_blocking({
        let socket = socket.clone();
        move || {
            let mut wire = FrameStream::with_timeout(
                std::os::unix::net::UnixStream::connect(socket).unwrap(),
                std::time::Duration::from_secs(5),
            )
            .unwrap();
            wire.send(
                &Request::<String>::Hello {
                    protocol: PROTOCOL.into(),
                    capabilities: REQUIRED_CAPABILITIES
                        .iter()
                        .map(|value| (*value).into())
                        .collect(),
                    username: "alice".into(),
                    repository_id: repository,
                },
                None,
            )
            .unwrap();
            let (response, descriptor) = wire.receive::<Response>().unwrap();
            assert!(matches!(response, Response::Ok { .. }));
            assert!(descriptor.is_none());
            wire.0
        }
    })
    .await
    .unwrap();
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
    assert!(!socket.exists());
    tokio::task::spawn_blocking(move || {
        assert_eq!(stream.read(&mut [0u8; 1]).unwrap(), 0);
    })
    .await
    .unwrap();
}

fn immutable_fixture_payloads(root: &Path) -> Vec<PathBuf> {
    fn visit(root: &Path, found: &mut Vec<PathBuf>) {
        for entry in fs::read_dir(root).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                visit(&path, found);
            } else if path
                .components()
                .any(|part| part.as_os_str() == "file-containers")
                && fs::read(&path).unwrap() == b"old"
            {
                found.push(path);
            }
        }
    }
    let mut found = Vec::new();
    visit(root, &mut found);
    found.sort();
    found
}

#[test]
fn raw_fd_and_writer_copy_reject_corruption_and_use_verified_fallback() {
    for writable in [false, true] {
        let (temp, shared, _backend, branch) = fixture();
        let payloads = immutable_fixture_payloads(temp.path());
        assert_eq!(payloads.len(), 3);
        // Corrupt the primary specifically, leaving the backup replicas valid.
        let primary = payloads
            .iter()
            .find(|path| path.starts_with(temp.path().join("p")))
            .unwrap();
        fs::set_permissions(primary, fs::Permissions::from_mode(0o600)).unwrap();
        fs::write(primary, b"bad").unwrap();
        let mut active = session(shared.clone(), &branch);
        let (opened, file) = open_file(&mut active, writable);
        let mut bytes = [0u8; 3];
        file.read_exact_at(&mut bytes, 0).unwrap();
        assert_eq!(
            &bytes, b"old",
            "raw FD or writer copy exposed corrupted bytes"
        );
        active.close(opened.handle, true).unwrap();
        drop(active);
        for path in payloads {
            fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
            fs::write(path, b"bad").unwrap();
        }
        let mut denied = session(shared, &branch);
        let result = denied.open("report.txt".into(), writable, false, false, false, false);
        assert!(
            matches!(result, Err(ref error) if error.code == storage_contract::ErrorCode::ContentUnavailable)
        );
        assert!(denied.handles.is_empty(), "corrupt descriptor was admitted");
    }
}

#[test]
fn writable_open_rejects_entry_changed_before_workspace_snapshot() {
    for (truncate, create, exclusive, missing) in [
        (false, false, false, false),
        (true, false, false, false),
        (false, true, false, false),
        (false, true, true, true),
    ] {
        let (_temp, shared, backend, branch) = fixture();
        let mut client = session(shared.clone(), &branch);
        if missing {
            client
                .dispatch(Request::Namespace {
                    operation_id: new_id("op"),
                    kind: smb_protocol::protocol::NamespaceOperation::Unlink,
                    path: "report.txt".into(),
                    destination: None,
                    replace: false,
                })
                .unwrap();
        }
        backend
            .change_before_workspace
            .store(true, Ordering::SeqCst);
        let result = client.open(
            "report.txt".into(),
            true,
            create,
            truncate,
            exclusive,
            false,
        );
        assert!(
            result.is_err(),
            "stale entry must not be copied or truncated under a newer workspace baseline"
        );
        assert_eq!(
            result.unwrap_err().code,
            storage_contract::ErrorCode::FileVersionConflict
        );
        assert!(client.handles.is_empty());
        assert!(client.writer_locks.is_empty());
        let entry = client
            .find(&name_policy::LookupPath::parse("report.txt").unwrap())
            .unwrap()
            .unwrap();
        let content = backend
            .content(&client.identity, &entry.version_id)
            .unwrap();
        let mut bytes = [0; 3];
        content.file.read_exact_at(&mut bytes, 0).unwrap();
        assert_eq!(&bytes, b"new");
    }
}

#[test]
fn disconnect_before_dirty_delivery_retains_unacknowledged_fd_bytes() {
    for published in [false, true] {
        let (_temp, shared, _backend, branch) = fixture();
        let mut client = session(shared.clone(), &branch);
        let (opened, file) = open_file(&mut client, true);
        if published {
            file.write_all_at(b"one", 0).unwrap();
            client.dirty(opened.handle).unwrap();
            client.flush(opened.handle).unwrap();
        }
        let source = match &client.handles[&opened.handle] {
            Handle::Write(writer) => writer.source.clone(),
            _ => unreachable!(),
        };
        file.write_all_at(b"two", 0).unwrap();
        // The descriptor changed, but the connection died before Dirty arrived.
        drop(client);
        assert!(
            source.is_file(),
            "unacknowledged FD bytes must survive disconnect"
        );
        assert_eq!(fs::read(&source).unwrap(), b"two");
        let record: Value =
            json_codec::from_slice(&fs::read(source.with_extension("work.json")).unwrap()).unwrap();
        assert_eq!(record["descriptor_changes_unconfirmed"], true);
        assert_eq!(record["published"], published);
        assert_eq!(record["dirty"], false);
        assert!(shared.file_writers.lock().unwrap().is_empty());
    }
}

#[test]
fn unchanged_writable_close_cleans_up_but_disconnect_remains_uncertain() {
    for disconnect in [false, true] {
        let (_temp, shared, backend, branch) = fixture();
        let mut client = session(shared, &branch);
        let before = client
            .find(&name_policy::LookupPath::parse("report.txt").unwrap())
            .unwrap()
            .unwrap();
        let (opened, _file) = open_file(&mut client, true);
        let source = match &client.handles[&opened.handle] {
            Handle::Write(writer) => writer.source.clone(),
            _ => unreachable!(),
        };
        if !disconnect {
            client.close(opened.handle, false).unwrap();
            assert_eq!(
                client
                    .find(&name_policy::LookupPath::parse("report.txt").unwrap())
                    .unwrap()
                    .unwrap()
                    .version_id,
                before.version_id
            );
        }
        drop(client);
        assert_eq!(source.exists(), disconnect);
        assert_eq!(source.with_extension("work.json").exists(), disconnect);
        assert_eq!(backend.uploads.load(Ordering::SeqCst), 0);
    }
}

#[test]
fn established_sessions_preserve_idle_no_handle_read_and_unsaved_write() {
    use std::time::Duration;
    let (_temp, shared, _backend, _branch) = fixture();
    let repository = repository(&shared);
    let (client, server) = std::os::unix::net::UnixStream::pair().unwrap();
    let worker = std::thread::spawn(move || {
        run_connection(
            FrameStream::with_timeout(server, Duration::from_millis(30)).unwrap(),
            shared,
        )
    });
    let mut wire = FrameStream::new(client).unwrap();
    let rpc = |wire: &mut FrameStream, request: Request| {
        wire.send(&request, None).unwrap();
        let (response, fd) = wire.receive::<Response>().unwrap();
        let Response::Ok { result } = response else {
            panic!("RPC failed: {response:?}")
        };
        (result, fd)
    };
    rpc(
        &mut wire,
        Request::Hello {
            protocol: PROTOCOL.into(),
            capabilities: REQUIRED_CAPABILITIES
                .iter()
                .map(|value| (*value).into())
                .collect(),
            username: "alice".into(),
            repository_id: repository,
        },
    );
    std::thread::sleep(Duration::from_millis(100));
    rpc(
        &mut wire,
        Request::Stat {
            path: "report.txt".into(),
        },
    );
    for writable in [false, true] {
        let (result, fd) = rpc(
            &mut wire,
            Request::Open {
                path: "report.txt".into(),
                writable,
                create: false,
                truncate: false,
                exclusive: false,
                write_through: false,
            },
        );
        let opened: OpenResult = json_codec::from_value(result).unwrap();
        let file = File::from(fd.unwrap());
        if writable {
            file.write_all_at(b"new", 0).unwrap();
        }
        std::thread::sleep(Duration::from_millis(100));
        if writable {
            rpc(
                &mut wire,
                Request::Dirty {
                    handle: opened.handle,
                },
            );
            rpc(
                &mut wire,
                Request::Flush {
                    handle: opened.handle,
                },
            );
        } else {
            rpc(
                &mut wire,
                Request::AuthorizeRead {
                    handle: opened.handle,
                },
            );
            let mut bytes = [0; 3];
            file.read_exact_at(&mut bytes, 0).unwrap();
            assert_eq!(&bytes, b"old");
        }
        rpc(
            &mut wire,
            Request::Close {
                handle: opened.handle,
            },
        );
    }
    drop(wire);
    worker.join().unwrap().unwrap();
}

#[test]
fn flush_disconnect_cycles_keep_full_spool_reservations() {
    let (_temp, shared, _backend, branch) = fixture();
    for generation in 0..2 {
        let mut session = session(shared.clone(), &branch);
        let (opened, file) = open_file(&mut session, true);
        session
            .dispatch(Request::PrepareWrite {
                handle: opened.handle,
                end: 11,
                bytes: 11,
            })
            .unwrap();
        file.write_all_at(&[b'a' + generation], 0).unwrap();
        session.dirty(opened.handle).unwrap();
        session.flush(opened.handle).unwrap();
        // Publication does not prove that the peer's FD cannot change again.
        drop(session);
        file.write_all_at(b"after flush", 0).unwrap();
        drop(file);
    }
    let records = fs::read_dir(&shared.spool)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.path().to_string_lossy().ends_with(".budget.json"))
        .collect::<Vec<_>>();
    assert_eq!(records.len(), 2);
    for record in records {
        let value: Value = json_codec::from_slice(&fs::read(record.path()).unwrap()).unwrap();
        assert_eq!(value["admitted_end"].as_str(), Some("11"));
    }
}

#[test]
fn oversized_existing_file_is_rejected_before_private_copy() {
    let (_temp, shared, _backend, branch) = fixture();
    let mut session = session(shared.clone(), &branch);
    session.max_file_bytes = 2;
    assert!(
        session
            .open("report.txt".into(), true, false, false, false, false)
            .is_err()
    );
    let retained = fs::read_dir(&shared.spool)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().unwrap().is_file() && entry.file_name() != ".budget.lock")
        .count();
    assert_eq!(retained, 0);
}

#[test]
fn hello_rejects_missing_write_reservations_before_admitting_a_session() {
    let (_temp, shared, _backend, _branch) = fixture();
    let repository = repository(&shared);
    for capabilities in [
        None,
        Some(json!([])),
        Some(json!(["unrelated"])),
        Some(json!([
            smb_protocol::protocol::WRITE_RESERVATION_CAPABILITY
        ])),
        Some(json!([
            smb_protocol::protocol::AUTHENTICATED_ACCOUNT_CAPABILITY
        ])),
    ] {
        let mut hello = json!({
            "op":"hello","protocol":PROTOCOL,"username":"alice","repository_id":repository
        });
        let missing = capabilities.is_none();
        if let Some(capabilities) = capabilities {
            hello["capabilities"] = capabilities;
        }
        let (client, server) = std::os::unix::net::UnixStream::pair().unwrap();
        let server_shared = shared.clone();
        let worker = std::thread::spawn(move || {
            run_connection(FrameStream::new(server).unwrap(), server_shared)
        });
        let mut wire = FrameStream::new(client).unwrap();
        wire.send(&hello, None).unwrap();
        if missing {
            assert_eq!(
                wire.receive::<Response>().unwrap_err().kind(),
                io::ErrorKind::UnexpectedEof
            );
            assert!(worker.join().unwrap().is_err());
        } else {
            let (response, descriptor) = wire.receive::<Response>().unwrap();
            assert!(descriptor.is_none());
            assert!(
                matches!(response, Response::Error { code, .. } if code == storage_contract::ErrorCode::InvalidRequest)
            );
            assert_eq!(
                wire.receive::<Response>().unwrap_err().kind(),
                io::ErrorKind::UnexpectedEof
            );
            worker.join().unwrap().unwrap();
        }
        assert_eq!(fs::read_dir(&shared.spool).unwrap().count(), 2);
    }
}

#[test]
fn replaced_smb_source_or_spool_is_rejected_before_backend_publication() {
    for swap_parent in [false, true] {
        let (_temp, shared, backend, branch) = fixture();
        let mut session = session(shared.clone(), &branch);
        let (opened, file) = open_file(&mut session, true);
        file.write_all_at(b"new", 0).unwrap();
        session.dirty(opened.handle).unwrap();
        let source = match &session.handles[&opened.handle] {
            Handle::Write(writer) => writer.source.clone(),
            _ => unreachable!(),
        };
        if swap_parent {
            fs::rename(&shared.spool, shared.spool.with_extension("moved")).unwrap();
            fs::create_dir(&shared.spool).unwrap();
            fs::set_permissions(&shared.spool, fs::Permissions::from_mode(0o700)).unwrap();
        } else {
            fs::rename(&source, source.with_extension("moved")).unwrap();
        }
        fs::write(&source, b"bad").unwrap();
        fs::set_permissions(&source, fs::Permissions::from_mode(0o600)).unwrap();
        assert!(session.flush(opened.handle).is_err());
        assert_eq!(backend.uploads.load(Ordering::SeqCst), 0);
        let entry = session
            .find(&name_policy::LookupPath::parse("report.txt").unwrap())
            .unwrap()
            .unwrap();
        let published = backend
            .content(&session.identity, &entry.version_id)
            .unwrap();
        assert_eq!(published.hash, blake3::hash(b"old").to_hex().to_string());
    }
}

#[test]
fn smb_flush_preserves_the_clients_shared_descriptor_cursor() {
    use std::io::{Seek, SeekFrom};
    let (_temp, shared, _backend, branch) = fixture();
    let mut session = session(shared, &branch);
    let (opened, mut file) = open_file(&mut session, true);
    file.seek(SeekFrom::Start(2)).unwrap();
    file.write_all_at(b"new", 0).unwrap();
    session.dirty(opened.handle).unwrap();
    session.flush(opened.handle).unwrap();
    assert_eq!(file.stream_position().unwrap(), 2);
}

#[test]
fn canonical_equivalent_writable_open_binds_stored_display_path() {
    let (_temp, shared, _backend, branch) = fixture();
    let mut session = session(shared, &branch);
    session
        .namespace(
            smb_protocol::protocol::NamespaceOperation::Rename,
            "report.txt".into(),
            Some("é.txt".into()),
            false,
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let original = session
        .find(&name_policy::LookupPath::parse("é.txt").unwrap())
        .unwrap()
        .unwrap();
    let (result, fd) = session
        .open("e\u{301}.txt".into(), true, false, false, false, false)
        .unwrap();
    let opened: OpenResult = json_codec::from_value(result).unwrap();
    let Handle::Write(writer) = &session.handles[&opened.handle] else {
        panic!("expected writer")
    };
    assert_eq!(writer.path, "é.txt");
    let file = File::from(fd.unwrap());
    file.write_all_at(b"new", 0).unwrap();
    session.dirty(opened.handle).unwrap();
    session.flush(opened.handle).unwrap();
    // A second dirty generation compares two published snapshots using the
    // admitted path, even though the client originally opened an NFD spelling.
    file.write_all_at(b"end", 0).unwrap();
    session.dirty(opened.handle).unwrap();
    session.flush(opened.handle).unwrap();
    session.close(opened.handle, false).unwrap();
    let published = session
        .find(&name_policy::LookupPath::parse("e\u{301}.txt").unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(published.path, "é.txt");
    assert_eq!(published.name(), "é.txt");
    assert_eq!(published.entry_id, original.entry_id);
    assert_eq!(published.file_id, original.file_id);
    assert_ne!(published.version_id, original.version_id);
    let before = json_codec::to_value(session.entries().unwrap()).unwrap();
    let next_handle = session.next_handle;
    let handles = session.handles.len();
    let spool_entries = fs::read_dir(&session.shared.spool).unwrap().count();
    let reservations = session.shared.file_writers.lock().unwrap().clone();
    assert!(
        session
            .open("new-e\u{301}.txt".into(), true, true, false, false, false)
            .is_err()
    );
    assert_eq!(session.next_handle, next_handle);
    assert_eq!(session.handles.len(), handles);
    assert_eq!(
        fs::read_dir(&session.shared.spool).unwrap().count(),
        spool_entries
    );
    assert_eq!(*session.shared.file_writers.lock().unwrap(), reservations);
    assert!(
        session
            .open("e\u{301}.txt".into(), true, true, false, true, false)
            .is_err()
    );
    let (result, fd) = session
        .open("e\u{301}.txt".into(), true, true, true, false, false)
        .unwrap();
    let opened: OpenResult = json_codec::from_value(result).unwrap();
    let Handle::Write(writer) = &session.handles[&opened.handle] else {
        panic!("expected writer")
    };
    assert_eq!(writer.path, "é.txt");
    drop(fd);
    session.close(opened.handle, true).unwrap();
    assert!(
        session
            .namespace(
                smb_protocol::protocol::NamespaceOperation::Mkdir,
                "new-e\u{301}".into(),
                None,
                false,
                storage_contract::OperationId::generate()
            )
            .is_err()
    );
    assert!(
        session
            .namespace(
                smb_protocol::protocol::NamespaceOperation::Rename,
                "é.txt".into(),
                Some("new-e\u{301}.txt".into()),
                false,
                storage_contract::OperationId::generate()
            )
            .is_err()
    );
    assert_eq!(
        json_codec::to_value(session.entries().unwrap()).unwrap(),
        before
    );
    session
        .namespace(
            smb_protocol::protocol::NamespaceOperation::Rename,
            "e\u{301}.txt".into(),
            Some("Å.txt".into()),
            false,
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    session
        .namespace(
            smb_protocol::protocol::NamespaceOperation::Unlink,
            "A\u{30a}.txt".into(),
            None,
            false,
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    assert!(session.entries().unwrap().is_empty());
}

#[path = "snapshot_semantics_tests.rs"]
mod snapshot_semantics_tests;
