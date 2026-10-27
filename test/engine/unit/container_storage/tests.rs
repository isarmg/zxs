use super::*;
use std::{collections::BTreeMap, os::unix::fs::symlink};

#[test]
fn disk_root_preserves_private_admission_errors() {
    use platform_io::fs_safety::Error as FsError;
    use std::os::unix::fs::PermissionsExt;

    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("root");
    assert!(matches!(
        DiskRoot::open(&path),
        Err(Error::Filesystem(FsError::Io(error)))
            if error.kind() == std::io::ErrorKind::NotFound
    ));
    fs::create_dir(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        DiskRoot::open(&path),
        Err(Error::Filesystem(FsError::UnsafePermissions(rejected))) if rejected == path
    ));
    assert_eq!(
        fs::metadata(path).unwrap().permissions().mode() & 0o777,
        0o755
    );
}

fn version(state: &State, history: &str, file: &str, name: &str, bytes: &[u8]) -> FileVersion {
    let hash = blake3::hash(bytes);
    let (storage_name, created_at) = version_identity(state, file, name, &hash).unwrap();
    FileVersion {
        history_tree_id: history.into(),
        version_id: new_id("ver"),
        file_id: file.into(),
        creation_primary_slot_id: "P01".into(),
        logical_size: bytes.len() as u64,
        version_sequence: 900,
        content_hash: ContentHash::from_digest(hash),
        checksum_chunk_bytes: 262_144,
        checksum_manifest_hash: blake3::hash(
            &ChecksumManifest {
                logical_size: bytes.len() as u64,
                chunk_bytes: 262_144,
                chunks: bytes
                    .chunks(262_144)
                    .map(|chunk| *blake3::hash(chunk).as_bytes())
                    .collect(),
            }
            .encode()
            .unwrap(),
        )
        .to_hex()
        .to_string(),
        creation_name: name.into(),
        storage_name,
        created_at,
        source_version_ids: Vec::new(),
        created_as_full_checkpoint: true,
    }
}
fn prepare(history: &Path, version: &FileVersion, bytes: &[u8]) -> PathBuf {
    let directory = history.join("versions").join(&version.version_id);
    private_dir(&directory.join("content")).unwrap();
    write_atomic(
        &directory.join("content").join(&version.creation_name),
        bytes,
        false,
    )
    .unwrap();
    let checksums = ChecksumManifest {
        logical_size: version.logical_size,
        chunk_bytes: version.checksum_chunk_bytes,
        chunks: bytes
            .chunks(version.checksum_chunk_bytes as usize)
            .map(|chunk| *blake3::hash(chunk).as_bytes())
            .collect(),
    }
    .encode()
    .unwrap();
    write_atomic(&directory.join("checksums.bin"), &checksums, false).unwrap();
    directory
}

fn install_existing(directory: &Path, version: &FileVersion) -> Result<PathBuf> {
    install_version_content(
        directory,
        PreparedContent::Verified(VerifiedExisting::read(directory, version)?),
    )
}

fn reserve(state: &mut State, version: &FileVersion) -> String {
    let id = new_id("br");
    state.branches.insert(
        id.clone(),
        Branch {
            branch_id: id.clone(),
            history_tree_id: version.history_tree_id.clone(),
            name: "draft".into(),
            head_commit_id: new_id("commit"),
            fork_commit_id: new_id("commit"),
            branch_kind: storage_contract::BranchKind::File,
            file_id: Some(version.file_id.clone()),
            file_node_id: None,
            pending_file_versions: BTreeMap::from([(
                version.file_id.clone(),
                version.version_id.clone(),
            )]),
            generation: 1,
            state: storage_contract::BranchState::Active,
        },
    );
    id
}

fn with_version(version: &FileVersion) -> State {
    let mut state = State::default();
    state
        .versions
        .insert(version.version_id.clone(), version.clone());
    state.placements.insert(
        version.version_id.clone(),
        Placement {
            version_id: version.version_id.clone(),
            primary_slot_id: "P01".into(),
            target_copies: 1,
            route_slot_ids: vec!["P01".into()],
            topology_generation: 1,
            binding_generations: BTreeMap::new(),
        },
    );
    state
        .file_names
        .insert(version.file_id.clone(), version.creation_name.clone());
    state
}

#[test]
fn version_creation_time_requires_the_current_whole_second_utc_form() {
    let mut item = version(
        &State::default(),
        &new_id("history"),
        &new_id("file"),
        "file.txt",
        b"body",
    );
    assert_eq!(item.created_at.len(), 20);
    assert!(item.created_at.ends_with('Z'));
    validate_version(&item).unwrap();
    for invalid in [
        "2026-10-24T01:00:00+00:00",
        "2026-10-24T02:00:00+01:00",
        "2026-10-24T01:00:00.000Z",
        "2026-10-24t01:00:00z",
        "2026-02-30T01:00:00Z",
    ] {
        item.created_at = invalid.into();
        assert_eq!(
            validate_version(&item).unwrap_err().code(),
            storage_contract::ErrorCode::ContainerLayoutInvalid
        );
        assert_eq!(
            item.created_at, invalid,
            "validation never rewrites stored time"
        );
    }
}

#[test]
fn current_version_requires_storage_name_and_creation_time() {
    let item = version(
        &State::default(),
        &new_id("history"),
        &new_id("file"),
        "file.txt",
        b"body",
    );
    for field in ["storage_name", "created_at"] {
        let mut missing = json_codec::to_value(&item).unwrap();
        missing.as_object_mut().unwrap().remove(field);
        let error = json_codec::from_value::<FileVersion>(missing).unwrap_err();
        assert!(
            error
                .to_string()
                .contains(&format!("missing field `{field}`"))
        );
        let mut null = json_codec::to_value(&item).unwrap();
        null[field] = json_codec::Value::Null;
        assert!(json_codec::from_value::<FileVersion>(null).is_err());
    }
    let mut tagged = json_codec::to_value(&item).unwrap();
    tagged["format_version"] = json!(2);
    assert!(json_codec::from_value::<FileVersion>(tagged).is_err());
    assert!(
        json_codec::to_value(item)
            .unwrap()
            .get("format_version")
            .is_none()
    );
}

