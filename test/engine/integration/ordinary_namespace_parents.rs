//! Actual Initial ordinary parents and FILE-triggered B/C occurrences.
//! The original 900s terminal-switch transaction remains unchanged.
use std::{
    collections::BTreeMap, fs, io::Read,
    os::unix::fs::{MetadataExt, PermissionsExt}, path::{Path, PathBuf},
};
use storage_contract::{CommitState, DiskId, ErrorCode, Generation, OperationId, Principal};
use storage_engine::{
    AutomaticBackupConfigurationRequest, CommitRequest, Config, Engine, NamespaceOperation,
    Repository, SystemCatalogKind, SystemCatalogValue,
};

fn op() -> OperationId {
    OperationId::generate().unwrap()
}
fn mkdir(path: &str) -> NamespaceOperation {
    NamespaceOperation::Mkdir { path: name_policy::PortablePath::parse(path).unwrap() }
}
fn identity(path: &Path) -> (u64, u64) {
    let metadata = fs::metadata(path).unwrap();
    (metadata.dev(), metadata.ino())
}
fn fixture() -> (tempfile::TempDir, Engine, Repository, [PathBuf; 3]) {
    let temp = tempfile::tempdir().unwrap();
    let roots = ["alpha", "beta", "gamma"].map(|name| temp.path().join(name));
    let config = Config::development(["Alpha", "Beta", "Gamma"].into_iter()
        .zip(roots.iter()).map(|(name, root)| {
            (name_policy::Name::parse(name).unwrap(), root.clone())
        })).unwrap();
    let mut engine = Engine::initialize(&temp.path().join("system"), config).unwrap();
    let repository = engine.create_disk_root("P01", "admin", op()).unwrap();
    (temp, engine, repository, roots)
}
fn save(engine: &mut Engine, repository: &Repository, bytes: &[u8]) -> storage_engine::Entry {
    let workspace = engine.create_workspace(&repository.main_branch_id, "admin").unwrap();
    engine.stage_bytes(&workspace.workspace_id, "nested/empty/file.txt", bytes).unwrap();
    let receipt = engine.commit_workspace(CommitRequest {
        workspace_id: workspace.workspace_id, operation_id: op(), principal: "admin".into(),
        expected_head: workspace.base_head, fencing_token: workspace.fencing_token,
        message: "real FILE below an admitted parent".into(), request_digest: None,
    }).unwrap();
    assert_eq!(receipt.commit_state, CommitState::Published);
    engine.namespace_tree(&repository.main_branch_id).unwrap()
        .get("nested/empty/file.txt").unwrap().clone()
}
fn body(engine: &Engine, version: &str) -> Vec<u8> {
    let mut content = engine.open_content(version).unwrap();
    let mut bytes = Vec::new();
    content.file.read_to_end(&mut bytes).unwrap();
    bytes
}
fn journal(roots: &[PathBuf; 3]) -> Vec<Vec<u8>> {
    roots.iter().map(|root| fs::read(root.join("metadata/application-log/journal.bin"))
        .unwrap()).collect()
}
fn exact_cut(error: &storage_engine::Error, point: &str) {
    let injected = if point == "after_payloads" {
        assert_eq!(error.code(), ErrorCode::InjectedFailure, "{point}: {error}");
        error
    } else {
        assert_eq!(error.code(), ErrorCode::InDoubt, "{point}: {error}");
        let storage_engine::Error::Context { source, .. } = error else {
            panic!("missing decided error context at {point}: {error}");
        };
        source.as_ref()
    };
    assert_eq!(injected.code(), ErrorCode::InjectedFailure, "{point}: {error}");
    assert_eq!(injected.message().as_ref(), format!("interrupted at {point}").as_str());
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir(to).unwrap();
    fs::set_permissions(to, fs::Permissions::from_mode(0o700)).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}
