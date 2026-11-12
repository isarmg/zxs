//! Phase-one local SMB bridge acceptance through its public listener and Client.
//! These are real framed Unix RPC/FD/Engine tests, not Samba network tests.
#[path = "phase1_smb_rpc/directory_rename.rs"]
mod directory_rename;
use smb_protocol::{
    client::{Client, RemoteError, RemoteErrorCode},
    protocol::{DirectoryPage, NamespaceOperation, OpenResult, Request},
};
use std::{
    collections::BTreeSet,
    fs::{self, File},
    io,
    os::unix::fs::FileExt,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use storage_contract::{Config, Entry, ErrorCode, OperationId, Repository};
use storage_engine::Engine;
use storage_server::{
    Backend, ReadFileHistory, ReadTree, SmbConfig, SpoolLimits,
    auth::{AuthStore, Identity},
    engine_backend::EngineBackend,
};

fn tag_command(
    file: &str,
    repository: &str,
    path: &str,
    generation: u64,
    labels: &[&str],
) -> storage_contract::file_container_tags::ReplaceCommand {
    use storage_contract::file_container_tags::{Context, ReplaceCommand, Tag, TagSet};
    ReplaceCommand {
        file_id: file.parse().unwrap(),
        context: Context {
            repository_id: repository.parse().unwrap(),
            path: name_policy::PortablePath::parse(path).unwrap(),
        },
        expected_generation: storage_contract::Generation::new(generation),
        tags: TagSet::new(labels.iter().map(|label| Tag::parse((*label).into()).unwrap())).unwrap(),
    }
}

fn write_history_content(engine: &mut Engine, branch: &str, path: &str, bytes: &[u8]) -> String {
    let workspace = engine.create_workspace(branch, "alice").unwrap();
    engine.stage_bytes(&workspace.workspace_id, path, bytes).unwrap();
    engine.commit_workspace(storage_contract::CommitRequest {
        workspace_id: workspace.workspace_id,
        principal: "alice".into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        operation_id: OperationId::generate().unwrap(),
        message: "retained history source".into(),
        request_digest: None,
    }).unwrap();
    engine.file_branch_head(branch).unwrap()
}

struct Fixture {
    runtime: tokio::runtime::Runtime,
    server: tokio::task::JoinHandle<io::Result<()>>,
    backend: Arc<EngineBackend>,
    auth: Arc<Mutex<AuthStore>>,
    root: tempfile::TempDir,
}

impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let system = root.path().join("state");
        let engine = Engine::initialize(
            &system,
            Config::development([root.path().join("p")]).unwrap(),
        )
        .unwrap();
        let backend = Arc::new(EngineBackend::new(engine));
        let auth = backend
            .protect_auth(
                AuthStore::initialize(&system, "alice", "phase-one-smb-password").unwrap(),
            )
            .unwrap();
        let auth = Arc::new(Mutex::new(auth));
        let config = SmbConfig {
            socket: root.path().join("run/bridge.sock"),
            spool_dir: root.path().join("management/smb-spool"),
            allowed_uid: rustix::process::geteuid().as_raw(),
            username: "alice".into(),
            additional_users: vec![],
            spool_limits: SpoolLimits {
                bytes_per_account: 16 * 1024 * 1024,
                bytes_total: 64 * 1024 * 1024,
                recovery_reserve_bytes: 1,
            },
        };
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .max_blocking_threads(4)
            .enable_all()
            .build()
            .unwrap();
        let server = runtime.spawn(storage_server::smb_rpc::serve(
            config,
            Arc::clone(&auth),
            backend.clone(),
            1024 * 1024,
        ));
        Self {
            runtime,
            server,
            backend,
            auth,
            root,
        }
    }

    fn connect(&mut self) -> Client {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            match Client::connect_default(&self.root.path().join("run/bridge.sock"), "alice") {
                Ok(client) => return client,
                Err(error)
                    if matches!(
                        error.kind(),
                        io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                    ) =>
                {
                    if self.server.is_finished() {
                        panic!(
                            "bridge startup stopped: {:?}",
                            self.runtime.block_on(&mut self.server)
                        );
                    }
                    let remaining = deadline.saturating_duration_since(Instant::now());
                    assert!(!remaining.is_zero(), "bridge startup deadline: {error}");
                    std::thread::sleep(remaining.min(Duration::from_millis(5)));
                }
                Err(error) => panic!("bridge connection failed: {error}"),
            }
        }
    }

    fn identity(&self) -> Identity {
        self.auth.lock().unwrap().sole_identity().unwrap()
    }

    fn repository(&self) -> Repository {
        let repositories = self
            .backend
            .repositories_snapshot(&self.identity())
            .unwrap();
        assert_eq!(repositories.len(), 1);
        let repository = repositories.into_iter().next().unwrap();
        assert!(repository.file_manager_root && repository.smb_enabled);
        assert_eq!(repository.primary_slot_id, "P01");
        assert_eq!(repository.target_copies, 1);
        repository
    }

    fn entry(&self, branch: &str, path: &str) -> Entry {
        self.backend
            .tree_snapshot(
                &self.identity(),
                ReadTree {
                    branch,
                    commit: None,
                },
            )
            .unwrap()
            .entries
            .into_iter()
            .find(|entry| entry.path() == path)
            .unwrap()
    }

    fn annotate(&self, repository: &str, source: &Entry) {
        let identity = self.identity();
        self.backend
            .replace_container_tags(
                &identity,
                storage_server::ReplaceContainerTags {
                    command: tag_command(&source.file_id, repository, source.path(), 0,
                        &["source-only"]),
                    operation_id: OperationId::generate().unwrap(),
                },
            )
            .unwrap();
        for target in [
            storage_server::NoteTarget::File(
                source.file_id.parse::<storage_contract::FileId>().unwrap().into(),
            ),
            storage_server::NoteTarget::Version(
                source
                    .version_id
                    .parse::<storage_contract::VersionId>()
                    .unwrap()
                    .into(),
            ),
        ] {
            self.backend
                .update_note(
                    &identity,
                    storage_server::UpdateNote {
                        target,
                        note: storage_contract::Note::parse("source note".into()).unwrap(),
                        operation_id: OperationId::generate().unwrap(),
                    },
                )
                .unwrap();
        }

    }

    fn cold_open(self) -> (tempfile::TempDir, Engine) {
        let Self {
            runtime,
            server,
            backend,
            auth,
            root,
        } = self;
        server.abort();
        let result = runtime.block_on(server);
        assert!(result.unwrap_err().is_cancelled());
        drop(backend);
        drop(auth);
        // The bridge cancellation guard drains its actual native workers.
        runtime.shutdown_timeout(Duration::from_secs(2));
        let engine = Engine::open(&root.path().join("state")).unwrap();
        (root, engine)
    }
}

