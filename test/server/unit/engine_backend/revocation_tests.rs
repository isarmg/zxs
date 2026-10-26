//! Deterministic account-publication races at the auth -> engine boundary.
use super::*;
use crate::{AuthorizedBackend, BackendAction, auth::AuthStore};
use std::{
    path::PathBuf,
    sync::{Arc, Barrier},
};
use storage_engine::{CommitRequest, Config, Workspace, new_id};

/// Pause only after AuthorizedBackend has released its account-store lock.
/// Publishing a revocation here must finish before EngineBackend captures a
/// read plan or admits a mutation; neither side may reverse the lock order.
struct PausedBackend {
    inner: Arc<EngineBackend>,
    captured: Arc<Barrier>,
    released: Arc<Barrier>,
}
impl PausedBackend {
    fn enter(&self) {
        self.captured.wait();
        self.released.wait();
    }
}
impl Backend for PausedBackend {
    fn snapshot_at(
        &self,
        identity: &crate::auth::Identity,
        branch: &str,
        commit: &str,
    ) -> Result<std::sync::Arc<storage_contract::Commit>, crate::ApiError> {
        self.enter();
        self.inner.snapshot_at(identity, branch, commit)
    }

    fn lookup_at(
        &self,
        identity: &crate::auth::Identity,
        branch: &str,
        commit: &str,
        path: &name_policy::LookupPath,
    ) -> Result<Option<storage_contract::Entry>, crate::ApiError> {
        self.enter();
        self.inner.lookup_at(identity, branch, commit, path)
    }

    fn call(
        &self,
        action: BackendAction,
        identity: &Identity,
        input: Value,
    ) -> Result<Value, ApiError> {
        self.enter();
        self.inner.call(action, identity, input)
    }
    fn upload(
        &self,
        identity: &Identity,
        workspace: &str,
        item: &str,
        path: &name_policy::PortablePath,
        source: &crate::UploadSource,
    ) -> Result<Value, ApiError> {
        self.enter();
        self.inner.upload(identity, workspace, item, path, source)
    }
    fn upload_generation(
        &self,
        identity: &Identity,
        workspace: &str,
        item: &str,
        path: &name_policy::PortablePath,
        source: &crate::UploadSource,
        generation: u64,
    ) -> Result<Value, ApiError> {
        self.enter();
        self.inner
            .upload_generation(identity, workspace, item, path, source, generation)
    }
    fn content(&self, identity: &Identity, version: &str) -> Result<Content, ApiError> {
        self.enter();
        self.inner.content(identity, version)
    }
    fn range_content(&self, identity: &Identity, version: &str) -> Result<Content, ApiError> {
        self.enter();
        self.inner.range_content(identity, version)
    }
    fn workspace_content(
        &self,
        identity: &Identity,
        workspace: &str,
        item: &str,
    ) -> Result<Content, ApiError> {
        self.enter();
        self.inner.workspace_content(identity, workspace, item)
    }
    fn checksums(&self, identity: &Identity, version: &str) -> Result<Vec<u8>, ApiError> {
        self.enter();
        self.inner.checksums(identity, version)
    }
}

