//! Socket-free application test of the production dispatcher and real EngineBackend.
//! Transport peer authentication, Unix framing/FD transfer and Samba are separate tests.
use crate::auth::AuthStore;
use crate::engine_backend::EngineBackend;
use crate::smb_rpc::default_share::DefaultShare;
use crate::smb_rpc::{MAX_AUTHENTICATED_SESSIONS, Reply, Shared, budget};
use crate::{Backend, SmbConfig, SpoolLimits};
use json_codec::Value;
use smb_protocol::protocol::{DirectoryEntry, OpenIo, OpenResult, Request, WorkingPosition};
use std::{
    collections::BTreeSet,
    fs::{self, File},
    os::unix::fs::{FileExt, PermissionsExt},
    path::Path,
    sync::{Arc, Mutex},
};
use storage_contract::file_draft::DraftLocation;
use storage_engine::{Config, Engine};

#[path = "incremental_manual.rs"]
mod incremental_manual;

struct Direct {
    backend: Arc<EngineBackend>,
    share: DefaultShare,
    // AuthReader is weak: retain the real store and its lock until dispatch ends.
    _auth: AuthStore,
}

impl Direct {
    fn start(root: &Path, engine: Engine, auth: AuthStore) -> Self {
        Self::start_with_limit(root, engine, auth, 4096)
    }

    fn start_with_limit(root: &Path, engine: Engine, auth: AuthStore, limit: u64) -> Self {
        let backend = Arc::new(EngineBackend::new(engine));
        let auth = backend.protect_auth(auth).unwrap();
        backend.set_management_auth(auth.reader()).unwrap();
        let identity = auth.sole_identity().unwrap();
        let spool = root.join("spool");
        let config = SmbConfig {
            socket: root.join("unused.sock"),
            spool_dir: spool.clone(),
            allowed_uid: rustix::process::geteuid().as_raw(),
            username: identity.principal.clone(),
            additional_users: Vec::new(),
            spool_limits: SpoolLimits {
                bytes_per_account: 16 * 1024 * 1024,
                bytes_total: 64 * 1024 * 1024,
                recovery_reserve_bytes: 1,
            },
        };
        let account = crate::smb_account_binding::Authority::open(&config).unwrap();
        let budget = budget::Budget::recover(&spool, config.spool_limits).unwrap();
        let max_file_bytes = backend.capabilities().unwrap().max_upload_bytes().min(limit);
        let account_generation = account.read().unwrap().selection().generation;
        let shared = Arc::new(Shared {
            backend: backend.clone(),
            account,
            auth: auth.reader(),
            spool,
            budget,
            max_file_bytes,
            authenticated_sessions: tokio::sync::Semaphore::new(MAX_AUTHENTICATED_SESSIONS),
            file_writers: Mutex::new(BTreeSet::new()),
            namespace_operations: Mutex::new(BTreeSet::new()),
        });
        let share =
            DefaultShare::new(shared, identity, account_generation, max_file_bytes).unwrap();
        Self { backend, share, _auth: auth }
    }

    fn call(&mut self, request: Request) -> Reply {
        self.share.dispatch(request).unwrap()
    }

    fn open(&mut self, path: &str, writable: bool, create: bool) -> (OpenResult, File) {
        let reply = self.call(Request::Open {
            path: path.into(),
            writable,
            create,
            truncate: false,
            exclusive: create,
            write_through: false,
        });
        let Reply::Open { metadata, descriptor } = reply else {
            panic!("Open returned a non-open reply: {reply:?}");
        };
        (metadata, File::from(descriptor))
    }

    fn read(&mut self, handle: u64) -> Vec<u8> {
        let reply = self.call(Request::Read { handle, offset: 0, length: 32 });
        let Reply::Read(bytes) = reply else {
            panic!("Read returned a non-content reply: {reply:?}");
        };
        bytes.to_vec()
    }

    fn write(&mut self, handle: u64, position: WorkingPosition, bytes: &[u8]) {
        let reply = self.call(Request::WorkingWrite { handle, position, bytes: bytes.to_vec() });
        let Reply::WorkingWrite(result) = reply else {
            panic!("WorkingWrite returned a non-write reply: {reply:?}");
        };
        assert_eq!(result.written as usize, bytes.len());
    }

    fn flush(&mut self, handle: u64) -> Value {
        let reply = self.call(Request::Flush { handle });
        let Reply::Json(result) = reply else {
            panic!("Flush returned a non-operation reply: {reply:?}");
        };
        result
    }

    fn listed(&mut self, expected_size: u64) -> DraftLocation {
        let (directory, placeholder) = self.open("p", false, false);
        assert_eq!(directory.data_io, OpenIo::Directory);
        assert!(placeholder.metadata().unwrap().is_dir());
        let reply = self.call(Request::ReadDirectory { handle: directory.handle, restart: true });
        let Reply::Directory(page) = reply else {
            panic!("ReadDirectory returned a non-directory reply: {reply:?}");
        };
        assert!(!page.more);
        assert_eq!(page.entries.len(), 1);
        let DirectoryEntry::Unborn { target, logical_size } = &page.entries[0] else {
            panic!("native working content must not manufacture a historical Entry");
        };
        assert_eq!(target.path.as_str(), "first.txt");
        assert_eq!(*logical_size, expected_size);
        let wire = json_codec::to_value(&page.entries[0]).unwrap();
        assert!(wire.get("version_id").is_none() && wire.get("entry").is_none());
        let target = target.clone();
        self.call(Request::Close { handle: directory.handle });
        target
    }
}

