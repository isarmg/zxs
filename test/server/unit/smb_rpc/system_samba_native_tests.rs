//! Driver-owned real smbd smoke. Each cold phase is a new Engine process.
use crate::auth::AuthStore;
use crate::engine_backend::EngineBackend;
use crate::smb_rpc::{ConnectionShutdown, serve_owned};
use crate::{ManagementRuntime, ServerConfig, SmbConfig, SpoolLimits};
use json_codec::json;
use name_policy::Name;
use platform_io::fs_safety::{AtomicFile, EntryName, PrivateDirectory};
use std::{fs, io::Read, path::PathBuf, sync::Arc, time::Duration};
use storage_contract::file_draft::DraftLocation;
use storage_engine::{Config, Engine, NativeCurrentEntry};

fn inspect(engine: &Engine, phase: u8) -> Option<DraftLocation> {
    if phase == 0 {
        assert!(engine.state().repositories.is_empty());
        assert!(engine.state().versions.is_empty());
        return None;
    }
    let repos: Vec<_> = engine.state().repositories.values()
        .filter(|repo| repo.file_manager_root)
        .collect();
    assert_eq!(repos.len(), 1);
    let entries = engine.native_current_entries(repos[0].repository_id.parse().unwrap()).unwrap();
    assert_eq!(entries.len(), 1);
    match (&entries[0], phase) {
        (NativeCurrentEntry::Unborn { target, working }, 1) => {
            assert_eq!(target.path.as_str(), "native-first.txt");
            assert_eq!(working.mode(), storage_contract::file_draft::CommitMode::Manual);
            assert_eq!(working.logical_size(), 3);
            assert!(engine.state().versions.is_empty());
            assert!(engine.state().branches.values().all(|branch| branch.file_id.is_none()));
            assert!(engine.state().operations.values().all(|op| op.kind != "workspace.commit"));
            let held = engine.open_native_working(target, &"webadmin".parse().unwrap(), false)
                .unwrap();
            let mut bytes = [0; 4];
            assert_eq!(engine.read_native_working(&held, &mut bytes, 0).unwrap(), 3);
            assert_eq!(&bytes[..3], b"xyz");
            Some(target.clone())
        }
        (NativeCurrentEntry::Published(entry), 2) => {
            assert_eq!(entry.path.as_str(), "native-first.txt");
            assert_eq!(engine.state().versions.len(), 1);
            assert!(engine.state().file_path_nodes.is_empty());
            assert!(engine.state().file_metadata_nodes.is_empty());
            let file_branches: Vec<_> = engine.state().branches.values()
                .filter(|branch| branch.file_id.is_some()).collect();
            assert_eq!(file_branches.len(), 1);
            let head = engine.file_branch_head(&file_branches[0].branch_id).unwrap();
            assert_eq!(head, entry.version_id);
            assert!(engine.file_node_parents(&entry.file_id, &head).unwrap().is_empty());
            assert_eq!(engine.state().operations.values()
                .filter(|op| op.kind == "file.working.first_commit").count(), 1);
            let mut bytes = Vec::new();
            engine.open_content(&entry.version_id).unwrap().file.take(4)
                .read_to_end(&mut bytes).unwrap();
            assert_eq!(bytes, b"xyz");
            Some(DraftLocation {
                repository: repos[0].repository_id.parse().unwrap(),
                path: entry.path.clone(),
                file: entry.file_id.parse().unwrap(),
                entry: entry.entry_id.parse().unwrap(),
            })
        }
        _ => panic!("cold phase did not preserve the required working/history state"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires isolated exact Samba runtime and real smbclient native driver"]
async fn system_samba_native_working_fixture() {
    let root = PathBuf::from(std::env::var_os("ZXS_SMB_E2E_DIR")
        .expect("driver must select its private empty test directory"));
    let directory = PrivateDirectory::open_trusted(&root).unwrap();
    let phase: u8 = std::env::var("ZXS_SMB_NATIVE_PHASE").unwrap().parse().unwrap();
    assert!(phase <= 2);
    let username = std::env::var("ZXS_SMB_E2E_USER").unwrap();
    smb_protocol::protocol::validate_username(&username).unwrap();
    let password = std::env::var("ZXS_SMB_NATIVE_WEB_PASSWORD").unwrap();
    let engine_uid = smb_protocol::protocol::effective_uid();
    assert_ne!(engine_uid, 0, "engine runs as the isolated non-root service account");
    let system = root.join("system");
    let engine = if phase == 0 {
        assert!(fs::read_dir(&root).unwrap().next().is_none(), "initial test root is not empty");
        let config = Config::development([(
            Name::parse("p").unwrap(), root.join("mounted-primary"),
        )]).unwrap();
        Engine::initialize(&system, config).unwrap()
    } else {
        Engine::open(&system).unwrap()
    };
    let target = inspect(&engine, phase);
    let slot = engine.config().disks[0].slot_id.clone();
    let backend = Arc::new(EngineBackend::new(engine));
    let auth = if phase == 0 {
        AuthStore::initialize(&system, "webadmin", &password).unwrap()
    } else {
        AuthStore::open(&system).unwrap()
    };
    let mut auth = backend.protect_auth(auth).unwrap();
    backend.set_management_auth(auth.reader()).unwrap();
    let token = auth.login("webadmin", &password).unwrap().token;
    let socket = root.join("rpc/smb.sock");
    let smb = SmbConfig {
        socket: socket.clone(),
        spool_dir: root.join("smb-spool"),
        allowed_uid: 0,
        username,
        additional_users: Vec::new(),
        spool_limits: SpoolLimits {
            bytes_per_account: 16 * 1024 * 1024,
            bytes_total: 64 * 1024 * 1024,
            recovery_reserve_bytes: 1,
        },
    };
    let mut config = ServerConfig::new(&root.join("management"));
    config.smb = Some(smb.clone());
    let config = config.admit().unwrap();
    let runtime = ManagementRuntime::start(backend, auth, &config).unwrap();
    let rpc_backend = Arc::clone(&runtime.backend);
    let rpc_auth = Arc::clone(&runtime.auth);
    let account = runtime.smb_account.clone().unwrap();
    let app = crate::router_shared(
        runtime.backend,
        runtime.auth,
        config.clone(),
        runtime.telemetry,
        crate::http_spool::HttpSpool::open_blocking(&config),
        runtime.smb_account,
    ).unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let http = tokio::spawn(async move { http_routing::serve(listener, app).await });
    let rpc = tokio::spawn(async move {
        let mut shutdown = ConnectionShutdown::new();
        let result = serve_owned(
            smb, rpc_auth, rpc_backend, 4096, shutdown.registry(), account,
        ).await;
        shutdown.finish().await?;
        result
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        while !socket.exists() {
            assert!(!rpc.is_finished(), "RPC startup failed");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }).await.unwrap();
    let cold_state = match phase {
        0 => "empty",
        1 => "unborn",
        2 => "one_history_node",
        _ => panic!("driver phase was outside its three-phase contract"),
    };
    let ready = json!({
        "engine_uid": engine_uid,
        "allowed_peer_uid": 0,
        "socket": socket,
        "password_http_port": port,
        "password_http_token": token,
        "default_disk_name": "p",
        "slot_id": slot,
        "phase": phase,
        "target": target,
        "cold_state": cold_state,
    });
    AtomicFile::create(
        &directory,
        &EntryName::new(format!("native-ready-{phase}.json")).unwrap(),
        &json_codec::to_vec(&ready).unwrap(),
    ).unwrap();
    tokio::time::timeout(Duration::from_secs(180), async {
        loop {
            assert!(!rpc.is_finished() && !http.is_finished(), "service ended during client I/O");
            match fs::read(root.join(format!("native-stop-{phase}"))) {
                Ok(bytes) => {
                    assert_eq!(bytes, b"ok\n");
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => panic!("native driver marker: {error}"),
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    }).await.unwrap();
    http.abort();
    assert!(http.await.unwrap_err().is_cancelled());
    rpc.abort();
    assert!(rpc.await.unwrap_err().is_cancelled());
    AtomicFile::create(
        &directory,
        &EntryName::new(format!("native-stopped-{phase}")).unwrap(),
        b"ok\n",
    ).unwrap();
}