#[test]
fn unsupported_legacy_containers_are_rejected_without_cleanup_or_migration() {
    for legacy_name in ["pending-branches", "legacy"] {
        for bytes in [b"".as_slice(), b"preserve old bytes".as_slice()] {
            let temporary = tempfile::tempdir().unwrap();
            let config = ValidatedConfig::try_from(Config::development([
                temporary.path().join("p"),
                temporary.path().join("b"),
                temporary.path().join("c"),
            ]))
            .unwrap();
            drop(
                Engine::initialize_validated(&temporary.path().join("system"), config.clone())
                    .unwrap(),
            );
            let history_id = new_id("history");
            let item = version(
                &State::default(),
                &history_id,
                &new_id("file"),
                "file.txt",
                b"body",
            );
            let history = config.disks[0].root.join("histories").join(history_id);
            let directory = prepare(&history, &item, b"body");
            let body = install_existing(&directory, &item).unwrap();
            let inode = fs::metadata(&body).unwrap().ino();
            let named = body.parent().unwrap().parent().unwrap();
            let legacy = named.join(legacy_name);
            private_dir(&legacy).unwrap();
            let retained = legacy.join("old.pending");
            write_atomic(&retained, bytes, false).unwrap();
            let previous = with_version(&item);
            let mut next = previous.clone();
            next.file_names
                .insert(item.file_id.clone(), "renamed.txt".into());
            assert_eq!(
                content_path(&directory, &item).unwrap_err().code(),
                storage_contract::ErrorCode::FormatUnsupported
            );
            assert_eq!(
                install_existing(&directory, &item).unwrap_err().code(),
                storage_contract::ErrorCode::FormatUnsupported
            );
            assert_eq!(
                synchronize_names(&config, &previous, &next)
                    .unwrap_err()
                    .code(),
                storage_contract::ErrorCode::FormatUnsupported
            );
            assert_eq!(fs::read(&retained).unwrap(), bytes);
            assert_eq!(fs::read(&body).unwrap(), b"body");
            assert_eq!(fs::metadata(&body).unwrap().ino(), inode);
            assert!(!named.parent().unwrap().join("renamed.txt").exists());
            assert!(!named.join("container.json").exists());
        }
    }
}

#[test]
fn inherited_references_update_cache_without_creating_files_and_follow_rename() {
    let temporary = tempfile::tempdir().unwrap();
    let config = ValidatedConfig::try_from(Config::development([
        temporary.path().join("p"),
        temporary.path().join("b"),
        temporary.path().join("c"),
    ]))
    .unwrap();
    drop(Engine::initialize_validated(&temporary.path().join("state"), config.clone()).unwrap());
    let history_id = new_id("history");
    let file = new_id("file");
    let item = version(
        &State::default(),
        &history_id,
        &file,
        "original.txt",
        b"body",
    );
    let history = config.disks[0].root.join("histories").join(&history_id);
    let directory = prepare(&history, &item, b"body");
    let body = install_existing(&directory, &item).unwrap();
    let body_inode = fs::metadata(&body).unwrap().ino();
    let previous = with_version(&item);
    let mut pending = previous.clone();
    let branch = reserve(&mut pending, &item);
    synchronize_names(&config, &previous, &pending).unwrap();
    let named = body.parent().unwrap().parent().unwrap();
    assert!(!named.join("pending-branches").exists());
    synchronize_names(&config, &previous, &pending).unwrap();
    assert!(!named.join("pending-branches").exists());
    let cached: json_codec::Value = read_json(&named.join("container.json")).unwrap();
    assert_eq!(cached["inherited_branches"][0]["branch_id"], branch);
    assert_eq!(
        cached["inherited_branches"][0]["source_version_id"],
        item.version_id
    );
    assert_eq!(
        cached["inherited_branches"][0]["version_id"],
        item.version_id
    );
    assert!(cached["inherited_branches"][0]["own_version_id"].is_null());
    assert_eq!(cached["inherited_branches"][0]["inherited"], true);
    assert_eq!(cached["inherited_branches"][0]["materialized"], false);
    assert!(cached.get("pending_branches").is_none());
    assert!(cached["inherited_branches"][0].get("placeholder").is_none());
    assert_eq!(pending.versions.len(), previous.versions.len());
    assert_eq!(fs::read_dir(body.parent().unwrap()).unwrap().count(), 1);
    assert_eq!(fs::metadata(&body).unwrap().ino(), body_inode);
    assert_eq!(fs::read(&body).unwrap(), b"body");

    let mut renamed = pending.clone();
    renamed.file_names.insert(file.clone(), "renamed.md".into());
    synchronize_names(&config, &pending, &renamed).unwrap();
    let new_named = named.parent().unwrap().join("renamed.md");
    assert!(!new_named.join("pending-branches").exists());
    let new_body = content_path(&directory, &item).unwrap();
    assert_eq!(fs::metadata(&new_body).unwrap().ino(), body_inode);
    let mut written = renamed.clone();
    written
        .branches
        .get_mut(&branch)
        .unwrap()
        .pending_file_versions
        .clear();
    synchronize_names(&config, &renamed, &written).unwrap();
    assert!(!new_named.join("pending-branches").exists());
    assert_eq!(fs::metadata(&new_body).unwrap().ino(), body_inode);
    let cached: json_codec::Value = read_json(&new_named.join("container.json")).unwrap();
    assert_eq!(cached["inherited_branches"], json!([]));
}

#[test]
fn physical_move_preserves_one_body_and_old_version_identity_across_replay() {
    let temporary = tempfile::tempdir().unwrap();
    let config = ValidatedConfig::try_from(Config::development([
        temporary.path().join("p"),
        temporary.path().join("b"),
        temporary.path().join("c"),
    ]))
    .unwrap();
    drop(Engine::initialize_validated(&temporary.path().join("state"), config.clone()).unwrap());
    let history_id = new_id("history");
    let file = new_id("file");
    let item = version(
        &State::default(),
        &history_id,
        &file,
        "报告.txt",
        b"retained parent bytes",
    );
    let history = config.disks[0].root.join("histories").join(&history_id);
    let directory = prepare(&history, &item, b"retained parent bytes");
    let original = install_existing(&directory, &item).unwrap();
    let inode = fs::metadata(&original).unwrap().ino();
    let previous = with_version(&item);
    let mut in_a = previous.clone();
    in_a.file_locations
        .insert(file.clone(), "a/报告.txt".into());
    synchronize_names(&config, &previous, &in_a).unwrap();
    let a = content_path(&directory, &item).unwrap();
    assert!(
        a.to_string_lossy()
            .contains("/namespace/a/报告.txt/versions/")
    );
    let mut in_b = in_a.clone();
    in_b.file_locations
        .insert(file.clone(), "b/报告.txt".into());
    synchronize_names(&config, &in_a, &in_b).unwrap();
    let b = content_path(&directory, &item).unwrap();
    assert!(
        b.to_string_lossy()
            .contains("/namespace/b/报告.txt/versions/")
    );
    assert!(!a.exists());
    assert!(!original.exists());
    assert_eq!(fs::metadata(&b).unwrap().ino(), inode);
    assert_eq!(fs::metadata(&b).unwrap().nlink(), 1);
    assert_eq!(fs::read(&b).unwrap(), b"retained parent bytes");
    assert_eq!(in_b.versions.len(), previous.versions.len());
    // A decided operation may replay against either its preceding state or
    // an empty reconstructed cache; old F1 still resolves by its stable ID.
    synchronize_names(&config, &in_a, &in_b).unwrap();
    synchronize_names(&config, &State::default(), &in_b).unwrap();
    assert_eq!(content_path(&directory, &item).unwrap(), b);
    assert_eq!(fs::metadata(&b).unwrap().ino(), inode);
    let cache: json_codec::Value =
        read_json(&b.parent().unwrap().parent().unwrap().join("container.json")).unwrap();
    assert_eq!(cache["location"], "b/报告.txt");
}

