//! Explicit external-client fixture: real engine, authority and Unix RPC.
use super::*;
use std::time::Duration;

const PAYLOAD: &[u8] = b"ZXS Debian system Samba real engine payload\n";

/// A test-private owner: an early transport failure reports the actual cause,
/// and the driver cannot publish a completion marker before all work drains.
pub(super) struct FixtureService {
    stop: tokio::sync::oneshot::Sender<()>,
    task: tokio::task::JoinHandle<io::Result<()>>,
}

impl FixtureService {
    pub(super) async fn require_running(&mut self, phase: &str) {
        if self.task.is_finished() {
            match (&mut self.task).await {
                Ok(Ok(())) => panic!("fixture service ended before {phase}"),
                Ok(Err(error)) => panic!("fixture service failed before {phase}: {error}"),
                Err(error) => panic!("fixture service task failed before {phase}: {error}"),
            }
        }
    }

    pub(super) async fn finish(self) {
        let requested = self.stop.send(());
        self.task.await.expect("fixture service panicked").expect("fixture service shutdown failed");
        requested.expect("service exited before its owner requested shutdown");
    }
}

/// Both harnesses require *both* services. Use the same stop boundary as the
/// service, but make an unexpected bridge exit fail this fixture, not just log
/// the optional-bridge error and keep HTTP running.
pub(super) fn start_fixture_service(
    listener: tokio::net::TcpListener,
    http: (http_routing::Router, crate::HttpWork),
    management: crate::service_lifecycle::ManagementShutdown,
    rpc: impl std::future::Future<Output = io::Result<()>> + Send + 'static,
) -> FixtureService {
    fn completed(first: io::Result<()>, second: io::Result<()>) -> io::Result<()> {
        match (first, second) {
            (Ok(()), result) | (result, Ok(())) => result,
            (Err(a), Err(b)) => Err(io::Error::other(format!("{a}; also: {b}"))),
        }
    }
    let (stop, stopped) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        let (app, work) = http;
        let http = |signal| async move {
            let served = crate::http_shutdown::serve(listener, app, signal).await;
            completed(served, work.drain().await)
        };
        let (report, failed) = tokio::sync::oneshot::channel();
        let bridge = async move {
            let error = match rpc.await {
                Ok(()) => io::Error::new(io::ErrorKind::UnexpectedEof, "required SMB listener ended"),
                Err(error) => error,
            };
            report.send(error).expect("required SMB result must reach its owner");
        };
        let mut early = None;
        let shutdown = async {
            tokio::select! {
                biased;
                failure = failed => early = Some(failure.expect("SMB failure reporter lost")),
                request = stopped => request.expect("fixture owner dropped before orderly stop"),
            }
        };
        let ended = crate::service_lifecycle::run_with_management(http, bridge, shutdown, management).await;
        completed(ended, early.map_or(Ok(()), Err))
    });
    FixtureService { stop, task }
}

async fn marker(root: &Path, name: &str, server: &mut FixtureService) {
    tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            server.require_running(name).await;
            match fs::read(root.join(name)) {
                Ok(bytes) => {
                    assert_eq!(bytes, b"ok\n", "invalid client phase marker");
                    return;
                }
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => panic!("cannot read client phase {name}: {error}"),
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    })
    .await
    .expect("system Samba client phase deadline exceeded");
}

fn publish_marker(directory: &platform_io::fs_safety::PrivateDirectory, name: &str) {
    platform_io::fs_safety::AtomicFile::create(
        directory,
        &platform_io::fs_safety::EntryName::new(name).unwrap(),
        b"ok\n",
    )
    .unwrap();
}

fn verify_body(shared: &Shared, branch: &str, path: &str, expected: &[u8]) {
    let tree = shared
        .backend
        .tree_snapshot(
            &identity(shared),
            crate::ReadTree {
                branch,
                commit: None,
            },
        )
        .unwrap();
    let entry = tree
        .entries
        .iter()
        .find(|entry| entry.path.as_str() == path)
        .expect("published file missing");
    let content = shared
        .backend
        .content(&identity(shared), &entry.version_id)
        .unwrap()
        .fully_verified()
        .unwrap();
    let mut bytes = Vec::new();
    content.file.take(4097).read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, expected, "authoritative published content differs");
}