fn no_file_history(engine: &Engine) {
    assert!(engine.state().versions.is_empty());
    assert!(engine.state().branches.values().all(|branch| branch.file_id.is_none()));
    assert!(engine.state().operations.values().all(|result| result.kind != "workspace.commit"));
}

#[test]
fn native_dispatch_uses_real_engine_and_cold_unborn_content_without_transport() {
    let root = tempfile::tempdir().unwrap();
    let system = root.path().join("state");
    let engine = Engine::initialize(
        &system,
        Config::development([(
            name_policy::Name::parse("p").unwrap(),
            root.path().join("mounted-primary"),
        )]).unwrap(),
    ).unwrap();
    assert!(engine.state().repositories.is_empty());
    no_file_history(&engine);
    let mut entropy = [0; 32];
    system_entropy::fill(&mut entropy).unwrap();
    let password: String = entropy.iter().map(|byte| format!("{byte:02x}")).collect();
    let auth = AuthStore::initialize(&system, "alice", &password).unwrap();
    let spool = root.path().join("spool");
    fs::create_dir(&spool).unwrap();
    fs::set_permissions(&spool, fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir(spool.join("empty-directory")).unwrap();
    let mut direct = Direct::start(root.path(), engine, auth);

    let (opened, placeholder) = direct.open("p/first.txt", true, true);
    assert_eq!(opened.data_io, OpenIo::NativeWorking);
    assert!(opened.version_id.is_none() && opened.workspace_id.is_none());
    assert_eq!(opened.size, 0);
    assert!(placeholder.metadata().unwrap().is_dir());
    assert!(placeholder.write_all_at(b"not a mutable BODY descriptor", 0).is_err());
    let target = direct.listed(0);
    assert_eq!(target.entry.to_string(), opened.entry_id);
    assert!(direct.read(opened.handle).is_empty());

    direct.write(opened.handle, WorkingPosition::At { offset: 0 }, b"abcdef");
    direct.write(opened.handle, WorkingPosition::Append {}, b"GH");
    assert_eq!(direct.read(opened.handle), b"abcdefGH");
    direct.call(Request::WorkingResize { handle: opened.handle, length: 4 });
    direct.call(Request::WorkingResize { handle: opened.handle, length: 10 });
    direct.write(opened.handle, WorkingPosition::At { offset: 7 }, b"Z");
    let expected = b"abcd\0\0\0Z\0\0";
    assert_eq!(direct.read(opened.handle), expected);
    let Reply::WorkingStat(stat) = direct.call(Request::WorkingStat {
        handle: opened.handle,
    }) else {
        panic!("working stat did not return the actual editing size");
    };
    assert_eq!(stat.size, expected.len() as u64);
    // Successful frames do not publish the body: current listing/stat retain
    // the admitted empty generation until the real Flush publication.
    assert_eq!(direct.listed(0), target);
    let Reply::Stat(stat) = direct.call(Request::Stat { path: "p/first.txt".into() }) else {
        panic!("path stat did not return admitted metadata");
    };
    assert_eq!(stat.size, 0);
    assert!(direct.share.dispatch(Request::WorkingResize {
        handle: opened.handle, length: opened.max_file_bytes + 1,
    }).is_err());
    assert_eq!(direct.read(opened.handle), expected);
    let receipt = direct.flush(opened.handle);
    assert_eq!(direct.flush(opened.handle), receipt);
    assert_eq!(direct.listed(expected.len() as u64), target);

    // Close must publish a later editing generation, not merely discard the
    // opaque handle after the earlier successful Flush.
    direct.write(opened.handle, WorkingPosition::At { offset: 9 }, b"Q");
    direct.call(Request::Close { handle: opened.handle });
    let expected = b"abcd\0\0\0Z\0Q";
    drop(placeholder);
    drop(direct);
    let engine = Engine::open(&system).unwrap();
    no_file_history(&engine);
    let auth = AuthStore::open(&system).unwrap();
    let mut direct = Direct::start(root.path(), engine, auth);
    assert_eq!(direct.listed(expected.len() as u64), target);
    let (read, placeholder) = direct.open("p/first.txt", false, false);
    assert_eq!(read.data_io, OpenIo::NativeWorking);
    assert_eq!(read.entry_id, target.entry.to_string());
    assert!(read.version_id.is_none() && read.workspace_id.is_none());
    assert_eq!(direct.read(read.handle), expected);
    assert!(direct.share.dispatch(Request::WorkingWrite {
        handle: read.handle,
        position: WorkingPosition::At { offset: 0 },
        bytes: b"forbidden".to_vec(),
    }).is_err());
    assert!(direct.share.dispatch(Request::Open {
        path: "p/first.txt".into(), writable: true, create: true,
        truncate: false, exclusive: true, write_through: false,
    }).is_err());
    assert_eq!(direct.read(read.handle), expected);
    direct.call(Request::Close { handle: read.handle });
    drop(placeholder);
    drop(direct);
    let engine = Engine::open(&system).unwrap();
    no_file_history(&engine);
    assert_eq!(engine.native_current_entries(target.repository).unwrap().len(), 1);
}

#[path = "native_worktree_tests.rs"]
mod worktree;
