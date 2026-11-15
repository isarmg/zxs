//! Existing V1/V2 are reprotected by the ordinary prepared-container writer.
//! No caller prefix, new version identity, synthetic receipt, or backup promotion.
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::Path,
};
use storage_contract::{CommitState, DiskId, FileId, Generation, OperationId, Principal};
use storage_engine::{
    AutomaticBackupConfigurationRequest, CommitRequest, Config, Engine, FileVersion,
    ValidatedConfig,
};

fn save(engine: &mut Engine, branch: &str, bytes: &[u8]) -> FileVersion {
    let workspace = engine.create_workspace(branch, "admin").unwrap();
    engine.stage_bytes(&workspace.workspace_id, "one.txt", bytes).unwrap();
    let receipt = engine.commit_workspace(CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: OperationId::generate().unwrap(),
        principal: "admin".into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "real retained version before backfill".into(),
        request_digest: None,
    }).unwrap();
    assert_eq!(receipt.commit_state, CommitState::Published);
    let entries = engine.list_tree(branch).unwrap();
    assert_eq!(entries.len(), 1);
    engine.state().versions[&entries[0].version_id].clone()
}

fn rows(path: &Path) -> BTreeMap<(String, String), json_codec::Value> {
    use sqlite_access::{Connection, Row, SqliteConnection};
    use sqlite_access::sqlite::{SqliteConnectOptions, SqliteOwnedBuf};
    let bytes = fs::read(path).unwrap();
    platform_io::sqlite::block_on_sqlite_connection(async {
        let mut database = SqliteConnection::connect_with(
            &SqliteConnectOptions::new().in_memory(true),
        ).await.unwrap();
        database.deserialize(None, SqliteOwnedBuf::try_from(bytes.as_slice()).unwrap(), true)
            .await.unwrap();
        let records = sqlite_access::query("SELECT kind,id,body FROM records ORDER BY kind,id")
            .fetch_all(&mut database).await.unwrap();
        let result = records.into_iter().map(|row| {
            let body: String = row.try_get("body").unwrap();
            ((row.try_get("kind").unwrap(), row.try_get("id").unwrap()),
                json_codec::from_str(&body).unwrap())
        }).collect();
        database.close().await.unwrap();
        result
    })
}

fn assert_body(engine: &Engine, version: &FileVersion, path: &Path, bytes: &[u8]) {
    let expected = fs::metadata(path).unwrap();
    let mut opened = engine.open_content(&version.version_id).unwrap();
    let actual = opened.file.metadata().unwrap();
    assert_eq!((actual.dev(), actual.ino()), (expected.dev(), expected.ino()));
    let mut read = Vec::new();
    opened.file.read_to_end(&mut read).unwrap();
    assert_eq!(read, bytes);
}