fn open(
    client: &Client,
    path: &str,
    writable: bool,
    create: bool,
    truncate: bool,
) -> (OpenResult, File) {
    let (opened, descriptor) = client
        .open(Request::Open {
            path: path.into(),
            writable,
            create,
            truncate,
            exclusive: create,
            write_through: false,
        })
        .unwrap();
    (opened, File::from(descriptor))
}

fn close(client: &Client, handle: u64) {
    client.call(Request::Close { handle }).unwrap();
}

fn save(client: &Client, path: &str, bytes: &[u8], create: bool) -> OpenResult {
    let (opened, file) = open(client, path, true, create, !create);
    client
        .call(Request::PrepareWrite {
            handle: opened.handle,
            end: bytes.len() as u64,
            bytes: bytes.len() as u64,
        })
        .unwrap();
    file.write_all_at(bytes, 0).unwrap();
    client
        .call(Request::Dirty {
            handle: opened.handle,
        })
        .unwrap();
    client
        .call(Request::Flush {
            handle: opened.handle,
        })
        .unwrap();
    close(client, opened.handle);
    opened
}

fn read(client: &Client, path: &str) -> (OpenResult, Vec<u8>) {
    let (opened, _placeholder) = open(client, path, false, false, false);
    let mut bytes = vec![0; usize::try_from(opened.size).unwrap()];
    assert_eq!(
        client.read(opened.handle, 0, &mut bytes).unwrap(),
        bytes.len()
    );
    assert_eq!(
        client
            .read(opened.handle, opened.size, &mut [0; 1])
            .unwrap(),
        0
    );
    close(client, opened.handle);
    (opened, bytes)
}

fn mkdir(client: &Client, path: &str) {
    client
        .call(Request::Namespace {
            kind: NamespaceOperation::Mkdir,
            path: path.into(),
            destination: None,
            replace: false,
            operation_id: OperationId::generate().unwrap(),
        })
        .unwrap();
}

fn directory(client: &Client, path: &str) -> Vec<String> {
    let (opened, _fd) = open(client, path, false, false, false);
    let mut names = vec![];
    loop {
        let page: DirectoryPage = json_codec::from_value(
            client
                .call(Request::ReadDirectory {
                    handle: opened.handle,
                    restart: false,
                })
                .unwrap(),
        )
        .unwrap();
        names.extend(
            page.entries
                .iter()
                .map(|entry| entry.entry.path.as_str().to_owned()),
        );
        if !page.more {
            break;
        }
    }
    close(client, opened.handle);
    names
}