#[test]
fn decided_cross_directory_move_recovers_both_views_and_one_physical_container() {
    for fault in ["after_finalize", "after_container_names"] {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("state");
        let config = ValidatedConfig::try_from(Config::development([
            temporary.path().join("p"),
            temporary.path().join("b"),
            temporary.path().join("c"),
        ]))
        .unwrap();
        let mut engine = Engine::initialize_validated(&root, config.clone()).unwrap();
        let repo = engine
            .create_repository(
                &storage_contract::RepositoryName::parse("移动恢复").unwrap(),
                "P01",
                3,
                "alice",
                storage_contract::OperationId::generate(),
            )
            .unwrap();
        let workspace = engine
            .create_workspace(&repo.main_branch_id, "alice")
            .unwrap();
        engine
            .stage_bytes(&workspace.workspace_id, "a/f.txt", b"frozen F1")
            .unwrap();
        engine
            .stage_bytes(&workspace.workspace_id, "b/keep.txt", b"untouched")
            .unwrap();
        engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: storage_contract::OperationId::generate(),
                principal: "alice".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "parent files".into(),
                request_digest: None,
            })
            .unwrap();
        let file = engine
            .list_tree(&repo.main_branch_id)
            .unwrap()
            .into_iter()
            .find(|entry| entry.path == "a/f.txt")
            .unwrap();
        let source = engine
            .initialize_directory_scope(
                &repo.repository_id,
                Some(&name_policy::LookupPath::parse("a").unwrap()),
                "alice",
                storage_contract::OperationId::generate(),
            )
            .unwrap()
            .result["directory_branch_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let target = engine
            .initialize_directory_scope(
                &repo.repository_id,
                Some(&name_policy::LookupPath::parse("b").unwrap()),
                "alice",
                storage_contract::OperationId::generate(),
            )
            .unwrap()
            .result["directory_branch_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let retained = engine
            .create_directory_branches(
                &repo.repository_id,
                Some(&name_policy::LookupPath::parse("a").unwrap()),
                &storage_contract::BranchName::parse("retained").unwrap(),
                "alice",
                storage_contract::OperationId::generate(),
            )
            .unwrap()
            .result["directory_branch_id"]
            .as_str()
            .unwrap()
            .to_owned();
        let retained_head = engine.state.directory_branches[&retained]
            .head_tree_id
            .clone();
        let item = engine.state.versions[&file.version_id].clone();
        let inodes = config
            .disks
            .iter()
            .map(|disk| {
                let directory = disk
                    .root
                    .join("histories")
                    .join(&repo.history_tree_id)
                    .join("versions")
                    .join(&file.version_id);
                let body = content_path(&directory, &item).unwrap();
                (body.clone(), fs::metadata(body).unwrap().ino())
            })
            .collect::<Vec<_>>();
        let request = DirectoryNamespaceBatch {
            kind: storage_contract::DirectoryBatchKind::Move,
            items: vec![DirectoryNamespaceItem {
                source: DirectoryNamespaceSource {
                    directory_branch_id: source.clone(),
                    expected_tree_id: engine.state.directory_branches[&source]
                        .head_tree_id
                        .clone(),
                    path: "a/f.txt".into(),
                    entry_id: file.entry_id.clone(),
                },
                target: Some(DirectoryNamespaceTarget {
                    directory_branch_id: target.clone(),
                    expected_tree_id: engine.state.directory_branches[&target]
                        .head_tree_id
                        .clone(),
                    path: "b/f.txt".into(),
                }),
            }],
        };
        let operation = storage_contract::OperationId::generate();
        engine.inject_fault_once(fault);
        assert_eq!(
            engine
                .directory_namespace_batch(&repo.repository_id, &request, "alice", operation)
                .unwrap_err()
                .code(),
            storage_contract::ErrorCode::InDoubt
        );
        drop(engine);
        let mut engine = Engine::open(&root).unwrap();
        let receipt = engine
            .directory_namespace_batch(&repo.repository_id, &request, "alice", operation)
            .unwrap();
        assert_eq!(engine.operation(operation).unwrap().result, receipt.result);
        assert!(
            !engine.state.directory_branches[&source]
                .entries
                .iter()
                .any(|entry| entry.file_id == file.file_id)
        );
        assert!(
            engine.state.directory_branches[&target]
                .entries
                .iter()
                .any(|entry| entry.file_id == file.file_id && entry.version_id == file.version_id)
        );
        assert_eq!(
            engine.state.directory_branches[&retained].head_tree_id,
            retained_head
        );
        assert_eq!(engine.state.file_locations[&file.file_id], "b/f.txt");
        for (disk, (old_body, inode)) in config.disks.iter().zip(inodes) {
            let directory = disk
                .root
                .join("histories")
                .join(&repo.history_tree_id)
                .join("versions")
                .join(&file.version_id);
            let body = content_path(&directory, &item).unwrap();
            assert!(
                body.to_string_lossy()
                    .contains("/namespace/b/f.txt/versions/")
            );
            assert!(!old_body.exists());
            assert_eq!(fs::metadata(&body).unwrap().ino(), inode);
            assert_eq!(fs::metadata(&body).unwrap().nlink(), 1);
            assert_eq!(fs::read(body).unwrap(), b"frozen F1");
        }
    }
}

#[test]
fn canonical_location_handles_reserved_looking_user_names_and_prefix_moves() {
    let temporary = tempfile::tempdir().unwrap();
    let config = ValidatedConfig::try_from(Config::development([
        temporary.path().join("p"),
        temporary.path().join("b"),
        temporary.path().join("c"),
    ]))
    .unwrap();
    drop(Engine::initialize_validated(&temporary.path().join("state"), config.clone()).unwrap());
    let history_id = new_id("history");
    let file = new_id("file");
    let item = version(
        &State::default(),
        &history_id,
        &file,
        "namespace",
        b"single body",
    );
    let history = config.disks[0].root.join("histories").join(&history_id);
    let directory = prepare(&history, &item, b"single body");
    let original = install_existing(&directory, &item).unwrap();
    let inode = fs::metadata(&original).unwrap().ino();
    let mut state = with_version(&item);
    for location in [
        "namespace",
        "namespace/sub/namespace",
        "namespace",
        "versions/.zxs-container-owner.json/namespace",
        "safe/.zxs-namespace.json/namespace",
        "safe/.zxs-namespace.json/namespace/sub/namespace",
        "safe/.zxs-namespace.json/namespace",
    ] {
        let previous = state.clone();
        state.file_locations.insert(file.clone(), location.into());
        synchronize_names(&config, &previous, &state).unwrap();
        let body = content_path(&directory, &item).unwrap();
        assert_eq!(fs::metadata(&body).unwrap().ino(), inode);
        assert_eq!(fs::read(body).unwrap(), b"single body");
    }
}