struct Fixture {
    _root: tempfile::TempDir,
    backend: Arc<EngineBackend>,
    auth: Arc<Mutex<AuthStore>>,
    identity: Identity,
    token: String,
    repo: Repository,
    version: String,
    workspace: Workspace,
    item: String,
    source: PathBuf,
}
impl Fixture {
    fn new(protected: bool) -> Self {
        let root = crate::tests::private_tempdir();
        let config = Config::development([
            root.path().join("primary"),
            root.path().join("secondary"),
            root.path().join("tertiary"),
        ]);
        let mut engine = Engine::initialize(&root.path().join("system"), config).unwrap();
        let repo = engine
            .create_repository("Authorization", "P01", 3, "owner", &new_id("op"))
            .unwrap();
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "owner")
            .unwrap();
        engine
            .stage_bytes(&workspace.workspace_id, "file.txt", b"published")
            .unwrap();
        engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: new_id("op"),
                principal: "owner".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "authorization fixture".into(),
                request_digest: None,
            })
            .unwrap();
        let version = engine.list_tree(&repo.main_branch_id).unwrap()[0]
            .version_id
            .clone();
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "owner")
            .unwrap();
        let staged = engine
            .stage_bytes(&workspace.workspace_id, "private.txt", b"private")
            .unwrap();
        let source = root.path().join("source");
        std::fs::write(&source, b"new upload").unwrap();
        let backend = Arc::new(EngineBackend::new(engine));
        let auth =
            AuthStore::initialize(root.path(), "owner", "integration-test-password").unwrap();
        let mut auth = if protected {
            backend.protect_auth(auth).unwrap()
        } else {
            auth
        };
        let token = auth
            .login("owner", "integration-test-password")
            .unwrap()
            .token;
        let identity = auth.authenticate(&token).unwrap();
        Self {
            _root: root,
            backend,
            auth: Arc::new(Mutex::new(auth)),
            identity,
            token,
            repo,
            version,
            workspace,
            item: staged.item_id,
            source,
        }
    }
    fn snapshot(&self) -> (Value, Value, Vec<Vec<u8>>) {
        let engine = self.backend.engine.lock().unwrap();
        (
            json_codec::to_value(engine.state()).unwrap(),
            json_codec::to_value(engine.workspace(&self.workspace.workspace_id).unwrap()).unwrap(),
            engine
                .config()
                .disks
                .iter()
                .filter(|disk| disk.metadata_member)
                .map(|disk| {
                    std::fs::read(disk.root.join("metadata/application-log/journal.bin")).unwrap()
                })
                .collect(),
        )
    }
}

#[derive(Clone, Copy, Debug)]
enum Request {
    Metadata,
    Administrator,
    Tree,
    SmbAuthorizeRead,
    SnapshotLookup,
    Snapshot,
    Version,
    Namespace,
    Content,
    RangeContent,
    WorkspaceContent,
    Checksums,
    Upload,
    UploadGeneration,
}
impl Request {
    fn execute(self, fixture: &Fixture, backend: &dyn Backend) -> Result<(), ApiError> {
        let identity = &fixture.identity;
        let workspace = &fixture.workspace.workspace_id;
        match self {
            Self::Metadata => backend
                .call(
                    BackendAction::UpdatePlacementRule,
                    identity,
                    json!({
                        "repository_id":fixture.repo.repository_id,"rule_id":new_id("rule"),"path_prefix":"",
                        "primary_slot_id":"P01","expected_generation":"0","operation_id":new_id("op")
                    }),
                )
                .map(|_| ()),
            Self::Administrator => backend
                .call(
                    BackendAction::CreateRepository,
                    identity,
                    json!({
                        "name":"Forbidden new repository","primary_slot_id":"P01","target_copies":3,
                        "operation_id":new_id("op")
                    }),
                )
                .map(|_| ()),
            Self::Tree => backend
                .call(
                    BackendAction::Tree,
                    identity,
                    json!({
                        "branch_id":fixture.repo.main_branch_id
                    }),
                )
                .map(|_| ()),
            Self::Snapshot => backend.snapshot_at(
                identity, &fixture.repo.main_branch_id, &fixture.workspace.base_head,
            ).map(|_| ()),
            Self::SnapshotLookup => backend.lookup_at(
                identity, &fixture.repo.main_branch_id, &fixture.workspace.base_head,
                &name_policy::LookupPath::parse("file.txt").unwrap(),
            ).map(|_| ()),
            // SMB directory/write handles need current admission, not a tree.
            Self::SmbAuthorizeRead => backend
                .call(
                    BackendAction::SmbAuthorizeRead,
                    identity,
                    json!({"branch_id":fixture.repo.main_branch_id}),
                )
                .map(|_| ()),
            // Immutable SMB file handles still authorize their pinned version.
            Self::Version => backend
                .call(
                    BackendAction::Version,
                    identity,
                    json!({
                        "version_id":fixture.version
                    }),
                )
                .map(|_| ()),
            Self::Namespace => backend
                .call(
                    BackendAction::Namespace,
                    identity,
                    json!({
                        "workspace_id":workspace,"kind":"mkdir","path":"forbidden"
                    }),
                )
                .map(|_| ()),
            Self::Content => backend.content(identity, &fixture.version).map(|_| ()),
            Self::RangeContent => backend
                .range_content(identity, &fixture.version)
                .map(|_| ()),
            Self::WorkspaceContent => backend
                .workspace_content(identity, workspace, &fixture.item)
                .map(|_| ()),
            Self::Checksums => backend.checksums(identity, &fixture.version).map(|_| ()),
            Self::Upload => backend
                .upload(
                    identity,
                    workspace,
                    "new-item",
                    &name_policy::PortablePath::parse("new.txt").unwrap(),
                    &crate::UploadSource::from_file(
                        std::fs::File::open(&fixture.source).unwrap(),
                        u64::MAX,
                    )
                    .unwrap(),
                )
                .map(|_| ()),
            Self::UploadGeneration => backend
                .upload_generation(
                    identity,
                    workspace,
                    "next-item",
                    &name_policy::PortablePath::parse("next.txt").unwrap(),
                    &crate::UploadSource::from_file(
                        std::fs::File::open(&fixture.source).unwrap(),
                        u64::MAX,
                    )
                    .unwrap(),
                    1,
                )
                .map(|_| ()),
        }
    }
}

