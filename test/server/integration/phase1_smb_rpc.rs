//! Phase-one local SMB bridge acceptance through its public listener and Client.
//! These are real framed Unix RPC/FD/Engine tests, not Samba network tests.
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

    fn annotate(&self, repository: &str, source: &Entry) -> String {
        let identity = self.identity();
        let history = self
            .backend
            .file_history_snapshot(
                &identity,
                ReadFileHistory::parse(
                    source.file_id.clone(),
                    Some(repository.into()),
                    Some(source.path().into()),
                )
                .unwrap(),
            )
            .unwrap();
        let node = history.current_node_id.unwrap();
        let result = self
            .backend
            .update_file_node_tags(
                &identity,
                storage_server::UpdateFileNodeTags {
                    file_id: source.file_id.clone(),
                    targets: vec![storage_contract::FileTagTarget {
                        branch_id: history.selected_branch_id.unwrap(),
                        source_node_id: node.clone(),
                        expected_node_id: node,
                    }],
                    add: vec!["source-only".into()],
                    remove: vec![],
                    location: Some(storage_contract::FileLocation {
                        repository_id: repository.into(),
                        path: source.path().into(),
                    }),
                    operation_id: OperationId::generate().unwrap(),
                },
            )
            .unwrap();
        for target in [
            storage_server::NoteTarget::File(source.file_id.clone()),
            storage_server::NoteTarget::Version(source.version_id.clone()),
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
        result.result["nodes"][0]["node_id"]
            .as_str()
            .unwrap()
            .to_owned()
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
    let tag_node = fixture.annotate(&repo.repository_id, &source);
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
            .file_node_tags(&target.file_id, &target.version_id)
            .unwrap()
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
        engine.file_node_tags(&source.file_id, &tag_node).unwrap(),
        BTreeSet::from(["source-only".into()])
    );
    assert_eq!(exported(&engine, root.path(), &target.version_id), b"later");
}

#[test]
fn public_smb_bridge_empty_create_close_and_flush_publish_once() {
    let mut fixture = Fixture::new();
    let client = fixture.connect();
    let mut saved = vec![];
    for (path, flush) in [("p/close.txt", false), ("p/flush.txt", true)] {
        let (opened, _fd) = open(&client, path, true, true, false);
        assert_eq!(opened.size, 0);
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
    assert_eq!(entries.len(), 2);
    assert_ne!(entries[0].file_id, entries[1].file_id);
    assert_eq!(engine.state().versions.len(), 2);
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
