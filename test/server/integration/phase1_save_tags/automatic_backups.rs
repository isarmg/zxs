//! Management commands protect exact ordinary scopes on a real Add chain.
use super::*;
use std::{
    collections::BTreeSet,
    fs,
    os::unix::fs::{DirBuilderExt, MetadataExt, PermissionsExt},
    path::PathBuf,
};
use storage_contract::{CommitState, OperationId};
use storage_engine::{
    NamespaceOperation, ObjectDatabaseMaterial, ObjectDatabaseScope, PendingMemberDirectory,
    Repository, ValidatedConfig, inspect_object_database_material,
};

fn add_tier(engine: &mut Engine, disk: &storage_contract::DiskConfig) {
    assert_eq!(fs::read_dir(&disk.root).unwrap().count(), 0);
    let mut target = engine.config().clone();
    target.disks.push(disk.clone());
    let selected = || {
        vec![PendingMemberDirectory {
            disk_id: disk.disk_id.clone(),
            directory: fs::File::open(&disk.root).unwrap(),
        }]
    };
    engine
        .begin_membership_handoff_with_directories(
            target,
            "admin",
            OperationId::generate().unwrap(),
            selected(),
        )
        .unwrap();
    let archive = engine.build_managed_membership_control_archive().unwrap();
    engine
        .enroll_pending_members(&archive.archive_path, &archive.root_hash, selected())
        .unwrap();
    assert!(
        engine
            .resume_membership_joint_decision()
            .unwrap()
            .all_certificates_synchronized()
    );
    assert_eq!(
        engine.activate_pending_membership().unwrap().commit_state,
        CommitState::Published
    );
}

async fn added_chain() -> (Fixture, Config, Repository) {
    let root = tempfile::Builder::new()
        .permissions(fs::Permissions::from_mode(0o700))
        .tempdir()
        .unwrap();
    let system = root.path().join("system");
    let topology = Config::development(["A", "B", "C"].map(|name| {
        (
            name_policy::Name::parse(name).unwrap(),
            root.path().join(name),
        )
    }))
    .unwrap();
    for path in std::iter::once(&system).chain(topology.disks.iter().map(|disk| &disk.root)) {
        fs::DirBuilder::new().mode(0o700).create(path).unwrap();
    }
    let mut initial = topology.clone();
    initial.disks.truncate(1);
    let mut engine = Engine::initialize_held(
        &system,
        ValidatedConfig::try_from(initial).unwrap(),
        vec![fs::File::open(&topology.disks[0].root).unwrap()],
    )
    .unwrap();
    let repository = engine
        .create_disk_root("P01", "admin", OperationId::generate().unwrap())
        .unwrap();
    for path in ["docs", "docs/empty"] {
        engine
            .file_namespace(
                &repository.repository_id,
                NamespaceOperation::Mkdir {
                    path: name_policy::PortablePath::parse(path).unwrap(),
                },
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap();
    }
    for disk in &topology.disks[1..] {
        add_tier(&mut engine, disk);
    }
    let config = engine.config().clone();
    let auth = AuthStore::initialize(&system, "admin", "phase-one-test-password").unwrap();
    let app = router(engine, auth, &system);
    let login = request(
        &app,
        None,
        "POST",
        "/api/auth/login",
        Some(json!({
            "username": "admin", "password": "phase-one-test-password",
        })),
    )
    .await;
    let token = login["token"].as_str().unwrap().to_owned();
    (Fixture { root, app, token }, config, repository)
}

fn area(config: &Config, tier: usize) -> PathBuf {
    let mut path = config.disks[tier].root.clone();
    for source in config.disks[..tier].iter().rev() {
        let uuid = source.disk_id.strip_prefix("disk_").unwrap();
        path.push(format!("{}--{uuid}", source.name));
    }
    path.join("common-data")
}

fn assert_local_copies(config: &Config, repository: &Repository, entry: &Value, bytes: &[u8]) {
    let file = entry["file_id"].as_str().unwrap();
    let version = entry["version_id"].as_str().unwrap();
    let name = entry["name"].as_str().unwrap();
    let scope = ObjectDatabaseScope::File {
        file_id: file.into(),
        history_tree_id: repository.history_tree_id.clone(),
    };
    let mut materials: Option<ObjectDatabaseMaterial> = None;
    let mut bodies = BTreeSet::new();
    for tier in 0..3 {
        let container = area(config, tier).join("docs").join(name);
        let material = inspect_object_database_material(
            fs::File::open(&container).unwrap(),
            container.clone(),
            &scope,
        )
        .unwrap();
        let row = material
            .records()
            .iter()
            .find(|row| row.kind() == "version" && row.id() == version)
            .unwrap();
        let record: Value = json_codec::from_str(row.body_json()).unwrap();
        assert_eq!(record["version"]["file_id"], file);
        assert_eq!(record["version"]["version_id"], version);
        let storage = record["version"]["storage_id"].as_str().unwrap();
        let body = container.join("versions").join(format!("{storage}-{name}"));
        let identity = fs::metadata(&body).unwrap();
        assert!(bodies.insert((identity.dev(), identity.ino())));
        assert_eq!(fs::read(body).unwrap(), bytes);
        match &materials {
            Some(expected) => {
                assert_eq!(material.scope(), expected.scope());
                assert_eq!(material.records_hash(), expected.records_hash());
                assert_eq!(material.records(), expected.records());
            }
            None => materials = Some(material),
        }
    }
}

async fn stop_writers(fixture: Fixture) -> tempfile::TempDir {
    let Fixture {
        root,
        app,
        token: _,
    } = fixture;
    drop(app);
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        match Engine::open(&root.path().join("system")) {
            Ok(engine) => {
                drop(engine);
                return root;
            }
            Err(error) if error.code() == storage_contract::ErrorCode::WriterAlreadyActive => {
                let remaining = deadline.saturating_duration_since(std::time::Instant::now());
                assert!(!remaining.is_zero(), "router retained writer lock: {error}");
                tokio::time::sleep(remaining.min(Duration::from_millis(10))).await;
            }
            Err(error) => panic!("cold writer release failed: {error:?}"),
        }
    }
}