#[test]
fn new_name_admission_does_not_revalidate_published_projection_spelling() {
    let temporary = tempfile::tempdir().unwrap();
    let config = ValidatedConfig::try_from(Config::development([
        temporary.path().join("p"),
        temporary.path().join("b"),
        temporary.path().join("c"),
    ]))
    .unwrap();
    drop(Engine::initialize_validated(&temporary.path().join("state"), config.clone()).unwrap());
    let resources = Arc::new(DiskResources::open(&config, true).unwrap());
    let history_id = new_id("history");
    let file = new_id("file");
    let item = version(&State::default(), &history_id, &file, "file.txt", b"body");
    let history = config.disks[0].root.join("histories").join(&history_id);
    let directory = prepare(&history, &item, b"body");
    let original = install_existing(&directory, &item).unwrap();
    let inode = fs::metadata(&original).unwrap().ino();
    let previous = with_version(&item);
    let mut published = previous.clone();
    published
        .file_names
        .insert(file.clone(), ".ZXS-NAMESPACE.JSON".into());
    published
        .file_locations
        .insert(file.clone(), ".ZXS-NAMESPACE.JSON".into());

    assert!(matches!(
        prepare_names(&config, &resources, &ProjectionTransition::new(&previous, &published, None).unwrap(), MAX_METADATA),
        Err(error) if error.code() == storage_contract::ErrorCode::NameCollision
    ));
    // The same physical spelling is valid for replay of an immutable decision.
    // This is the existing publication phase boundary, not a second disk format.
    prepare_names_inner(
        &config,
        &resources,
        &ProjectionTransition::new(&previous, &published, None).unwrap(),
        MAX_METADATA,
        true,
    )
    .unwrap()
    .apply()
    .unwrap();
    let body = content_path(&directory, &item).unwrap();
    assert_eq!(fs::metadata(&body).unwrap().ino(), inode);
    assert_eq!(fs::read(&body).unwrap(), b"body");

    let mut renamed = published.clone();
    renamed
        .file_names
        .insert(file.clone(), NAMESPACE_MARKER.into());
    renamed
        .file_locations
        .insert(file.clone(), NAMESPACE_MARKER.into());
    assert!(
        matches!(
            prepare_names(&config, &resources, &ProjectionTransition::new(&published, &renamed, None).unwrap(), MAX_METADATA),
            Err(error) if error.code() == storage_contract::ErrorCode::NameCollision
        ),
        "case-only changes are new admissions, not publication replay"
    );
    assert_eq!(content_path(&directory, &item).unwrap(), body);

    let marker = history
        .join("file-containers")
        .join(file)
        .join("namespace")
        .join(NAMESPACE_MARKER);
    write_json(&marker, &new_id("file"), true).unwrap();
    let evidence = fs::read(&marker).unwrap();
    assert!(matches!(
        prepare_names_inner(&config, &resources, &ProjectionTransition::new(&previous, &published, None).unwrap(), MAX_METADATA, true),
        Err(error) if error.code() == storage_contract::ErrorCode::ContainerLayoutInvalid
    ));
    assert_eq!(
        fs::read(marker).unwrap(),
        evidence,
        "invalid marker evidence is preserved"
    );
    assert_eq!(fs::metadata(&body).unwrap().ino(), inode);
    assert_eq!(fs::read(body).unwrap(), b"body");
}

#[test]
fn identity_counts_each_file_independently_with_required_numbered_names() {
    let mut state = State::default();
    let file = new_id("file");
    let history = new_id("history");
    let old = version(&state, &history, &file, "报告.txt", b"first");
    state.versions.insert(old.version_id.clone(), old);
    let next = version(&state, &history, &file, "报告.txt", b"next");
    assert_eq!(sequence(&next.storage_name).unwrap(), 2);
    assert!(
        next.storage_name
            .ends_with(&format!("_{}.txt", &blake3::hash(b"next").to_hex()[..8]))
    );
    assert!(next.created_at.ends_with('Z'));
    assert_eq!(next.created_at.len(), 20);
    state.versions.insert(next.version_id.clone(), next);
    state.file_names.insert(file.clone(), "改名.md".into());
    assert!(
        version(&state, &history, &file, "报告.txt", b"new extension")
            .storage_name
            .ends_with(".md")
    );
    assert_eq!(
        sequence(
            version(&state, &history, &file, "renamed.md", b"third")
                .storage_name
                .as_str()
        )
        .unwrap(),
        3
    );
    assert_eq!(
        sequence(
            version(&state, &history, &new_id("file"), "other.txt", b"other")
                .storage_name
                .as_str()
        )
        .unwrap(),
        1
    );
    assert!(
        version_identity(
            &state,
            "invalid-file-id",
            "valid.txt",
            &blake3::hash(b"body")
        )
        .is_err()
    );
}

#[test]
fn identity_preserves_number_width_and_exhaustion_without_global_sequence_coupling() {
    let history = new_id("history");
    let file = new_id("file");
    let original = version(&State::default(), &history, &file, "file.txt", b"old");
    let digest = original.content_hash.digest().to_hex();
    for (number, expected) in [
        ("000001", 2),
        ("0000001", 2),
        ("1000000", 1_000_001),
        ("18446744073709551614", u64::MAX),
    ] {
        let mut retained = original.clone();
        // The storage stamp has its existing shape-only contract; it need not
        // equal the independently validated RFC3339 creation time.
        retained.storage_name = format!("v{number}_20269999T999999Z_{}.txt", &digest[..8]);
        validate_version(&retained).unwrap();
        let mut state = with_version(&retained);
        state.sequence = u64::MAX;
        let before = state.versions.clone();
        let generated = version(&state, &history, &file, "file.txt", b"new");
        assert_eq!(sequence(&generated.storage_name).unwrap(), expected);
        assert_eq!(state.versions, before);
    }

    let mut last = original;
    last.storage_name = format!("v{}_20261018T133000Z_{}.txt", u64::MAX, &digest[..8]);
    validate_version(&last).unwrap();
    let state = with_version(&last);
    let error = version_identity(&state, &file, "file.txt", &blake3::hash(b"next")).unwrap_err();
    assert_eq!(error.code(), ErrorCode::ResourceLimit);
    assert_eq!(error.message(), "file version sequence exhausted");
    let other = version(&state, &history, &new_id("file"), "other.txt", b"other");
    assert_eq!(sequence(&other.storage_name).unwrap(), 1);
}

#[test]
fn raw_storage_sequence_parser_retains_format_errors() {
    for number in ["000000", "00001", "18446744073709551616", "-00001"] {
        let name = format!("v{number}_20261018T133000Z_01234567.txt");
        assert_eq!(
            sequence(&name).unwrap_err().code(),
            ErrorCode::ContainerLayoutInvalid
        );
    }
}