fn catalog(
    engine: &Engine,
) -> BTreeMap<String, storage_contract::file_container::NamespaceDirectoryRecord> {
    let page = engine.system_catalog_records(SystemCatalogKind::NamespaceDirectory, None, 100)
        .unwrap();
    assert!(page.next.is_none());
    page.records.into_iter().map(|row| {
        let SystemCatalogValue::NamespaceDirectory(record) = row.value else {
            panic!("wrong SYSTEM family");
        };
        (row.id, record)
    }).collect()
}

#[test]
fn ordinary_mkdir_holds_parent_occurrences_through_cuts_file_writes_and_cold_index() {
    #[cfg(feature = "fault-injection")]
    let cuts = [None, Some("after_payloads"), Some("after_accept_1")];
    #[cfg(not(feature = "fault-injection"))]
    let cuts: [Option<&str>; 1] = [None];
    for cut in cuts {
        let (temp, mut engine, repository, roots) = fixture();
        let system = temp.path().join("system");
        let operation = op();
        let before = storage_engine::canonical_json(engine.state()).unwrap();
        if cut.is_none() {
            let orphan = roots[0].join("common-data/nested");
            fs::create_dir(&orphan).unwrap();
            fs::set_permissions(&orphan, fs::Permissions::from_mode(0o700)).unwrap();
            let physical = media(&roots);
            let error = engine.file_namespace(&repository.repository_id,
                mkdir("nested/empty"), "admin", operation).unwrap_err();
            assert_eq!(error.code(), ErrorCode::ContainerLayoutInvalid);
            assert_eq!(storage_engine::canonical_json(engine.state()).unwrap(), before);
            assert_eq!(media(&roots), physical);
            assert!(engine.operation(operation).is_err());
            fs::remove_dir(&orphan).unwrap();
        }
        if let Some(point) = cut {
            #[cfg(feature = "fault-injection")]
            engine.inject_fault_once(point);
            let error = engine.file_namespace(&repository.repository_id,
                mkdir("nested/empty"), "admin", operation).unwrap_err();
            exact_cut(&error, point);
            assert_eq!(storage_engine::canonical_json(engine.state()).unwrap(), before);
            assert!(!roots[0].join("common-data/nested").exists());
        } else {
            assert_eq!(engine.file_namespace(&repository.repository_id,
                mkdir("nested/empty"), "admin", operation).unwrap().commit_state,
                CommitState::Published);
        }
        let staged = fs::read_dir(roots[0].join("zxs-data")).unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| path.file_name().unwrap().to_str().unwrap()
                .starts_with("namespace-prepare-"))
            .map(|path| {
                let id = identity(&path);
                (path, id)
            }).collect::<Vec<_>>();
        if cut.is_some() {
            assert_eq!(staged.len(), 2);
        }
        drop(engine);
        if cut == Some("after_accept_1") {
            // Same bytes are not the actual decided directory occurrence.
            let stage = &staged[0].0;
            let parked = temp.path().join("accepted-parent");
            fs::rename(stage, &parked).unwrap();
            copy_tree(&parked, stage);
            assert_ne!(identity(stage), staged[0].1);
            let before = journal(&roots);
            assert_eq!(Engine::open(&system).err().unwrap().code(),
                ErrorCode::ContainerLayoutInvalid);
            assert_eq!(journal(&roots), before);
            assert!(!roots[0].join("common-data/nested").exists());
            fs::remove_dir_all(stage).unwrap();
            fs::rename(parked, stage).unwrap();
        }
        engine = Engine::open(&system).unwrap();
        assert!(!engine.is_read_only());
        if cut == Some("after_payloads") {
            assert!(engine.operation(operation).is_err());
            assert!(!roots[0].join("common-data/nested").exists());
            for (stage, id) in &staged {
                assert_eq!(identity(stage), *id);
            }
        }
        let result = engine.file_namespace(&repository.repository_id,
            mkdir("nested/empty"), "admin", operation).unwrap();
        assert_eq!(result.commit_state, CommitState::Published);
        let parent = roots[0].join("common-data/nested");
        let child = parent.join("empty");
        assert_eq!(fs::read_dir(&child).unwrap().count(), 0);
        let parent_id = identity(&parent);
        let child_id = identity(&child);
        let owner = engine.config().disks[0].disk_id.parse::<DiskId>().unwrap();
        let rows = catalog(&engine);
        assert_eq!(rows.len(), 2);
        for record in rows.values() {
            assert_eq!(record.created_by, operation);
            assert_eq!(record.replicas.len(), 1);
            let occurrence = &record.replicas[&owner];
            assert!(occurrence.source_chain.is_empty());
            let observed = if record.name.as_str() == "nested" { parent_id } else { child_id };
            assert_eq!((occurrence.entity.device(), occurrence.entity.inode()), observed);
            assert!(!roots[0].join("zxs-data")
                .join(format!("namespace-prepare-{}", occurrence.preparation)).exists());
        }
        if cut == Some("after_accept_1") {
            for (_, id) in &staged {
                assert!([parent_id, child_id].contains(id));
            }
        }
        // New child preparation must preserve both already published ancestors.
        engine.file_namespace(&repository.repository_id,
            mkdir("nested/empty/deeper"), "admin", op()).unwrap();
        assert_eq!(identity(&parent), parent_id);
        assert_eq!(identity(&child), child_id);
        let first = save(&mut engine, &repository, b"first nested body");
        let second = save(&mut engine, &repository, b"second nested body");
        assert_eq!(first.file_id, second.file_id);
        assert_ne!(first.version_id, second.version_id);
        assert!(child.join("file.txt/.zxs-file.sqlite").is_file());
        let records = catalog(&engine);
        drop(engine);
        // SYSTEM is a derived index. Reconstruct the new family from the same
        // decided native snapshot, not a surviving SQLite row or local locator.
        let parked_index = temp.path().join("derived-index-before-rebuild");
        fs::create_dir(&parked_index).unwrap();
        for leaf in [
            "index.sqlite", "index.sqlite-wal", "index.sqlite-shm", "index.sqlite-journal",
        ] {
            let path = system.join(leaf);
            if path.exists() {
                fs::rename(path, parked_index.join(leaf)).unwrap();
            }
        }
        assert!(!system.join("index.sqlite").exists());
        engine = Engine::open(&system).unwrap();
        assert!(system.join("index.sqlite").is_file());
        assert_eq!(catalog(&engine), records);
        assert_eq!(body(&engine, &first.version_id), b"first nested body");
        assert_eq!(body(&engine, &second.version_id), b"second nested body");
        assert_eq!(identity(&parent), parent_id);
        assert_eq!(identity(&child), child_id);
        drop(engine);
        let parked = temp.path().join("published-parent");
        fs::rename(&parent, &parked).unwrap();
        copy_tree(&parked, &parent);
        let before = journal(&roots);
        assert_eq!(Engine::open(&system).err().unwrap().code(), ErrorCode::ContainerLayoutInvalid);
        assert_eq!(journal(&roots), before);
        fs::remove_dir_all(&parent).unwrap();
        fs::rename(parked, &parent).unwrap();
        let engine = Engine::open(&system).unwrap();
        assert_eq!(body(&engine, &second.version_id), b"second nested body");
        assert_eq!(identity(&parent), parent_id);
    }
}