fn exported(engine: &Engine, root: &Path, version: &str) -> Vec<u8> {
    let output = root.join(format!("export-{version}"));
    engine.export_version(version, &output).unwrap();
    fs::read(output).unwrap()
}

#[test]
fn public_smb_bridge_read_to_new_file_keeps_independent_history_after_restart() {
    let mut fixture = Fixture::new();
    let client = fixture.connect();
    assert_eq!(directory(&client, ""), vec!["p"]);
    assert!(directory(&client, "p").is_empty());
    assert!(
        fixture
            .backend
            .repositories_snapshot(&fixture.identity())
            .unwrap()
            .is_empty()
    );
    mkdir(&client, "p/from");
    mkdir(&client, "p/to");
    let repo = fixture.repository();
    save(&client, "p/from/source.txt", b"first body longer", true);
    let original = fixture.entry(&repo.main_branch_id, "from/source.txt");
    save(&client, "p/from/source.txt", b"later", false);
    let source = fixture.entry(&repo.main_branch_id, "from/source.txt");
    assert_eq!(source.file_id, original.file_id);
    assert_ne!(source.version_id, original.version_id);
    fixture.annotate(&repo.repository_id, &source);
    let (_, bytes) = read(&client, "p/from/source.txt");
    assert_eq!(bytes, b"later");
    let created = save(&client, "p/to/new.txt", &bytes, true);
    let target = fixture.entry(&repo.main_branch_id, "to/new.txt");
    assert_eq!(target.entry_id, created.entry_id);
    assert_ne!(target.entry_id, source.entry_id);
    assert_ne!(target.file_id, source.file_id);
    assert_ne!(target.version_id, source.version_id);
    let history = fixture
        .backend
        .file_history_snapshot(
            &fixture.identity(),
            ReadFileHistory::parse(
                target.file_id.clone(),
                Some(repo.repository_id.clone()),
                Some(target.path().into()),
            )
            .unwrap(),
        )
        .unwrap();
    assert_eq!(history.versions.len(), 1);
    assert_eq!(history.nodes.len(), 1);
    assert_eq!(history.branches.len(), 1);
    assert!(history.merges.is_empty());
    assert!(history.note.is_empty());
    assert_eq!(read(&client, "p/to/new.txt").1, bytes);
    drop(client);
    let (root, engine) = fixture.cold_open();
    let source_versions = engine.file_history(&source.file_id).unwrap();
    assert_eq!(source_versions.len(), 2);
    assert_eq!(
        exported(&engine, root.path(), &original.version_id),
        b"first body longer"
    );
    assert_eq!(exported(&engine, root.path(), &source.version_id), b"later");
    let versions = engine.file_history(&target.file_id).unwrap();
    assert_eq!(versions.len(), 1);
    assert!(versions[0].source_version_ids.is_empty());
    assert!(
        engine
            .file_node_parents(&target.file_id, &target.version_id)
            .unwrap()
            .is_empty()
    );
    assert!(
        engine
            .state().container_tags[&target.file_id.parse::<storage_contract::FileId>().unwrap()]
            .tags()
            .is_empty()
    );
    assert!(!engine.state().file_notes.contains_key(&target.file_id));
    assert!(
        !engine
            .state()
            .version_notes
            .contains_key(&target.version_id)
    );
    assert_eq!(engine.state().file_notes[&source.file_id], "source note");
    assert_eq!(
        engine.state().version_notes[&source.version_id],
        "source note"
    );
    assert_eq!(
        engine.state().container_tags[
            &source.file_id.parse::<storage_contract::FileId>().unwrap()
        ].tags().iter().map(|tag| tag.as_str()).collect::<Vec<_>>(),
        ["source-only"]
    );
    assert_eq!(exported(&engine, root.path(), &target.version_id), b"later");
}