#[test]
fn installation_moves_one_ordinary_inode_and_missing_paths_remain_classifiable() {
    let temporary = tempfile::tempdir().unwrap();
    let history_id = new_id("history");
    let file = new_id("file");
    let history = temporary.path().join(&history_id);
    let item = version(
        &State::default(),
        &history_id,
        &file,
        "notes.txt",
        b"complete bytes",
    );
    let directory = prepare(&history, &item, b"complete bytes");
    let prepared = directory.join("content/notes.txt");
    assert_eq!(
        content_path(&directory, &item).unwrap(),
        prepared,
        "a prepared body remains readable before its atomic move"
    );
    let before = fs::metadata(&prepared).unwrap();
    let target = install_existing(&directory, &item).unwrap();
    assert!(!prepared.exists());
    let after = fs::metadata(&target).unwrap();
    assert_eq!((before.dev(), before.ino()), (after.dev(), after.ino()));
    assert_eq!(after.nlink(), 1);
    assert_eq!(fs::read(&target).unwrap(), b"complete bytes");
    assert_eq!(install_existing(&directory, &item).unwrap(), target);
    assert_eq!(content_path(&directory, &item).unwrap(), target);
    fs::remove_file(&target).unwrap();
    assert_eq!(
        content_path(&directory, &item).unwrap(),
        target,
        "missing content returns the expected path rather than a generic layout error"
    );
    symlink("/does-not-exist", &target).unwrap();
    assert_eq!(
        content_path(&directory, &item).unwrap_err().code(),
        storage_contract::ErrorCode::ContainerLayoutInvalid
    );
}

#[test]
fn rename_and_note_projection_replays_without_changing_body_identity() {
    let temporary = tempfile::tempdir().unwrap();
    let config = ValidatedConfig::try_from(Config::development([
        temporary.path().join("primary"),
        temporary.path().join("backup"),
        temporary.path().join("tertiary"),
    ]))
    .unwrap();
    let engine =
        Engine::initialize_validated(&temporary.path().join("state"), config.clone()).unwrap();
    drop(engine);
    let history_id = new_id("history");
    let file = new_id("file");
    let history = config.disks[0].root.join("histories").join(&history_id);
    let mut previous = State::default();
    let item = version(
        &previous,
        &history_id,
        &file,
        "original.txt",
        b"one unique full file",
    );
    let directory = prepare(&history, &item, b"one unique full file");
    let original = install_existing(&directory, &item).unwrap();
    let inode = fs::metadata(&original).unwrap().ino();
    previous
        .versions
        .insert(item.version_id.clone(), item.clone());
    previous.placements.insert(
        item.version_id.clone(),
        Placement {
            version_id: item.version_id.clone(),
            primary_slot_id: "P01".into(),
            target_copies: 1,
            route_slot_ids: vec!["P01".into()],
            topology_generation: 1,
            binding_generations: BTreeMap::new(),
        },
    );
    previous
        .file_names
        .insert(file.clone(), "original.txt".into());
    let mut next = previous.clone();
    next.file_names.insert(file.clone(), "重命名.md".into());
    next.file_notes.insert(file.clone(), "文件备注".into());
    next.version_notes
        .insert(item.version_id.clone(), "版本备注".into());
    next.file_selections.insert(file.clone(), new_id("br"));
    synchronize_names(&config, &previous, &next).unwrap();
    let renamed = content_path(&directory, &item).unwrap();
    assert!(renamed.to_str().unwrap().contains("/重命名.md/versions/"));
    assert!(!original.exists());
    assert_eq!(fs::metadata(&renamed).unwrap().ino(), inode);
    assert_eq!(
        ContentHash::from_digest(blake3::hash(&fs::read(&renamed).unwrap())),
        item.content_hash
    );
    let cache = renamed
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("container.json");
    let cached: json_codec::Value = read_json(&cache).unwrap();
    assert_eq!(cached["name"], "重命名.md");
    assert_eq!(cached["note"], "文件备注");
    assert_eq!(cached["versions"][0]["note"], "版本备注");
    assert!(cached.get("selected_branch_id").is_none());
    synchronize_names(&config, &previous, &next).unwrap();
    assert_eq!(fs::metadata(&renamed).unwrap().ino(), inode);
    let cache_inode = fs::metadata(&cache).unwrap().ino();
    synchronize_names(&config, &next, &next).unwrap();
    assert_eq!(
        fs::metadata(&cache).unwrap().ino(),
        cache_inode,
        "unrelated transactions do not rewrite the cache"
    );
    private_dir(
        &history
            .join("file-containers")
            .join(&file)
            .join("ambiguous.txt"),
    )
    .unwrap();
    assert_eq!(
        content_path(&directory, &item).unwrap_err().code(),
        storage_contract::ErrorCode::ContainerAmbiguous
    );
}

#[test]
fn empty_name_projection_has_no_staging_file() {
    let temporary = tempfile::tempdir().unwrap();
    let config = ValidatedConfig::try_from(Config::development([
        temporary.path().join("p"),
        temporary.path().join("b"),
        temporary.path().join("c"),
    ]))
    .unwrap();
    let resources = Arc::new(DiskResources::open(&config, true).unwrap());
    let previous = State::default();
    let mut metadata_only = previous.clone();
    metadata_only.sequence = 1;
    metadata_only
        .file_notes
        .insert(new_id("file"), "no physical container".into());
    for next in [&previous, &metadata_only] {
        for historical in [false, true] {
            let prepared = prepare_names_inner(
                &config,
                &resources,
                &ProjectionTransition::new(&previous, next, None).unwrap(),
                0,
                historical,
            )
            .unwrap();
            assert!(
                prepared.cache.is_none(),
                "empty projections must not allocate staging"
            );
            prepared.apply().unwrap();
        }
    }
    for disk in &config.disks {
        assert!(!disk.root.exists());
    }
}

fn capture_fixture() -> (State, String, String) {
    let temporary = tempfile::tempdir().unwrap();
    let mut engine = Engine::initialize(
        &temporary.path().join("system"),
        Config::development([temporary.path().join("disk")]),
    )
    .unwrap();
    let repo = engine
        .create_repository(
            &storage_contract::RepositoryName::parse("Names").unwrap(),
            "P01",
            1,
            "owner",
            storage_contract::OperationId::generate(),
        )
        .unwrap();
    let mut state = engine.state().clone();
    let head = state.branches[&repo.main_branch_id].head_commit_id.clone();
    let file = new_id("file");
    state.commits.get_mut(&head).unwrap().entries = ["first.txt", "alias.txt"]
        .into_iter()
        .map(|name| Entry {
            entry_id: new_id("entry"),
            path: name_policy::PortablePath::parse(&(format!("folder/{name}"))).unwrap(),
            kind: NamespaceEntryKind::File,
            file_id: file.clone(),
            version_id: new_id("ver"),
            size: 1,
        })
        .collect();
    (state, repo.main_branch_id, file)
}

#[test]
fn capture_names_only_inserts_missing_fields_and_keeps_first_alias() {
    let (mut state, _, file) = capture_fixture();
    capture_names(&mut state);
    assert_eq!(state.file_names[&file], "first.txt");
    assert_eq!(state.file_locations[&file], "folder/first.txt");

    state.file_names.insert(file.clone(), "retained.txt".into());
    state.file_locations.remove(&file);
    capture_names(&mut state);
    assert_eq!(state.file_names[&file], "retained.txt");
    assert_eq!(state.file_locations[&file], "folder/first.txt");

    state.file_names.remove(&file);
    state
        .file_locations
        .insert(file.clone(), "retained/location.txt".into());
    capture_names(&mut state);
    assert_eq!(state.file_names[&file], "first.txt");
    assert_eq!(state.file_locations[&file], "retained/location.txt");

    let before = state.clone();
    state.begin_tracking();
    capture_names(&mut state);
    assert!(
        state
            .file_names
            .changes_since(&before.file_names)
            .is_empty()
    );
    assert!(
        state
            .file_locations
            .changes_since(&before.file_locations)
            .is_empty()
    );
}