fn media(roots: &[PathBuf; 3]) -> BTreeMap<(usize, PathBuf), (u64, u64, Option<Vec<u8>>)> {
    fn visit(root: &Path, path: &Path, disk: usize,
        out: &mut BTreeMap<(usize, PathBuf), (u64, u64, Option<Vec<u8>>)>)
    {
        for entry in fs::read_dir(path).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            let metadata = fs::metadata(&path).unwrap();
            let bytes = if metadata.is_dir() { None } else { Some(fs::read(&path).unwrap()) };
            out.insert((disk, path.strip_prefix(root).unwrap().to_owned()),
                (metadata.dev(), metadata.ino(), bytes));
            if metadata.is_dir() {
                visit(root, &path, disk, out);
            }
        }
    }
    let mut out = BTreeMap::new();
    for (disk, root) in roots.iter().enumerate() {
        visit(root, root, disk, &mut out);
    }
    out
}

fn inspect_file(path: &Path, version: &storage_engine::FileVersion)
    -> storage_engine::ObjectDatabaseMaterial
{
    storage_engine::inspect_object_database_material(
        fs::File::open(path).unwrap(), path.to_owned(), &storage_engine::ObjectDatabaseScope::File {
            file_id: version.file_id.clone(), history_tree_id: version.history_tree_id.clone(),
        },
    ).unwrap()
}