#[test]
fn automatic_backfill_keeps_retained_versions_and_reads_real_b_then_c_after_cold_open() {
    let temporary = tempfile::tempdir().unwrap();
    let system = temporary.path().join("system");
    let roots = ["first-mount", "second-mount", "third-mount"]
        .map(|mount| temporary.path().join(mount));
    for path in std::iter::once(&system).chain(roots.iter()) {
        fs::create_dir(path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
        assert_eq!(fs::read_dir(path).unwrap().count(), 0);
    }
    let config = Config::development(["Alpha", "Beta", "Gamma"].into_iter()
        .zip(roots.iter()).map(|(name, root)| {
            (name_policy::Name::parse(name).unwrap(), root.clone())
        })).unwrap();
    let disks = config.disks.clone();
    let ids = disks.iter().map(|disk| disk.disk_id.parse::<DiskId>().unwrap())
        .collect::<Vec<_>>();
    let a_component = format!("Alpha--{}", &ids[0].to_string()["disk_".len()..]);
    let b_component = format!("Beta--{}", &ids[1].to_string()["disk_".len()..]);
    let mut engine = Engine::initialize_held(
        &system, ValidatedConfig::try_from(config).unwrap(),
        roots.iter().map(|root| fs::File::open(root).unwrap()).collect(),
    ).unwrap();
    let repository = engine.create_disk_root(&disks[0].slot_id, "admin",
        OperationId::generate().unwrap()).unwrap();
    let first = save(&mut engine, &repository.main_branch_id, b"retained V1");
    let second = save(&mut engine, &repository.main_branch_id, b"retained V2");
    assert_eq!(first.file_id, second.file_id);
    assert_ne!(first.version_id, second.version_id);
    let file = first.file_id.parse::<FileId>().unwrap();
    let owner_record = engine.state().file_containers[&file].clone();
    assert_eq!(owner_record.replicas.len(), 1);
    assert_eq!(owner_record.owner_disk, ids[0]);
    let versions = engine.state().versions.clone();
    for version in [&first, &second] {
        assert_eq!(engine.state().placements[&version.version_id].route_slot_ids,
            vec![disks[0].slot_id.clone()]);
    }
    let principal = "admin".parse::<Principal>().unwrap();
    let b_request = AutomaticBackupConfigurationRequest {
        repository_id: repository.repository_id.parse().unwrap(),
        path: None, target_copies: 2, expected_generation: Generation::new(0),
        principal: &principal, operation_id: OperationId::generate().unwrap(),
    };
    let b_receipt = engine.configure_automatic_backups(b_request).unwrap();
    assert_eq!(b_receipt.commit_state, CommitState::Published);
    assert_eq!(b_receipt.result["version_count"], "2");
    assert_eq!(engine.state().versions, versions);
    let with_b = engine.state().file_containers[&file].clone();
    assert_eq!(with_b.replicas[&ids[0]], owner_record.replicas[&ids[0]]);
    assert_eq!(with_b.replicas[&ids[1]].prefixes.iter()
        .map(|prefix| prefix.source_disk).collect::<Vec<_>>(), vec![ids[0]]);
    assert_eq!(fs::read_dir(roots[2].join("common-data")).unwrap().count(), 0);
    let a_container = roots[0].join("common-data/one.txt");
    let b_prefix = roots[1].join("common-data").join(&a_component);
    let b_container = b_prefix.join("one.txt");
    let prefix_entity = fs::metadata(&b_prefix).unwrap();
    assert_eq!((with_b.replicas[&ids[1]].prefixes[0].entity.device(),
        with_b.replicas[&ids[1]].prefixes[0].entity.inode()),
        (prefix_entity.dev(), prefix_entity.ino()));
    drop(engine);
    engine = Engine::open(&system).unwrap();
    assert!(!engine.is_read_only());
    assert_eq!(engine.configure_automatic_backups(b_request).unwrap(), b_receipt);
    assert_eq!(engine.state().versions, versions);
    let b_rows = rows(&b_container.join(".zxs-file.sqlite"));
    for (version, bytes) in [(&first, b"retained V1".as_slice()),
        (&second, b"retained V2".as_slice())]
    {
        let leaf = format!("{}-one.txt", version.storage_id);
        let a_body = a_container.join("versions").join(&leaf);
        let b_body = b_container.join("versions").join(&leaf);
        assert_eq!(fs::read(&b_body).unwrap(), bytes);
        assert_eq!(b_rows[&("version".into(), version.version_id.clone())]["version"],
            json_codec::to_value(version).unwrap());
        assert!(roots[1].join("zxs-data").join(&repository.history_tree_id)
            .join("versions").join(&version.version_id).join("version.json").is_file());
        let parked = temporary.path().join(format!("A-{}", version.storage_id));
        fs::rename(&a_body, &parked).unwrap();
        assert_body(&engine, version, &b_body, bytes);
        fs::rename(&parked, &a_body).unwrap();
    }

    let c_request = AutomaticBackupConfigurationRequest {
        target_copies: 3, expected_generation: Generation::new(1),
        operation_id: OperationId::generate().unwrap(), ..b_request
    };
    let c_receipt = engine.configure_automatic_backups(c_request).unwrap();
    assert_eq!(c_receipt.commit_state, CommitState::Published);
    assert_eq!(c_receipt.result["version_count"], "2");
    let with_c = engine.state().file_containers[&file].clone();
    assert_eq!(with_c.replicas[&ids[0]], with_b.replicas[&ids[0]]);
    assert_eq!(with_c.replicas[&ids[1]], with_b.replicas[&ids[1]]);
    assert_eq!(with_c.replicas[&ids[2]].prefixes.iter()
        .map(|prefix| prefix.source_disk).collect::<Vec<_>>(), vec![ids[1], ids[0]]);
    let outer = roots[2].join("common-data").join(&b_component);
    let nested = outer.join(&a_component);
    let c_container = nested.join("one.txt");
    for (prefix, path) in with_c.replicas[&ids[2]].prefixes.iter().zip([&outer, &nested]) {
        let entity = fs::metadata(path).unwrap();
        assert_eq!((prefix.entity.device(), prefix.entity.inode()), (entity.dev(), entity.ino()));
    }
    assert!(!roots[2].join("common-data").join(&a_component).exists());
    drop(engine);
    engine = Engine::open(&system).unwrap();
    assert!(!engine.is_read_only());
    let before_retry = storage_engine::canonical_json(engine.state()).unwrap();
    assert_eq!(engine.configure_automatic_backups(c_request).unwrap(), c_receipt);
    assert_eq!(storage_engine::canonical_json(engine.state()).unwrap(), before_retry);
    assert_eq!(engine.state().versions, versions);
    let c_rows = rows(&c_container.join(".zxs-file.sqlite"));
    assert_eq!(rows(&a_container.join(".zxs-file.sqlite")), c_rows);
    assert_eq!(rows(&b_container.join(".zxs-file.sqlite")), c_rows);
    for (version, bytes) in [(&first, b"retained V1".as_slice()),
        (&second, b"retained V2".as_slice())]
    {
        assert_eq!(engine.state().versions[&version.version_id], *version);
        assert_eq!(engine.state().placements[&version.version_id].route_slot_ids,
            disks.iter().map(|disk| disk.slot_id.clone()).collect::<Vec<_>>());
        assert_eq!(c_rows[&("version".into(), version.version_id.clone())]["version"],
            json_codec::to_value(version).unwrap());
        let leaf = format!("{}-one.txt", version.storage_id);
        let a_body = a_container.join("versions").join(&leaf);
        let b_body = b_container.join("versions").join(&leaf);
        let c_body = c_container.join("versions").join(&leaf);
        assert!(roots[2].join("zxs-data").join(&repository.history_tree_id)
            .join("versions").join(&version.version_id).join("version.json").is_file());
        let parked_a = temporary.path().join(format!("C-read-A-{}", version.storage_id));
        let parked_b = temporary.path().join(format!("C-read-B-{}", version.storage_id));
        fs::rename(&a_body, &parked_a).unwrap();
        fs::rename(&b_body, &parked_b).unwrap();
        assert_body(&engine, version, &c_body, bytes);
        fs::rename(&parked_b, &b_body).unwrap();
        fs::rename(&parked_a, &a_body).unwrap();
    }
}