#[test]
fn public_smb_bridge_empty_create_close_and_flush_publish_once() {
    let mut fixture = Fixture::new();
    let client = fixture.connect();
    let mut saved = vec![];
    for (path, writable, flush) in [
        ("p/readonly-close.txt", false, false),
        ("p/readonly-flush.txt", false, true),
        ("p/close.txt", true, false),
        ("p/flush.txt", true, true),
    ] {
        let (opened, _fd) = open(&client, path, writable, true, false);
        assert_eq!(opened.size, 0);
        assert!(opened.workspace_id.is_some());
        assert!(opened.version_id.is_none());
        if flush {
            client
                .call(Request::Flush {
                    handle: opened.handle,
                })
                .unwrap();
        }
        close(&client, opened.handle);
        let (reader, bytes) = read(&client, path);
        assert_eq!(reader.entry_id, opened.entry_id);
        assert!(bytes.is_empty());
        saved.push(reader);
    }
    let repo = fixture.repository();
    drop(client);
    let (root, engine) = fixture.cold_open();
    let entries = engine.list_tree(&repo.main_branch_id).unwrap();
    assert_eq!(entries.len(), 4);
    assert_eq!(
        entries
            .iter()
            .map(|entry| &entry.file_id)
            .collect::<BTreeSet<_>>()
            .len(),
        4
    );
    assert_eq!(engine.state().versions.len(), 4);
    for opened in saved {
        let entry = entries
            .iter()
            .find(|entry| entry.entry_id == opened.entry_id)
            .unwrap();
        let versions = engine.file_history(&entry.file_id).unwrap();
        assert_eq!(versions.len(), 1);
        assert_eq!(Some(&versions[0].version_id), opened.version_id.as_ref());
        assert!(exported(&engine, root.path(), &entry.version_id).is_empty());
    }
}

#[test]
fn public_smb_bridge_exclusive_conflicts_preserve_content_and_connection() {
    let mut fixture = Fixture::new();
    let client = fixture.connect();
    save(&client, "p/Résumé.txt", b"keep", true);
    let repo = fixture.repository();
    let before = fixture.entry(&repo.main_branch_id, "Résumé.txt");
    let spool = fixture.root.path().join("management/smb-spool");
    let inventory = || {
        fs::read_dir(&spool)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect::<BTreeSet<_>>()
    };
    let before_spool = inventory();
    for path in ["p/Résumé.txt", "p/RÉSUMÉ.TXT", "p/Re\u{301}sume\u{301}.txt"] {
        let error = client
            .open(Request::Open {
                path: path.into(),
                writable: true,
                create: true,
                truncate: true,
                exclusive: true,
                write_through: false,
            })
            .unwrap_err();
        assert_eq!(
            error
                .get_ref()
                .unwrap()
                .downcast_ref::<RemoteError>()
                .unwrap()
                .code,
            RemoteErrorCode::Server(ErrorCode::NameConflict)
        );
        assert_eq!(fixture.entry(&repo.main_branch_id, "Résumé.txt"), before);
        assert_eq!(read(&client, path).1, b"keep");
        assert_eq!(inventory(), before_spool);
    }
    save(&client, "p/next.txt", b"next", true);
    drop(client);
    let (root, engine) = fixture.cold_open();
    assert_eq!(engine.file_history(&before.file_id).unwrap().len(), 1);
    assert_eq!(exported(&engine, root.path(), &before.version_id), b"keep");
}

#[test]
fn public_smb_bridge_read_only_create_existing_keeps_pinned_content_without_a_writer() {
    let mut fixture = Fixture::new();
    let client = fixture.connect();
    save(&client, "p/existing.txt", b"before", true);
    let repo = fixture.repository();
    let before = fixture.entry(&repo.main_branch_id, "existing.txt");
    let (reader, _placeholder) = client
        .open(Request::Open {
            path: "p/existing.txt".into(),
            writable: false,
            create: true,
            truncate: false,
            exclusive: false,
            write_through: false,
        })
        .unwrap();
    assert!(reader.workspace_id.is_none());
    assert_eq!(reader.entry_id, before.entry_id);
    assert_eq!(
        reader.version_id.as_deref(),
        Some(before.version_id.as_str())
    );
    // A read-only create of an existing file must not reserve its write path.
    save(&client, "p/existing.txt", b"after", false);
    let mut pinned = [0; 6];
    assert_eq!(client.read(reader.handle, 0, &mut pinned).unwrap(), 6);
    assert_eq!(&pinned, b"before");
    close(&client, reader.handle);
    assert_eq!(read(&client, "p/existing.txt").1, b"after");
    let current = fixture.entry(&repo.main_branch_id, "existing.txt");
    drop(client);
    let (root, engine) = fixture.cold_open();
    assert_eq!(engine.file_history(&before.file_id).unwrap().len(), 2);
    assert_eq!(
        exported(&engine, root.path(), &before.version_id),
        b"before"
    );
    assert_eq!(
        exported(&engine, root.path(), &current.version_id),
        b"after"
    );
}

#[path = "phase1_smb_rpc/http_soft_deletion.rs"]
mod http_soft_deletion;
#[path = "phase1_smb_rpc/soft_deletion.rs"]
mod soft_deletion;