fn assert_body_from(engine: &Engine, version: &storage_engine::FileVersion,
    path: &Path, bytes: &[u8])
{
    let mut opened = engine.open_content(&version.version_id).unwrap();
    let observed = opened.file.metadata().unwrap();
    assert_eq!((observed.dev(), observed.ino()), identity(path));
    let mut actual = Vec::new();
    opened.file.read_to_end(&mut actual).unwrap();
    assert_eq!(actual, bytes);
}

#[test]
fn nested_backup_binds_b_then_c_parent_occurrences_and_retained_bodies() {
    let (temp, mut engine, repository, roots) = fixture();
    let system = temp.path().join("system");
    engine.file_namespace(&repository.repository_id,
        mkdir("nested/empty"), "admin", op()).unwrap();
    // Standalone empty directories are not this slice's replication trigger.
    engine.file_namespace(&repository.repository_id,
        mkdir("standalone-empty"), "admin", op()).unwrap();
    let first_entry = save(&mut engine, &repository, b"nested V1");
    let second_entry = save(&mut engine, &repository, b"nested V2");
    let first = engine.state().versions[&first_entry.version_id].clone();
    let second = engine.state().versions[&second_entry.version_id].clone();
    assert_eq!(first.file_id, second.file_id);
    let versions = engine.state().versions.clone();
    let ids = engine.config().disks.iter().map(|disk| disk.disk_id.parse::<DiskId>().unwrap())
        .collect::<Vec<_>>();
    let a_component = format!("Alpha--{}", &ids[0].to_string()["disk_".len()..]);
    let b_component = format!("Beta--{}", &ids[1].to_string()["disk_".len()..]);
    let slices = [roots[0].clone(), roots[1].join(&a_component),
        roots[2].join(&b_component).join(&a_component)];
    let parents = slices.each_ref().map(|root| root.join("common-data/nested"));
    let containers = parents.each_ref().map(|parent| parent.join("empty/file.txt"));
    let mut parent_ids = vec![identity(&parents[0])];
    let mut rows = catalog(&engine);
    let principal = "admin".parse::<Principal>().unwrap();
    for copies in [2, 3] {
        let target = (copies - 1) as usize;
        let request = AutomaticBackupConfigurationRequest {
            repository_id: repository.repository_id.parse().unwrap(), path: None,
            target_copies: copies, expected_generation: Generation::new(u64::from(copies - 2)),
            principal: &principal, operation_id: op(),
        };
        #[cfg(feature = "fault-injection")]
        {
            engine.inject_fault_once("after_accept_1");
            let error = engine.configure_automatic_backups(request).unwrap_err();
            exact_cut(&error, "after_accept_1");
            assert!(!parents[target].exists());
            assert_eq!(engine.state().versions, versions);
            drop(engine);
            engine = Engine::open(&system).unwrap();
            assert!(!engine.is_read_only());
        }
        let receipt = engine.configure_automatic_backups(request).unwrap();
        assert_eq!(receipt.commit_state, CommitState::Published);
        assert_eq!(receipt.result["version_count"], "2");
        assert_eq!(engine.state().versions, versions);
        let latest = catalog(&engine);
        for (id, old) in &rows {
            let record = &latest[id];
            assert_eq!(record.entry, old.entry);
            assert_eq!(record.created_by, old.created_by);
            for (disk, occurrence) in &old.replicas {
                assert_eq!(record.replicas[disk], *occurrence);
            }
            if record.name.as_str() == "standalone-empty" {
                assert_eq!(record.replicas.len(), 1);
                continue;
            }
            let occurrence = &record.replicas[&ids[target]];
            let path = if record.name.as_str() == "nested" {
                parents[target].clone()
            } else {
                parents[target].join("empty")
            };
            assert_eq!((occurrence.entity.device(), occurrence.entity.inode()), identity(&path));
            assert_eq!(occurrence.created_by, request.operation_id);
            let source_ids = occurrence.source_chain.iter()
                .map(|source| source.disk_id.parse::<DiskId>().unwrap()).collect::<Vec<_>>();
            assert_eq!(source_ids, ids[..target].iter().rev().copied().collect::<Vec<_>>());
            assert_eq!(occurrence.root_id,
                engine.config().storage_roots[&ids[target]].root_id);
            assert_eq!((occurrence.parent_entity.device(), occurrence.parent_entity.inode()),
                identity(path.parent().unwrap()));
        }
        rows = latest;
        parent_ids.push(identity(&parents[target]));
        for (path, id) in parents.iter().zip(&parent_ids) {
            assert_eq!(identity(path), *id);
        }
        assert!(!slices[target].join("common-data/standalone-empty").exists());
        drop(engine);
        engine = Engine::open(&system).unwrap();
        assert!(!engine.is_read_only());
        let unchanged = media(&roots);
        assert_eq!(engine.configure_automatic_backups(request).unwrap(), receipt);
        assert_eq!(media(&roots), unchanged);
        assert_eq!(catalog(&engine), rows);
        for (version, bytes) in [(&first, b"nested V1".as_slice()),
            (&second, b"nested V2".as_slice())]
        {
            let leaf = format!("{}-file.txt", version.storage_id);
            let paths = containers.each_ref()
                .map(|container| container.join("versions").join(&leaf));
            let mut parked = Vec::new();
            for (index, path) in paths.iter().enumerate().take(target) {
                let to = temp.path().join(format!("held-body-{index}-{}", version.storage_id));
                fs::rename(path, &to).unwrap();
                parked.push((path, to));
            }
            assert_body_from(&engine, version, &paths[target], bytes);
            for (path, held) in parked {
                fs::rename(held, path).unwrap();
            }
            assert!(slices[target].join("zxs-data/common-data").join(&version.history_tree_id)
                .join("versions").join(&version.version_id).join("version.json").is_file());
        }
    }
    assert!(!roots[2].join(&a_component).exists());
    assert!(!roots[1].join("common-data").join(&a_component).exists());
    let third_entry = save(&mut engine, &repository, b"nested V3 after both additions");
    assert_eq!(third_entry.file_id, first.file_id);
    let third = engine.state().versions[&third_entry.version_id].clone();
    assert_eq!(catalog(&engine), rows);
    drop(engine);
    let original = inspect_file(&containers[0], &third);
    for container in &containers[1..] {
        let copy = inspect_file(container, &third);
        assert_eq!(copy.scope(), original.scope());
        assert_eq!(copy.records_hash(), original.records_hash());
        assert_eq!(copy.records(), original.records());
        for version in [&first, &second, &third] {
            assert!(copy.records().iter().any(|row| row.kind() == "version"
                && row.id() == version.version_id));
        }
    }
    // An installed prefix is insufficient: its exact nested parent is durable
    // evidence too. Never adopt a same-name, same-content replacement beneath it.
    let b_prefix = identity(&slices[1]);
    let parked = temp.path().join("original-B-parent");
    fs::rename(&parents[1], &parked).unwrap();
    copy_tree(&parked, &parents[1]);
    assert_eq!(identity(&slices[1]), b_prefix);
    assert_ne!(identity(&parents[1]), parent_ids[1]);
    let before = journal(&roots);
    assert_eq!(Engine::open(&system).err().unwrap().code(), ErrorCode::ContainerLayoutInvalid);
    assert_eq!(journal(&roots), before);
    fs::remove_dir_all(&parents[1]).unwrap();
    fs::rename(parked, &parents[1]).unwrap();
    let engine = Engine::open(&system).unwrap();
    assert_eq!(catalog(&engine), rows);
    assert_eq!(body(&engine, &third.version_id), b"nested V3 after both additions");
    for (path, id) in parents.iter().zip(parent_ids) {
        assert_eq!(identity(path), id);
    }
}