#[derive(Clone, Copy, Debug)]
enum Revocation {
    Logout,
    PasswordChange,
}

#[test]
fn completed_account_revocation_precedes_all_engine_admissions() {
    for revocation in [Revocation::Logout, Revocation::PasswordChange] {
        let fixture = Arc::new(Fixture::new(true));
        let requests = vec![
            Request::Metadata,
            Request::Administrator,
            Request::Tree,
            Request::SmbAuthorizeRead,
            Request::SnapshotLookup,
            Request::Snapshot,
            Request::Version,
            Request::Namespace,
            Request::Content,
            Request::RangeContent,
            Request::WorkspaceContent,
            Request::Checksums,
            Request::Upload,
            Request::UploadGeneration,
        ];
        let captured = Arc::new(Barrier::new(requests.len() + 1));
        let released = Arc::new(Barrier::new(requests.len() + 1));
        let backend = Arc::new(AuthorizedBackend {
            backend: Arc::new(PausedBackend {
                inner: fixture.backend.clone(),
                captured: captured.clone(),
                released: released.clone(),
            }),
            auth: fixture.auth.clone(),
        });
        let threads = requests
            .into_iter()
            .map(|request| {
                let fixture = fixture.clone();
                let backend = backend.clone();
                (
                    request,
                    std::thread::spawn(move || request.execute(&fixture, backend.as_ref())),
                )
            })
            .collect::<Vec<_>>();
        captured.wait();
        let revoked = match revocation {
            Revocation::Logout => fixture.auth.lock().unwrap().logout(&fixture.token),
            Revocation::PasswordChange => fixture.auth.lock().unwrap().change_password(
                &fixture.token,
                "integration-test-password",
                "new-integration-test-password",
            ),
        };
        let before = fixture.snapshot();
        released.wait();
        revoked.unwrap();
        for (request, thread) in threads {
            let error = thread
                .join()
                .unwrap()
                .expect_err("revoked request must be rejected");
            assert_eq!(
                error.code,
                storage_contract::ErrorCode::Unauthorized,
                "{revocation:?}: {request:?}"
            );
        }
        assert!(
            fixture.snapshot() == before,
            "rejected requests changed state, workspace or journals"
        );
    }
}