#[test]
fn identity_preserves_generated_storage_name_limit() {
    let name = format!("x.{}", "字".repeat(75));
    name_policy::validate_path(&name).unwrap();
    let error = version_identity(
        &State::default(),
        &new_id("file"),
        &name,
        &blake3::hash(b"body"),
    )
    .unwrap_err();
    assert_eq!(error.code(), storage_contract::ErrorCode::NameTooLong);
}

#[test]
#[should_panic(expected = "invariant: every repository has its main branch")]
fn name_capture_exposes_missing_internal_main_branch() {
    let (mut state, _, _) = capture_fixture();
    let branch = state
        .repositories
        .values()
        .next()
        .unwrap()
        .main_branch_id
        .clone();
    state.branches.remove(&branch);
    capture_names(&mut state);
}

#[test]
fn copied_body_installs_owned_inode_but_existing_corrupt_target_is_rejected() {
    use std::os::unix::fs::PermissionsExt;
    let temporary = tempfile::tempdir().unwrap();
    let history_id = new_id("history");
    let history = temporary.path().join(&history_id);
    let bytes = b"a complete copied payload";
    let mut item = version(
        &State::default(),
        &history_id,
        &new_id("file"),
        "body",
        bytes,
    );
    let manifest = ChecksumManifest {
        logical_size: bytes.len() as u64,
        chunk_bytes: item.checksum_chunk_bytes,
        chunks: vec![*blake3::hash(bytes).as_bytes()],
    };
    item.checksum_manifest_hash = blake3::hash(&manifest.encode().unwrap())
        .to_hex()
        .to_string();
    let external = temporary.path().join("external");
    fs::write(&external, bytes).unwrap();
    let directory = history.join("versions").join(&item.version_id);
    private_dir(&directory.join("content")).unwrap();
    let source = directory.join("content").join(&item.creation_name);
    let copied = verified_full_copy(
        open_regular(&external).unwrap(),
        &source,
        &item,
        item.logical_size,
    )
    .unwrap();
    write_atomic(
        &directory.join("checksums.bin"),
        &manifest.encode().unwrap(),
        false,
    )
    .unwrap();
    let before = fs::metadata(&source).unwrap();
    let target = install_version_content(&directory, PreparedContent::Copied(copied)).unwrap();
    let installed = fs::metadata(&target).unwrap();
    assert_eq!(
        (before.dev(), before.ino()),
        (installed.dev(), installed.ino())
    );
    assert_eq!(installed.nlink(), 1);
    assert_eq!(installed.mode() & 0o777, 0o400);
    assert!(!source.exists());
    assert_eq!(fs::read(&target).unwrap(), bytes);

    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&target, b"a corrupted copy payload").unwrap();
    let copied = verified_full_copy(
        open_regular(&external).unwrap(),
        &source,
        &item,
        item.logical_size,
    )
    .unwrap();
    assert_eq!(
        install_version_content(&directory, PreparedContent::Copied(copied))
            .unwrap_err()
            .code(),
        ErrorCode::ContentCorrupt
    );
    assert_eq!(fs::read(&source).unwrap(), bytes);
    assert_eq!(fs::read(&target).unwrap(), b"a corrupted copy payload");
    assert_eq!(
        install_existing(&directory, &item).unwrap_err().code(),
        ErrorCode::ChecksumMismatch
    );
}

#[test]
fn resumed_installation_verifies_external_prepared_body() {
    let temporary = tempfile::tempdir().unwrap();
    let history_id = new_id("history");
    let history = temporary.path().join(&history_id);
    let item = version(
        &State::default(),
        &history_id,
        &new_id("file"),
        "body",
        b"original",
    );
    let directory = prepare(&history, &item, b"modified");
    assert_eq!(
        install_existing(&directory, &item).unwrap_err().code(),
        ErrorCode::ChecksumMismatch
    );
    assert_eq!(
        fs::read(directory.join("content/body")).unwrap(),
        b"modified"
    );
}

fn verified_existing_fixture() -> (tempfile::TempDir, DiskConfig, FileVersion, PathBuf) {
    let temporary = tempfile::tempdir().unwrap();
    let config =
        ValidatedConfig::try_from(Config::development([temporary.path().join("disk")])).unwrap();
    let disk = config.disks[0].clone();
    let history = disk.root.join("histories").join(new_id("history"));
    let bytes = b"verified existing body";
    let mut item = version(
        &State::default(),
        history.file_name().unwrap().to_str().unwrap(),
        &new_id("file"),
        "body",
        bytes,
    );
    let manifest = ChecksumManifest {
        logical_size: bytes.len() as u64,
        chunk_bytes: item.checksum_chunk_bytes,
        chunks: vec![*blake3::hash(bytes).as_bytes()],
    }
    .encode()
    .unwrap();
    item.checksum_manifest_hash = blake3::hash(&manifest).to_hex().to_string();
    let directory = prepare(&history, &item, bytes);
    (temporary, disk, item, directory)
}

#[test]
fn rooted_container_lookup_keeps_nested_markers_and_body_on_the_held_disk() {
    let (_temporary, disk, item, directory) = verified_existing_fixture();
    let installed = install_existing(&directory, &item).unwrap();
    let named = installed.parent().unwrap().parent().unwrap();
    let stable = named.parent().unwrap();
    let root = DiskRoot::open(&disk.root).unwrap();
    let named = synchronize_location(
        &root,
        named.to_path_buf(),
        stable,
        &item.file_id,
        Some("nested/branch/body"),
        "body",
    )
    .unwrap();
    let body = named.join("versions").join(&item.storage_name);
    let before = fs::metadata(&body).unwrap();
    let moved = disk.root.with_extension("held");
    fs::rename(&disk.root, &moved).unwrap();
    private_dir(&disk.root).unwrap();
    let replacement_history = history_root(&directory).unwrap();
    prepare(replacement_history, &item, b"replacement disk bytes");

    let resolved = content_path_at(&root, &directory, &item, &mut |_| Ok(())).unwrap();
    assert_eq!(resolved, body);
    let mut held = root.open_regular(&resolved).unwrap();
    let after = held.metadata().unwrap();
    assert_eq!((before.dev(), before.ino()), (after.dev(), after.ino()));
    let mut bytes = Vec::new();
    held.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"verified existing body");
    assert_eq!(
        fs::read(directory.join("content/body")).unwrap(),
        b"replacement disk bytes",
        "looking up through the old root neither reads nor changes its replacement"
    );

    let marker = moved
        .join(named.strip_prefix(&disk.root).unwrap())
        .join(OWNER_MARKER);
    write_json(&marker, &new_id("file"), true).unwrap();
    assert_eq!(
        content_path_at(&root, &directory, &item, &mut |_| Ok(()))
            .unwrap_err()
            .code(),
        ErrorCode::ContainerLayoutInvalid,
        "held resource identity never makes subsequently read marker bytes trusted"
    );
}

