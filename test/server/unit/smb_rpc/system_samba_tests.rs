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
    let (temp, shared, _backend, branch) = fixture();
    binding_tests::rebind(&shared, &branch, true);
    let repository = repository(&shared);
    let socket = temp.path().join("rpc/smb.sock");
    let config = crate::SmbConfig {
        socket: socket.clone(),
        spool_dir: temp.path().join("system-samba-spool"),
        allowed_uid: smb_protocol::protocol::effective_uid(),
        username,
        spool_limits: crate::SpoolLimits {
            bytes_per_account: 16 * 1024 * 1024,
            bytes_total: 64 * 1024 * 1024,
            recovery_reserve_bytes: 1,
        },
    };
    let auth = temp.auth.clone();
    let backend = shared.backend.clone();
    let server = tokio::spawn(async move { serve(config, auth, backend, 4096).await });
    tokio::time::timeout(Duration::from_secs(10), async {
        while !socket.exists() {
            assert!(!server.is_finished(), "RPC startup failed");
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let ready = json!({"socket":socket,"repository_id":repository,"branch_id":branch,"payload":std::str::from_utf8(PAYLOAD).unwrap()});
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
    publish_marker(&directory, "revoked");
    marker(&root, "denied", &server).await;
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
    server.abort();
    assert!(server.await.unwrap_err().is_cancelled());
    let receipt = json!({"real_engine":true,"published_upload_and_rename_verified":true,"original_body_preserved":true,"binding_revoked_then_restored":true,"published_delete_verified":true});
    platform_io::fs_safety::AtomicFile::create(
        &directory,
        &platform_io::fs_safety::EntryName::new("verified.json").unwrap(),
        &json_codec::to_vec(&receipt).unwrap(),
    )
    .unwrap();
}
