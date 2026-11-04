//! Explicit external-client fixture: real engine, authority and Unix RPC.
use super::*;
use std::time::Duration;

const PAYLOAD: &[u8] = b"ZXS Debian system Samba real engine payload\n";

async fn marker(root: &Path, name: &str, server: &tokio::task::JoinHandle<io::Result<()>>) {
    tokio::time::timeout(Duration::from_secs(120), async {
        loop {
            assert!(!server.is_finished(), "RPC listener ended before {name}");
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
        .find(|entry| entry.path == path)
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
    binding_tests::rebind(&shared, &branch, true);
    let repository = repository(&shared);
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
    let web_server = tokio::spawn(async move { http_routing::serve(listener, web).await });
    let auth = temp.auth.clone();
    let backend = shared.backend.clone();
    let server = tokio::spawn(async move {
        let mut shutdown = ConnectionShutdown::new();
        let result = serve_owned(config, auth, backend, 4096, shutdown.registry(), account).await;
        shutdown.finish().await?;
        result
    });
    tokio::time::timeout(Duration::from_secs(10), async {
        while !socket.exists() {
            assert!(!server.is_finished(), "RPC startup failed");
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
        "branch_id": branch,
        "payload": std::str::from_utf8(PAYLOAD).unwrap(),
    });
    platform_io::fs_safety::AtomicFile::create(
        &directory,
        &platform_io::fs_safety::EntryName::new("ready.json").unwrap(),
        &json_codec::to_vec(&ready).unwrap(),
    )
    .unwrap();
    marker(&root, "uploaded", &server).await;
    verify_body(&shared, &branch, "kept.txt", PAYLOAD);
    verify_body(&shared, &branch, "report.txt", b"old");
    binding_tests::rebind(&shared, &branch, false);
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
    marker(&root, "denied", &server).await;
    crate::lock(&temp.auth).replace_authority(AuthStore::open(&temp.path().join("auth")).unwrap());
    binding_tests::rebind(&shared, &branch, true);
    publish_marker(&directory, "enabled");
    marker(&root, "done", &server).await;
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
    web_server.abort();
    assert!(web_server.await.unwrap_err().is_cancelled());
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
    let receipt = json!({"real_engine":true,"published_upload_and_rename_verified":true,"original_body_preserved":true,"old_tree_authority_revoked_then_restored":true,"new_tree_binding_disabled_then_restored":true,"published_delete_verified":true});
    platform_io::fs_safety::AtomicFile::create(
        &directory,
        &platform_io::fs_safety::EntryName::new("verified.json").unwrap(),
        &json_codec::to_vec(&receipt).unwrap(),
    )
    .unwrap();
}