async fn rejected_scope(fixture: &Fixture, endpoint: &str, original: &Value) {
    let mut wrong = original.clone();
    wrong["directory_id"] = new_id("entry").unwrap().into();
    let response = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(endpoint)
                .header("authorization", format!("Bearer {}", fixture.token))
                .header("content-type", "application/json")
                .body(Body::from(json_codec::to_vec(&wrong).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let body = http_routing::body::to_bytes(response.into_body(), 8192)
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "{}",
        String::from_utf8_lossy(&body)
    );
    let error: Value = json_codec::from_slice(&body).unwrap();
    assert_eq!(error["error"]["code"], "SCOPE_CHANGED");
}

async fn native_listing(fixture: &Fixture, slot: &str, path: &str) -> Value {
    let route = format!(
        "/api/file-manager/disks/{slot}/entries?{}",
        path_query(path)
    );
    let tree = fixture.json("GET", &route, None).await;
    assert_eq!(tree["backup"], true);
    assert_eq!(tree["writable"], false);
    assert!(tree["context"].is_null());
    for row in tree["entries"].as_array().unwrap() {
        assert_eq!(row["backup"], true);
        assert_eq!(row["read_only"], true);
    }
    tree
}

async fn assert_native_listing(
    fixture: &Fixture,
    config: &Config,
    files: &[&Value],
    docs: &Value,
    empty: &Value,
) {
    let disks = fixture.json("GET", "/api/file-manager/disks", None).await;
    for tier in 1..3 {
        let disk = &config.disks[tier];
        let row = disks["disks"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["slot_id"] == disk.slot_id)
            .unwrap();
        assert_eq!(row["level"], disk.level);
        assert_eq!(row["writable"], false);
        let mut path = String::new();
        for source in config.disks[..tier].iter().rev() {
            let tree = native_listing(fixture, &disk.slot_id, &path).await;
            let uuid = source.disk_id.strip_prefix("disk_").unwrap();
            let component = format!("{}--{uuid}", source.name);
            let entries = tree["entries"].as_array().unwrap();
            assert_eq!(entries.len(), 1);
            assert_eq!(entries[0]["name"], component);
            assert_eq!(entries[0]["kind"], "directory");
            if !path.is_empty() {
                path.push('/');
            }
            path.push_str(&component);
            assert_eq!(entries[0]["path"], path);
        }
        let tree = native_listing(fixture, &disk.slot_id, &path).await;
        let directory = tree["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|row| row["name"] == "docs")
            .unwrap();
        assert_eq!(directory["entry_id"], docs["entry_id"]);
        let path = format!("{path}/docs");
        let tree = native_listing(fixture, &disk.slot_id, &path).await;
        let entries = tree["entries"].as_array().unwrap();
        let directory = entries.iter().find(|row| row["name"] == "empty").unwrap();
        assert_eq!(directory["entry_id"], empty["entry_id"]);
        assert_eq!(directory["kind"], "directory");
        for file in files {
            let row = entries
                .iter()
                .find(|row| row["file_id"] == file["file_id"])
                .unwrap();
            assert_eq!(row["entry_id"], file["entry_id"]);
            assert_eq!(row["version_id"], file["version_id"]);
            assert_eq!(
                row["path"],
                format!("{path}/{}", file["name"].as_str().unwrap())
            );
            assert_eq!(
                row["repository_path"],
                format!("docs/{}", file["name"].as_str().unwrap())
            );
        }
        assert_eq!(entries.len(), files.len() + 1);
        let tree = native_listing(fixture, &disk.slot_id, &format!("{path}/empty")).await;
        assert!(tree["entries"].as_array().unwrap().is_empty());
    }
}

#[tokio::test]
async fn automatic_directory_http_backfills_and_inherits_after_real_add_chain() {
    let (fixture, config, repository) = added_chain().await;
    let before_bytes = b"existing file before automatic directory configuration";
    fixture.save("P01", "docs/before.txt", before_bytes).await;
    let before = fixture.entry("P01", "docs/before.txt").await;
    let docs = fixture.entry("P01", "docs").await;
    let empty = fixture.entry("P01", "docs/empty").await;
    let read = format!(
        "/api/repositories/{}/backup-configuration?path=docs",
        repository.repository_id,
    );
    let configuration = fixture.json("GET", &read, None).await;
    assert_eq!(configuration["scope"]["kind"], "directory");
    assert_eq!(configuration["generation"], "0");
    assert_eq!(
        configuration["backup_routes"],
        json!([["P01", "B01", "C01"]])
    );
    let endpoint = format!(
        "/api/repositories/{}/automatic-directory-backup-configuration",
        repository.repository_id,
    );
    let original = json!({
        "path": "docs",
        "directory_id": configuration["scope"]["id"],
        "target_copies": 3,
        "expected_generation": configuration["generation"],
        "operation_id": new_id("op").unwrap(),
    });
    rejected_scope(&fixture, &endpoint, &original).await;
    assert_eq!(fixture.json("GET", &read, None).await, configuration);
    let receipt = fixture.json("PUT", &endpoint, Some(original.clone())).await;
    assert_eq!(
        receipt["operation_kind"],
        "backup.configure-automatic-directory"
    );
    assert_eq!(receipt["commit_state"], "PUBLISHED");
    assert_eq!(receipt["protection_at_publish"], "TARGET_MET");
    assert_eq!(
        storage_contract::client_protocol::operation_status(&receipt),
        Some(storage_contract::client_protocol::OperationStatus::Published),
    );
    assert_eq!(receipt["result"]["version_count"], "1");
    assert_eq!(
        receipt["result"]["automatic_request"]["directory_id"],
        configuration["scope"]["id"]
    );
    assert_eq!(receipt["result"]["backup_prefixes"], json!({}));

    let after_bytes = b"a new file inherits the directory's three real copies";
    let saved = fixture.save("P01", "docs/after.txt", after_bytes).await;
    assert_eq!(saved["result"]["protection_at_publish"], "TARGET_MET");
    let after = fixture.entry("P01", "docs/after.txt").await;
    assert_ne!(before["file_id"], after["file_id"]);
    assert_eq!(fixture.json("GET", &read, None).await["target_copies"], 3);
    for tier in 0..3 {
        assert!(area(&config, tier).join("docs/empty").is_dir());
    }
    assert_native_listing(&fixture, &config, &[&before, &after], &docs, &empty).await;
    let fixture = fixture.reopen().await;
    assert_native_listing(&fixture, &config, &[&before, &after], &docs, &empty).await;
    assert_eq!(
        fixture.json("PUT", &endpoint, Some(original)).await,
        receipt
    );
    let operation = receipt["operation_id"].as_str().unwrap();
    assert_eq!(
        fixture
            .json("GET", &format!("/api/operations/{operation}/outcome"), None)
            .await,
        receipt,
    );
    let queried = fixture
        .json("GET", &format!("/api/operations/{operation}"), None)
        .await;
    for field in [
        "operation_id",
        "operation_kind",
        "request_digest",
        "publication_sequence",
        "result",
    ] {
        assert_eq!(queried[field], receipt[field], "{field}");
    }
    for (entry, bytes) in [
        (&before, before_bytes.as_slice()),
        (&after, after_bytes.as_slice()),
    ] {
        let file = entry["file_id"].as_str().unwrap();
        let history = fixture.history(file).await;
        assert_eq!(current_node(&history)["version_id"], entry["version_id"]);
        assert_eq!(
            fixture
                .versions(file, &history)
                .await
                .as_array()
                .unwrap()
                .len(),
            1
        );
        assert_eq!(
            fixture.body(entry["version_id"].as_str().unwrap()).await,
            bytes
        );
    }
    // The standalone FILE inspection contract requires all actual writers to
    // stop. Hold the temporary roots alive, but no Router or Engine, below.
    let _root = stop_writers(fixture).await;
    assert_local_copies(&config, &repository, &before, before_bytes);
    assert_local_copies(&config, &repository, &after, after_bytes);
}

async fn rejected_file_request(fixture: &Fixture, endpoint: &str, body: Value, code: &str) {
    let response = fixture
        .app
        .clone()
        .oneshot(
            Request::builder()
                .method("PUT")
                .uri(endpoint)
                .header("authorization", format!("Bearer {}", fixture.token))
                .header("content-type", "application/json")
                .body(Body::from(json_codec::to_vec(&body).unwrap()))
                .unwrap(),
        )
        .await
        .unwrap();
    let status = response.status();
    let bytes = http_routing::body::to_bytes(response.into_body(), 8192)
        .await
        .unwrap();
    assert_eq!(
        status,
        StatusCode::CONFLICT,
        "{}",
        String::from_utf8_lossy(&bytes)
    );
    let error: Value = json_codec::from_slice(&bytes).unwrap();
    assert_eq!(error["error"]["code"], code);
}

#[tokio::test]
async fn automatic_file_http_pins_occurrence_and_replays_after_real_add_chain() {
    let (fixture, config, repository) = added_chain().await;
    let bytes = b"one exact file becomes independently readable on A B and C";
    let peer_bytes = b"the neighboring file keeps its own one-copy policy";
    fixture.save("P01", "docs/report.txt", bytes).await;
    fixture.save("P01", "docs/peer.txt", peer_bytes).await;
    let entry = fixture.entry("P01", "docs/report.txt").await;
    let peer = fixture.entry("P01", "docs/peer.txt").await;
    let read = format!(
        "/api/repositories/{}/backup-configuration?path=docs/report.txt",
        repository.repository_id,
    );
    let before = fixture.json("GET", &read, None).await;
    assert_eq!(before["target_copies"], 1);
    assert_eq!(before["scope"]["id"], entry["file_id"]);
    let endpoint = format!(
        "/api/repositories/{}/automatic-file-backup-configuration",
        repository.repository_id,
    );
    let original = json!({
        "path": "docs/report.txt",
        "entry_id": entry["entry_id"],
        "file_id": entry["file_id"],
        "target_copies": 3,
        "expected_generation": before["generation"],
        "operation_id": new_id("op").unwrap(),
    });
    let mut wrong_entry = original.clone();
    wrong_entry["entry_id"] = peer["entry_id"].clone();
    rejected_file_request(&fixture, &endpoint, wrong_entry.clone(), "SCOPE_CHANGED").await;
    assert_eq!(fixture.json("GET", &read, None).await, before);

    let receipt = fixture.json("PUT", &endpoint, Some(original.clone())).await;
    assert_eq!(receipt["operation_kind"], "backup.configure-automatic-file");
    assert_eq!(receipt["commit_state"], "PUBLISHED");
    assert_eq!(receipt["protection_at_publish"], "TARGET_MET");
    assert_eq!(receipt["result"]["version_count"], "1");
    let mut encoded = original.clone();
    encoded.as_object_mut().unwrap().remove("operation_id");
    encoded["repository_id"] = repository.repository_id.clone().into();
    assert_eq!(receipt["result"]["automatic_request"], encoded);
    let configured = fixture.json("GET", &read, None).await;
    assert_eq!(configured["target_copies"], 3);
    for path in ["docs", "docs/peer.txt"] {
        let read = format!(
            "/api/repositories/{}/backup-configuration?{}",
            repository.repository_id,
            path_query(path)
        );
        assert_eq!(fixture.json("GET", &read, None).await["target_copies"], 1);
    }
    // Once published, even a same-path/same-FileId request with another EntryId
    // is a different native request. It must not receive the original receipt.
    rejected_file_request(&fixture, &endpoint, wrong_entry, "IDEMPOTENCY_KEY_REUSED").await;
    let mut wrong_file = original.clone();
    wrong_file["file_id"] = peer["file_id"].clone();
    rejected_file_request(&fixture, &endpoint, wrong_file, "IDEMPOTENCY_KEY_REUSED").await;
    assert_eq!(fixture.json("GET", &read, None).await, configured);

    let fixture = fixture.reopen().await;
    assert_eq!(
        fixture.json("PUT", &endpoint, Some(original)).await,
        receipt
    );
    let operation = receipt["operation_id"].as_str().unwrap();
    assert_eq!(
        fixture
            .json("GET", &format!("/api/operations/{operation}/outcome"), None)
            .await,
        receipt
    );
    let file = entry["file_id"].as_str().unwrap();
    let history = fixture.history(file).await;
    assert_eq!(current_node(&history)["version_id"], entry["version_id"]);
    assert_eq!(
        fixture
            .versions(file, &history)
            .await
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        fixture.body(entry["version_id"].as_str().unwrap()).await,
        bytes
    );
    assert_eq!(
        fixture.body(peer["version_id"].as_str().unwrap()).await,
        peer_bytes
    );
    let _root = stop_writers(fixture).await;
    assert_local_copies(&config, &repository, &entry, bytes);
    for tier in 1..3 {
        assert!(!area(&config, tier).join("docs/peer.txt").exists());
    }
}