#[test]
fn rooted_container_lookup_still_rejects_ambiguity_after_root_replacement() {
    let (_temporary, disk, item, directory) = verified_existing_fixture();
    let installed = install_existing(&directory, &item).unwrap();
    let stable = installed
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap();
    let root = DiskRoot::open(&disk.root).unwrap();
    let moved = disk.root.with_extension("held");
    fs::rename(&disk.root, &moved).unwrap();
    private_dir(&disk.root).unwrap();
    private_dir(
        &moved
            .join(stable.strip_prefix(&disk.root).unwrap())
            .join("another-container"),
    )
    .unwrap();
    assert_eq!(
        content_path_at(&root, &directory, &item, &mut |_| Ok(()))
            .unwrap_err()
            .code(),
        ErrorCode::ContainerAmbiguous
    );
}

#[test]
fn rooted_container_lookup_preserves_missing_and_linked_body_classification() {
    let (_temporary, disk, item, directory) = verified_existing_fixture();
    let installed = install_existing(&directory, &item).unwrap();
    let root = DiskRoot::open(&disk.root).unwrap();
    fs::remove_file(&installed).unwrap();
    assert_eq!(
        content_path_at(&root, &directory, &item, &mut |_| Ok(())).unwrap(),
        installed
    );
    symlink("/does-not-exist", &installed).unwrap();
    assert_eq!(
        content_path_at(&root, &directory, &item, &mut |_| Ok(()))
            .unwrap_err()
            .code(),
        ErrorCode::ContainerLayoutInvalid
    );
}

#[test]
fn rooted_container_lookup_preserves_missing_metadata_parent_classification() {
    for missing in ["versions", "history", "histories"] {
        let (_temporary, disk, item, directory) = verified_existing_fixture();
        let history = history_root(&directory).unwrap();
        let root = DiskRoot::open(&disk.root).unwrap();
        let missing = match missing {
            "versions" => history.join("versions"),
            "history" => history.to_path_buf(),
            "histories" => disk.root.join("histories"),
            _ => unreachable!(),
        };
        fs::remove_dir_all(missing).unwrap();
        let expected = history
            .join("file-containers")
            .join(&item.file_id)
            .join(&item.creation_name)
            .join("versions")
            .join(&item.storage_name);
        assert_eq!(
            content_path_at(&root, &directory, &item, &mut |_| Ok(())).unwrap(),
            expected
        );
        assert_eq!(content_path(&directory, &item).unwrap(), expected);
    }
}

#[test]
fn rooted_container_lookup_rejects_unsafe_private_directory_without_chmod() {
    use platform_io::fs_safety::Error as FsError;
    use std::os::unix::fs::PermissionsExt;

    let (_temporary, disk, item, directory) = verified_existing_fixture();
    let installed = install_existing(&directory, &item).unwrap();
    let named = installed.parent().unwrap().parent().unwrap();
    let root = DiskRoot::open(&disk.root).unwrap();
    fs::set_permissions(named, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(matches!(
        content_path_at(&root, &directory, &item, &mut |_| Ok(())),
        Err(Error::Filesystem(FsError::UnsafePermissions(rejected))) if rejected == named
    ));
    assert_eq!(fs::metadata(named).unwrap().mode() & 0o7777, 0o755);
    assert_eq!(fs::read(installed).unwrap(), b"verified existing body");
}

#[test]
fn rooted_prepared_lookup_preserves_existing_parent_mode_contract() {
    use std::os::unix::fs::PermissionsExt;

    let (_temporary, disk, item, directory) = verified_existing_fixture();
    let root = DiskRoot::open(&disk.root).unwrap();
    for parent in [
        disk.root.join("histories"),
        directory.parent().unwrap().to_path_buf(),
        directory.clone(),
        directory.join("content"),
    ] {
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o755)).unwrap();
        let content = content_path_at(&root, &directory, &item, &mut |_| Ok(())).unwrap();
        assert_eq!(content, directory.join("content/body"));
        assert_eq!(
            root.read_bytes(&content, MAX_METADATA).unwrap(),
            b"verified existing body"
        );
        assert_eq!(fs::metadata(parent).unwrap().mode() & 0o7777, 0o755);
    }
}

#[test]
fn rooted_installation_retry_preserves_missing_prepared_parent_classification() {
    for missing in ["content", "version", "versions"] {
        let (_temporary, disk, item, directory) = verified_existing_fixture();
        let installed = install_existing(&directory, &item).unwrap();
        let root = DiskRoot::open(&disk.root).unwrap();
        let proof = VerifiedExisting::read_at(&root, &directory, &item).unwrap();
        let absent = match missing {
            "content" => directory.join("content"),
            "version" => directory.clone(),
            "versions" => directory.parent().unwrap().to_path_buf(),
            _ => unreachable!(),
        };
        fs::remove_dir_all(&absent).unwrap();
        assert_eq!(
            install_version_content_at(&root, &directory, PreparedContent::Verified(proof))
                .unwrap(),
            installed,
            "{missing}"
        );
        assert!(
            !absent.exists(),
            "retry must not recreate missing preparation"
        );
        assert_eq!(fs::read(installed).unwrap(), b"verified existing body");
    }
}

#[test]
fn rooted_installation_moves_the_verified_inode_without_touching_replacement_root() {
    let (_temporary, disk, item, directory) = verified_existing_fixture();
    let source = directory.join("content/body");
    let before = fs::metadata(&source).unwrap();
    let verified = VerifiedExisting::read(&directory, &item).unwrap();
    let root = DiskRoot::open(&disk.root).unwrap();
    let moved = disk.root.with_extension("held");
    fs::rename(&disk.root, &moved).unwrap();
    private_dir(&disk.root).unwrap();
    write_atomic(&disk.root.join("replacement"), b"unchanged", false).unwrap();

    let installed =
        install_version_content_at(&root, &directory, PreparedContent::Verified(verified)).unwrap();
    let after = root.open_regular(&installed).unwrap().metadata().unwrap();
    assert_eq!((before.dev(), before.ino()), (after.dev(), after.ino()));
    assert_eq!(after.nlink(), 1);
    let moved_source = moved.join(source.strip_prefix(&disk.root).unwrap());
    assert!(!moved_source.exists());
    let moved_target = moved.join(installed.strip_prefix(&disk.root).unwrap());
    assert_eq!(fs::read(moved_target).unwrap(), b"verified existing body");
    assert_eq!(fs::read_dir(&disk.root).unwrap().count(), 1);
    assert_eq!(
        fs::read(disk.root.join("replacement")).unwrap(),
        b"unchanged"
    );
}