#[test]
fn established_rpc_authority_revocation_precedes_disabled_binding() {
    let (temp, shared, _backend, branch) = fixture();
    binding_tests::rebind(&shared, &branch, true);
    let (mut client, worker, hello) = binding_tests::wire(shared.clone());
    assert!(matches!(hello, Response::Ok { .. }));
    binding_tests::rebind(&shared, &branch, false);
    crate::lock(&temp.auth).replace_authority(
        AuthStore::initialize(
            &temp.path().join("replacement-auth"),
            "bob",
            "long-test-password",
        )
        .unwrap(),
    );
    let (response, fd) = binding_tests::exchange(
        &mut client,
        Request::Stat {
            path: String::new(),
        },
    );
    assert!(fd.is_none());
    assert!(matches!(
        response,
        Response::Error {
            code: storage_contract::ErrorCode::Unauthorized,
            ..
        }
    ));
    drop(client);
    worker.join().unwrap().unwrap();
    crate::lock(&temp.auth).replace_authority(AuthStore::open(&temp.path().join("auth")).unwrap());
    binding_tests::rebind(&shared, &branch, true);
    let (mut client, worker, hello) = binding_tests::wire(shared);
    assert!(matches!(hello, Response::Ok { .. }));
    assert!(matches!(
        binding_tests::exchange(
            &mut client,
            Request::Stat {
                path: String::new()
            }
        )
        .0,
        Response::Ok { .. }
    ));
    drop(client);
    worker.join().unwrap().unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "requires isolated Debian system Samba and real smbclient driver"]