#[test]
fn revoked_session_cannot_turn_public_probes_into_data_admission() {
    let fixture = Fixture::new(true);
    assert_eq!(
        fixture
            .backend
            .call(BackendAction::Repositories, &fixture.identity, json!({}))
            .unwrap()["repositories"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(
        Request::Metadata
            .execute(&fixture, fixture.backend.as_ref())
            .is_ok()
    );
    assert!(
        Request::Content
            .execute(&fixture, fixture.backend.as_ref())
            .is_ok()
    );
    fixture.auth.lock().unwrap().logout(&fixture.token).unwrap();
    for action in [
        BackendAction::Repositories,
        BackendAction::Events,
        BackendAction::MonitorDisks,
    ] {
        assert_eq!(
            fixture
                .backend
                .call(action, &fixture.identity, json!({}))
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::Unauthorized
        );
    }
    let anonymous = Identity {
        admission_session: None,
        smb_binding: None,
        principal: String::new(),
    };
    for action in [BackendAction::Capabilities, BackendAction::Ready] {
        assert!(fixture.backend.call(action, &anonymous, json!({})).is_ok());
    }
    assert_eq!(
        fixture
            .backend
            .call(BackendAction::Repositories, &anonymous, json!({}))
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
}

#[test]
fn embedded_backend_without_protected_credentials_fails_closed() {
    let fixture = Fixture::new(false);
    assert!(
        fixture
            .backend
            .engine
            .lock()
            .unwrap()
            .auth_state()
            .is_none()
    );
    let before = fixture.snapshot();
    for request in [
        Request::Metadata,
        Request::Administrator,
        Request::Tree,
        Request::SmbAuthorizeRead,
        Request::Snapshot,
        Request::Version,
        Request::Namespace,
        Request::Content,
        Request::RangeContent,
        Request::WorkspaceContent,
        Request::Checksums,
        Request::Upload,
        Request::UploadGeneration,
    ] {
        assert_eq!(
            request
                .execute(&fixture, fixture.backend.as_ref())
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::Unauthorized
        );
    }
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn private_disk_sampling_cannot_grant_public_monitoring_access() {
    let fixture = Fixture::new(true);
    let before = fixture.snapshot();
    let observed = fixture
        .backend
        .observe_disks(&crate::InternalDiskObservation(()))
        .unwrap();
    assert_eq!(observed["disks"].as_array().unwrap().len(), 3);
    assert!(fixture.snapshot() == before);

    let synthetic = Identity {
        admission_session: None,
        smb_binding: None,
        principal: "zxs-system-telemetry".into(),
    };
    assert_eq!(
        fixture
            .backend
            .call(BackendAction::MonitorDisks, &synthetic, json!({}))
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
    fixture.auth.lock().unwrap().logout(&fixture.token).unwrap();
    assert_eq!(
        fixture
            .backend
            .call(BackendAction::MonitorDisks, &fixture.identity, json!({}))
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::Unauthorized
    );
    let before = fixture.snapshot();
    assert!(
        fixture
            .backend
            .observe_disks(&crate::InternalDiskObservation(()))
            .is_ok()
    );
    assert_eq!(fixture.snapshot(), before);
}

#[test]
fn both_upload_adapters_bind_the_copied_snapshot_before_acceptance() {
    for explicit_generation in [false, true] {
        let fixture = Fixture::new(true);
        let before = fixture.snapshot();
        let item = new_id("entry");
        let source =
            crate::UploadSource::from_file(std::fs::File::open(&fixture.source).unwrap(), u64::MAX)
                .unwrap();
        // Simulate mutation through the original inode after transport hashing,
        // not merely a pathname replacement. Copying must fail before acceptance.
        std::fs::write(&fixture.source, b"bad upload").unwrap();
        let result = if explicit_generation {
            fixture.backend.upload_generation(
                &fixture.identity,
                &fixture.workspace.workspace_id,
                &item,
                &name_policy::PortablePath::parse("bound.txt").unwrap(),
                &source,
                1,
            )
        } else {
            fixture.backend.upload(
                &fixture.identity,
                &fixture.workspace.workspace_id,
                &item,
                &name_policy::PortablePath::parse("bound.txt").unwrap(),
                &source,
            )
        };
        assert_eq!(
            result.unwrap_err().code,
            storage_contract::ErrorCode::ChecksumMismatch
        );
        assert_eq!(fixture.snapshot(), before);
    }
}