#[test]
fn prepared_names_apply_to_the_resource_captured_before_root_replacement() {
    let temporary = tempfile::tempdir().unwrap();
    let config =
        ValidatedConfig::try_from(Config::development([temporary.path().join("disk")])).unwrap();
    drop(Engine::initialize_validated(&temporary.path().join("system"), config.clone()).unwrap());
    let resources = Arc::new(DiskResources::open(&config, true).unwrap());
    let disk = &config.disks[0];
    let history_id = new_id("history");
    let item = version(
        &State::default(),
        &history_id,
        &new_id("file"),
        "body",
        b"original body",
    );
    let history = disk.root.join("histories").join(&history_id);
    let directory = prepare(&history, &item, b"original body");
    let body = install_existing(&directory, &item).unwrap();
    let inode = fs::metadata(&body).unwrap().ino();
    let previous = with_version(&item);
    let mut next = previous.clone();
    next.file_names
        .insert(item.file_id.clone(), "renamed".into());
    next.file_locations
        .insert(item.file_id.clone(), "nested/renamed".into());
    let projection = ProjectionTransition::new(&previous, &next, None).unwrap();
    let prepared = prepare_names(&config, &resources, &projection, MAX_METADATA).unwrap();

    let moved = disk.root.with_extension("held");
    fs::rename(&disk.root, &moved).unwrap();
    private_dir(&disk.root).unwrap();
    write_atomic(&disk.root.join("replacement"), b"unchanged", false).unwrap();
    prepared.apply().unwrap();

    let held = resources.get(disk).unwrap();
    let resolved = content_path_at(&held.root, &directory, &item, &mut |_| Ok(())).unwrap();
    assert!(
        resolved
            .to_string_lossy()
            .contains("/namespace/nested/renamed/versions/")
    );
    assert_eq!(
        held.root
            .open_regular(&resolved)
            .unwrap()
            .metadata()
            .unwrap()
            .ino(),
        inode
    );
    let named = resolved.parent().unwrap().parent().unwrap();
    let descriptor: json_codec::Value = held
        .root
        .read_json_observed(&named.join("container.json"), &mut |_| Ok(()))
        .unwrap();
    assert_eq!(descriptor["name"], "renamed");
    assert_eq!(descriptor["location"], "nested/renamed");
    assert_eq!(fs::read_dir(&disk.root).unwrap().count(), 1);
    assert_eq!(
        fs::read(disk.root.join("replacement")).unwrap(),
        b"unchanged"
    );
}

#[test]
fn verified_existing_installs_staged_inode_and_reuses_installed_target() {
    let (_temporary, _disk, item, directory) = verified_existing_fixture();
    let source = directory.join("content/body");
    let before = fs::metadata(&source).unwrap();
    let proof = VerifiedExisting::read(&directory, &item).unwrap();
    let target = install_version_content(&directory, PreparedContent::Verified(proof)).unwrap();
    let after = fs::metadata(&target).unwrap();
    assert_eq!((before.dev(), before.ino()), (after.dev(), after.ino()));
    assert_eq!(after.nlink(), 1);
    let proof = VerifiedExisting::read(&directory, &item).unwrap();
    assert_eq!(
        install_version_content(&directory, PreparedContent::Verified(proof)).unwrap(),
        target
    );
    assert_eq!(fs::read(target).unwrap(), b"verified existing body");
}

#[test]
fn verified_existing_rejects_replaced_or_missing_body() {
    for change in ["leaf", "parent", "disk", "missing"] {
        let (_temporary, disk, item, directory) = verified_existing_fixture();
        let target = install_existing(&directory, &item).unwrap();
        let proof = VerifiedExisting::read(&directory, &item).unwrap();
        match change {
            "leaf" => {
                fs::rename(&target, target.with_extension("held")).unwrap();
                write_atomic(&target, b"verified existing body", false).unwrap();
            }
            "parent" => {
                let parent = target.parent().unwrap();
                fs::rename(parent, parent.with_extension("held")).unwrap();
                private_dir(parent).unwrap();
                write_atomic(&target, b"verified existing body", false).unwrap();
            }
            "disk" => {
                fs::rename(&disk.root, disk.root.with_extension("held")).unwrap();
                private_dir(&disk.root).unwrap();
            }
            "missing" => fs::remove_file(&target).unwrap(),
            _ => unreachable!(),
        }
        assert!(
            install_version_content(&directory, PreparedContent::Verified(proof)).is_err(),
            "{change}"
        );
    }
}

#[test]
#[should_panic(
    expected = "invariant: existing proof belongs to its admitted version and directory"
)]
fn existing_proof_exposes_internal_version_misbinding() {
    let (_temporary, _disk, item, directory) = verified_existing_fixture();
    let proof = VerifiedExisting::read(&directory, &item).unwrap();
    proof.assert_binding(&directory, &item.clone());
}

#[test]
fn verified_existing_staged_proof_does_not_authorize_new_corrupt_target() {
    let (_temporary, _disk, item, directory) = verified_existing_fixture();
    let proof = VerifiedExisting::read(&directory, &item).unwrap();
    let target = install_existing(&directory, &item).unwrap();
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(&target, fs::Permissions::from_mode(0o600)).unwrap();
    fs::write(&target, b"modified existing body").unwrap();
    assert_eq!(
        install_version_content(&directory, PreparedContent::Verified(proof))
            .unwrap_err()
            .code(),
        ErrorCode::ContentCorrupt
    );
}

#[test]
#[ignore = "requires test/engine/unit/container_storage/fsync_eio.c as LD_PRELOAD"]
fn existing_body_sync_io_failure_retains_its_cause() {
    let (_temporary, _disk, item, directory) = verified_existing_fixture();
    // The interposer targets installed bodies only: staged-file and directory
    // synchronization still execute normally before the retry under test.
    let target = install_existing(&directory, &item).unwrap();
    fs::File::open(target.parent().unwrap())
        .unwrap()
        .sync_all()
        .unwrap();
    write_atomic(&directory.join("unrelated"), b"unrelated", false).unwrap();
    let proof = VerifiedExisting::read(&directory, &item).unwrap();
    let error = install_version_content(&directory, PreparedContent::Verified(proof)).unwrap_err();
    let Error::Contract(storage_contract::Error::Io { source, .. }) = error else {
        panic!("file sync failure must retain its I/O source: {error}");
    };
    assert_eq!(source.raw_os_error(), Some(5));
    assert_eq!(fs::read(target).unwrap(), b"verified existing body");
    assert_eq!(fs::read(directory.join("unrelated")).unwrap(), b"unrelated");
}

#[test]
fn system_clock_version_time_validates_calendar_boundary() {
    use std::{
        error::Error as _,
        time::{Duration, UNIX_EPOCH},
    };
    assert_eq!(
        version_time(UNIX_EPOCH).unwrap().format(&Rfc3339).unwrap(),
        "1970-01-01T00:00:00Z"
    );
    let last = UNIX_EPOCH + Duration::new(253_402_300_799, 999_999_999);
    assert_eq!(
        version_time(last).unwrap().format(&Rfc3339).unwrap(),
        "9999-12-31T23:59:59Z"
    );
    let error = version_time(last + Duration::from_nanos(1)).unwrap_err();
    assert_eq!(error.code(), storage_contract::ErrorCode::IoError);
    assert!(matches!(error, Error::ClockRange(_)));
    assert!(
        error
            .source()
            .unwrap()
            .is::<date_time::error::ComponentRange>()
    );
    assert!(matches!(
        version_time(UNIX_EPOCH - Duration::from_nanos(1)),
        Err(Error::Clock(_))
    ));
}