async fn system_samba_real_engine_fixture() {
    // This fixture runs alone in its own process. Retain the production RPC
    // boundary's complete local error source in engine.log, not the wire reply.
    let subscriber = tracing::collector::fmt()
        .without_time()
        .with_ansi(false)
        .with_max_level(tracing::Level::WARN)
        .with_writer(std::io::stderr)
        .finish();
    tracing::subscriber::set_global_default(subscriber).unwrap();
    let root = PathBuf::from(
        std::env::var_os("ZXS_SMB_E2E_DIR").expect("explicit isolated driver directory required"),
    );
    let directory = platform_io::fs_safety::PrivateDirectory::open_trusted(&root).unwrap();
    let username = std::env::var("ZXS_SMB_E2E_USER")
        .expect("explicit existing isolated Unix account required");
    smb_protocol::protocol::validate_username(&username).unwrap();
    let additional_username = std::env::var("ZXS_SMB_E2E_NEXT_USER")
        .expect("explicit second existing isolated Unix account required");
    smb_protocol::protocol::validate_username(&additional_username).unwrap();
    let engine_uid = smb_protocol::protocol::effective_uid();
    assert_ne!(
        engine_uid, 0,
        "real engine must run as its private service user"
    );
    let (temp, shared, _backend, branch) = fixture();
    let repository = repository(&shared);
    // Both share forms expose the same ordinary root. Revocation remains
    // bound to this exact registered root, not a share spelling.
    let rebind = |enabled| {
        let administrator = identity(&shared);
        let binding = shared
            .backend
            .smb_binding_status(&administrator, &repository)
            .unwrap();
        shared
            .backend
            .update_smb_binding(
                &administrator,
                crate::UpdateSmbBinding {
                    repository: &repository,
                    branch: &branch,
                    enabled,
                    expected_history_generation: binding.history_generation,
                    operation: storage_contract::OperationId::generate().unwrap(),
                },
            )
            .unwrap();
    };
    rebind(true);
    let named = shared
        .backend
        .repository_snapshot(&identity(&shared), &repository)
        .unwrap();
    let primary = shared
        .backend
        .topology_snapshot(&identity(&shared))
        .unwrap()
        .slots
        .into_iter()
        .find(|slot| slot.slot_id == named.primary_slot_id)
        .expect("invariant: named repository primary slot is configured");
    let default_disk_name = primary.directory_name().unwrap().as_str();
    let (selected_file, default_main, default_draft) = file_selections(&shared, &branch);
    select_file(&shared, &selected_file, &default_draft);
    {
        let mut selected = session(shared.clone(), &branch);
        let (opened, file) = open_file(&mut selected, true);
        file.write_all_at(b"new", 0).unwrap();
        selected.dirty(opened.handle).unwrap();
        selected.close(opened.handle, false).unwrap();
    }
    select_file(&shared, &selected_file, &default_main);
    let socket = temp.path().join("rpc/smb.sock");
    let config = crate::SmbConfig {
        socket: socket.clone(),
        spool_dir: temp.path().join("system-samba-state/smb-spool"),
        // Samba connects before impersonating the independent authenticated user.
        allowed_uid: 0,
        username,
        additional_users: vec![additional_username],
        spool_limits: crate::SpoolLimits {
            bytes_per_account: 16 * 1024 * 1024,
            bytes_total: 64 * 1024 * 1024,
            recovery_reserve_bytes: 1,
        },
    };
    // Real HTTP password boundary under the same non-root service identity.
    // Credentials are synthetic and remain in this private temporary fixture.
    let mut web_auth = AuthStore::initialize(
        &temp.path().join("smb-web-auth"),
        "webadmin",
        "synthetic-web-test-password",
    )
    .unwrap();
    let web_token = web_auth
        .login("webadmin", "synthetic-web-test-password")
        .unwrap()
        .token;
    let mut web_config = crate::ServerConfig::new(&temp.path().join("smb-web-management"));
    web_config.smb = Some(config.clone());
    let web_config = web_config.admit().unwrap();
    let runtime =
        crate::ManagementRuntime::start(shared.backend.clone(), web_auth, &web_config).unwrap();
    let account = runtime.smb_account.clone().unwrap();
    let mut management = crate::service_lifecycle::ManagementShutdown::new(runtime.backend.clone());
    management.set_telemetry(runtime.telemetry.clone());
    let workers = management.smb_connections();
    let web = crate::router_shared(
        runtime.backend,
        runtime.auth,
        web_config.clone(),
        runtime.telemetry,
        crate::http_spool::HttpSpool::open_blocking(&web_config),
        runtime.smb_account,
    )
    .unwrap();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let web_port = listener.local_addr().unwrap().port();
    let auth = temp.auth.clone();
    let backend = shared.backend.clone();
    let mut server = start_fixture_service(
        listener, web, management, serve_owned(config, auth, backend, 4096, workers, account),
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        while !socket.exists() {
            server.require_running("RPC socket readiness").await;
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let ready = json!({
        "engine_uid": engine_uid,
        "password_http_port": web_port,
        "password_http_token": web_token,
        "allowed_peer_uid": 0,
        "socket": socket,
        "repository_id": repository,
        "default_disk_name": default_disk_name,
        "default_report_path": format!("{default_disk_name}/report.txt"),
        "branch_id": branch,
        "payload": std::str::from_utf8(PAYLOAD).unwrap(),
    });
    platform_io::fs_safety::AtomicFile::create(
        &directory,
        &platform_io::fs_safety::EntryName::new("ready.json").unwrap(),
        &json_codec::to_vec(&ready).unwrap(),
    )
    .unwrap();
    marker(&root, "default-switch", &mut server).await;
    select_file(&shared, &selected_file, &default_draft);
    publish_marker(&directory, "default-switched");
    marker(&root, "default-restore", &mut server).await;
    select_file(&shared, &selected_file, &default_main);
    publish_marker(&directory, "default-restored");
    marker(&root, "uploaded", &mut server).await;
    verify_body(&shared, &branch, "kept.txt", PAYLOAD);
    verify_body(&shared, &branch, "report.txt", b"old");
    let roots = shared.backend.repositories_snapshot(&identity(&shared)).unwrap();
    assert_eq!(roots.len(), 1, "both shares must resolve the same registered namespace");
    assert_eq!(roots[0].repository_id, repository);
    verify_body(&shared, &branch, "default-kept.txt", PAYLOAD);
    rebind(false);
    // Replace only this fixture's sole authority: an already-authenticated Alice
    // session must now return Unauthorized/EACCES independently of stale binding.
    crate::lock(&temp.auth).replace_authority(
        AuthStore::initialize(
            &temp.path().join("replacement-auth"),
            "bob",
            "long-test-password",
        )
        .unwrap(),
    );
    publish_marker(&directory, "revoked");
    marker(&root, "denied", &mut server).await;
    crate::lock(&temp.auth).replace_authority(AuthStore::open(&temp.path().join("auth")).unwrap());
    rebind(true);
    publish_marker(&directory, "enabled");
    marker(&root, "done", &mut server).await;
    let tree = shared
        .backend
        .tree_snapshot(
            &identity(&shared),
            crate::ReadTree {
                branch: &branch,
                commit: None,
            },
        )
        .unwrap();
    assert_eq!(
        tree.entries.len(),
        1,
        "client did not delete its published file"
    );
    verify_body(&shared, &branch, "report.txt", b"old");
    assert_eq!(tree.entries[0].path.as_str(), "report.txt",
        "neither share's uploaded FILE may survive its confirmed deletion");
    server.finish().await;
    let receipt = json!({"real_engine":true,"published_upload_and_rename_verified":true,"original_body_preserved":true,"old_tree_authority_revoked_then_restored":true,"new_tree_binding_disabled_then_restored":true,"published_delete_verified":true,"default_namespace_verified":true,"file_branch_switch_verified":true});
    platform_io::fs_safety::AtomicFile::create(
        &directory,
        &platform_io::fs_safety::EntryName::new("verified.json").unwrap(),
        &json_codec::to_vec(&receipt).unwrap(),
    )
    .unwrap();
}
