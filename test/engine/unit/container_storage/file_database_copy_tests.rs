use super::*;
use crate::*;
use std::os::unix::fs::{MetadataExt, PermissionsExt};

struct Fixture {
    temp: tempfile::TempDir,
    config: Config,
    engine: Engine,
    repository: Repository,
    entry: Entry,
}

impl Fixture {
    fn new() -> Self {
        Self::with_copies(3, true)
    }

    fn with_copies(copies: u32, primary_metadata: bool) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let mut config = Config::development([
            (name_policy::Name::parse("Primary").unwrap(), temp.path().join("primary")),
            (name_policy::Name::parse("Near archive").unwrap(), temp.path().join("backup")),
            (name_policy::Name::parse("Far archive").unwrap(), temp.path().join("third")),
        ])
        .unwrap();
        config.disks[0].metadata_member = primary_metadata;
        let mut engine =
            Engine::initialize(&temp.path().join("system"), config.clone()).unwrap();
        let repository = engine
            .create_repository(
                &storage_contract::RepositoryName::parse("copy image").unwrap(),
                "P01",
                copies,
                "admin",
                OperationId::generate().unwrap(),
            )
            .unwrap();
        let workspace = engine
            .create_workspace(&repository.main_branch_id, "admin")
            .unwrap();
        engine
            .stage_bytes(&workspace.workspace_id, "file.txt", b"retained body")
            .unwrap();
        engine
            .commit_workspace(CommitRequest {
                workspace_id: workspace.workspace_id,
                operation_id: OperationId::generate().unwrap(),
                principal: "admin".into(),
                expected_head: workspace.base_head,
                fencing_token: workspace.fencing_token,
                message: "three physical copies".into(),
                request_digest: None,
            })
            .unwrap();
        let entry = engine.list_tree(&repository.main_branch_id).unwrap()[0].clone();
        Self {
            temp,
            config,
            engine,
            repository,
            entry,
        }
    }

    fn container(&self, disk: &DiskConfig) -> std::path::PathBuf {
        let root = &self.engine.disk_resources.get(disk).unwrap().root;
        let history = disk.root.join("histories").join(&self.repository.history_tree_id);
        crate::container_storage::container_at(
            root,
            &history,
            Some(&self.repository.history_tree_id),
            &self.entry.file_id,
            &mut |_| Ok(()),
        )
        .unwrap()
        .unwrap()
    }

    fn database_bytes(&self) -> Vec<Vec<u8>> {
        self.config
            .disks
            .iter()
            .map(|disk| {
                std::fs::read(
                    self.container(disk).join(crate::container_storage::file_database::NAME),
                )
                .unwrap()
            })
            .collect()
    }
}

fn reference_bytes(state: &State, file: &str, history: &str) -> Vec<u8> {
    let projection = crate::state_projection::ProjectionSnapshot::new(state);
    let records = crate::container_storage::file_database::validated_records(
        projection.view(), file, history,
    )
    .unwrap()
    .records;
    let hash = storage_contract::object_hash("file-local-records", &records).unwrap();
    crate::container_storage::file_database::SCHEMA
        .encode(records, [file, history], state.sequence, &hash)
        .unwrap()
        .bytes
}

#[test]
fn ordinary_three_copy_publication_preserves_exact_file_bytes_and_body_inodes() {
    let mut fixture = Fixture::new();
    let before = fixture.engine.state().clone();
    let version = before.versions[&fixture.entry.version_id].clone();
    let bodies = fixture
        .config
        .disks
        .iter()
        .map(|disk| {
            let directory = crate::disk::version_dir(
                disk,
                &fixture.repository.history_tree_id,
                &fixture.entry.version_id,
            );
            let path = crate::container_storage::content_path(&directory, &version).unwrap();
            let metadata = std::fs::metadata(&path).unwrap();
            (path, metadata.dev(), metadata.ino())
        })
        .collect::<Vec<_>>();
    fixture
        .engine
        .set_file_note(
            &fixture.entry.file_id,
            Some(&fixture.entry.version_id),
            &Note::parse("one material, three targets".into()).unwrap(),
            "admin",
            OperationId::generate().unwrap(),
        )
        .unwrap();
    let expected = reference_bytes(
        fixture.engine.state(),
        &fixture.entry.file_id,
        &fixture.repository.history_tree_id,
    );
    assert_eq!(fixture.database_bytes(), vec![expected.clone(); 3]);
    let projection = crate::state_projection::ProjectionTransition::new(
        &before,
        fixture.engine.state(),
        None,
    )
    .unwrap();
    let names = crate::container_storage::prepare_names(
        &fixture.engine.config,
        &fixture.engine.disk_resources,
        &projection,
        crate::disk::MAX_METADATA,
    )
    .unwrap();
    let items = &names.cache.as_ref().unwrap().items;
    assert_eq!(items.len(), 3);
    assert_eq!(
        items
            .iter()
            .map(|item| item.database.as_ref().unwrap().byte_len())
            .sum::<usize>(),
        expected.len() * 3,
    );
    drop(names);
    drop(projection);
    for (path, device, inode) in &bodies {
        let metadata = std::fs::metadata(path).unwrap();
        assert_eq!((metadata.dev(), metadata.ino()), (*device, *inode));
        assert_eq!(std::fs::read(path).unwrap(), b"retained body");
    }
    let system = fixture.temp.path().join("system");
    drop(fixture.engine);
    fixture.engine = Engine::open(&system).unwrap();
    assert_eq!(fixture.database_bytes(), vec![expected; 3]);
    for (path, device, inode) in bodies {
        let metadata = std::fs::metadata(&path).unwrap();
        assert_eq!((metadata.dev(), metadata.ino()), (device, inode));
        assert_eq!(std::fs::read(path).unwrap(), b"retained body");
    }
}

#[test]
fn shared_image_keeps_its_view_and_rechecks_each_target_before_staging() {
    let fixture = Fixture::new();
    let original = fixture.database_bytes();
    let mut state = fixture.engine.state().clone();
    state
        .file_notes
        .insert(fixture.entry.file_id.clone(), "captured rows".into());
    let projection = crate::state_projection::ProjectionSnapshot::new(&state);
    let paths = fixture
        .config
        .disks
        .iter()
        .map(|disk| fixture.container(disk))
        .collect::<Vec<_>>();
    let histories = fixture
        .config
        .disks
        .iter()
        .map(|disk| disk.root.join("histories").join(&fixture.repository.history_tree_id))
        .collect::<Vec<_>>();
    let roots = fixture
        .config
        .disks
        .iter()
        .map(|disk| &fixture.engine.disk_resources.get(disk).unwrap().root)
        .collect::<Vec<_>>();
    let admission = InstallAdmission::NewOperation;
    let (copies, mut first) = PreparedFileCopies::capture(
        roots[0],
        (&histories[0], &fixture.repository.history_tree_id),
        &paths[0],
        &fixture.entry.file_id,
        projection.view(),
        admission,
    )
    .unwrap();
    let expected = reference_bytes(
        &state,
        &fixture.entry.file_id,
        &fixture.repository.history_tree_id,
    );
    assert_eq!(copies.database.bytes(), expected);
    let mut unrelated = state.clone();
    unrelated
        .file_notes
        .insert(fixture.entry.file_id.clone(), "different logical rows".into());
    assert_ne!(
        reference_bytes(
            &unrelated,
            &fixture.entry.file_id,
            &fixture.repository.history_tree_id,
        ),
        expected,
    );

    // A predecessor-only history still has to fail after its physical capture.
    let wrong_history = storage_contract::HistoryTreeId::generate().unwrap().to_string();
    let fresh = CapturedFile::capture(
        roots[1],
        (&histories[1], &wrong_history),
        &paths[1],
        &fixture.entry.file_id,
        projection.view(),
    )
    .err()
    .unwrap();
    let shared = copies
        .prepare(roots[1], (&histories[1], &wrong_history), &paths[1], admission)
        .err()
        .unwrap();
    assert_eq!(fresh.code(), ErrorCode::ContainerLayoutInvalid);
    assert_eq!(shared.code(), fresh.code());

    // The second target's old FILE is read independently, even with cached rows.
    let second_file = paths[1].join(crate::container_storage::file_database::NAME);
    std::fs::write(&second_file, b"not sqlite").unwrap();
    let fresh = CapturedFile::capture(
        roots[1],
        (&histories[1], &fixture.repository.history_tree_id),
        &paths[1],
        &fixture.entry.file_id,
        projection.view(),
    )
    .unwrap()
    .prepare(admission)
    .err()
    .unwrap();
    let shared = copies
        .prepare(
            roots[1],
            (&histories[1], &fixture.repository.history_tree_id),
            &paths[1],
            admission,
        )
        .err()
        .unwrap();
    assert_eq!(fresh.code(), ErrorCode::ContainerLayoutInvalid);
    assert_eq!(shared.code(), fresh.code());
    std::fs::write(&second_file, &original[1]).unwrap();

    let mut second = copies
        .prepare(
            roots[1],
            (&histories[1], &fixture.repository.history_tree_id),
            &paths[1],
            admission,
        )
        .unwrap();
    let mut third = copies
        .prepare(
            roots[2],
            (&histories[2], &fixture.repository.history_tree_id),
            &paths[2],
            admission,
        )
        .unwrap();
    // One owner of the image plus the three real per-target installers.
    assert_eq!(Arc::strong_count(&copies.database.bytes), 4);
    assert_eq!(first.byte_len() + second.byte_len() + third.byte_len(), expected.len() * 3);
    first.stage_unpublished(roots[0], &paths[0], &mut |_| Ok(())).unwrap();
    third.stage_unpublished(roots[2], &paths[2], &mut |_| Ok(())).unwrap();
    first.verify_unpublished(roots[0], &paths[0]).unwrap();
    third.verify_unpublished(roots[2], &paths[2]).unwrap();

    let retained = fixture.temp.path().join("retained-second-container");
    std::fs::rename(&paths[1], &retained).unwrap();
    std::fs::create_dir(&paths[1]).unwrap();
    std::fs::set_permissions(&paths[1], std::fs::Permissions::from_mode(0o700)).unwrap();
    assert_eq!(
        second
            .stage_unpublished(roots[1], &paths[1], &mut |_| Ok(()))
            .unwrap_err()
            .code(),
        ErrorCode::ContainerLayoutInvalid,
    );
    assert!(!paths[1].join(crate::container_storage::file_database::NAME).exists());
    assert_eq!(
        std::fs::read(retained.join(crate::container_storage::file_database::NAME)).unwrap(),
        original[1],
    );
    for index in [0, 2] {
        assert_eq!(
            std::fs::read(paths[index].join(crate::container_storage::file_database::NAME))
                .unwrap(),
            original[index],
        );
    }
    assert_eq!(copies.database.bytes(), expected);
}

#[test]
fn skipped_recovery_roots_do_not_capture_or_encode_file_material() {
    // Leave both metadata voters online, but make this FILE's only data root
    // genuinely unavailable at cold admission. Final-state repair may skip it;
    // a candidate (even decided recovery) still requires its selected root.
    let mut fixture = Fixture::with_copies(1, false);
    let primary = &fixture.config.disks[0];
    let database = fixture
        .container(primary)
        .join(crate::container_storage::file_database::NAME);
    let relative_database = database.strip_prefix(&primary.root).unwrap().to_path_buf();
    let original = std::fs::read(&database).unwrap();
    let identity = std::fs::metadata(&database).unwrap();
    let previous = fixture.engine.state().clone();
    let system = fixture.temp.path().join("system");
    let parked = fixture.temp.path().join("parked-primary");
    drop(fixture.engine);
    std::fs::rename(&primary.root, &parked).unwrap();
    fixture.engine = Engine::open(&system).unwrap();
    assert!(!fixture.engine.is_read_only());
    assert_eq!(fixture.engine.state(), &previous);
    assert!(fixture.engine.disk_resources.available(primary).is_none());
    for disk in fixture.config.disks.iter().filter(|disk| disk.metadata_member) {
        assert!(fixture.engine.disk_resources.available(disk).is_some());
    }

    // This sentinel would fail canonical capture. It is not published.
    let mut next = fixture.engine.state().clone();
    next.file_notes.insert(
        fixture.entry.file_id.clone(),
        "x".repeat(crate::container_storage::file_database::projection::MAX_RECORD_BYTES + 1),
    );
    let projection = crate::state_projection::ProjectionTransition::new(
        fixture.engine.state(),
        &next,
        None,
    )
    .unwrap();
    assert_eq!(
        CapturedMaterial::from_projection(
            projection.next(),
            &fixture.entry.file_id,
            &fixture.repository.history_tree_id,
        )
        .err()
        .unwrap()
        .code(),
        ErrorCode::ResourceLimit,
    );
    let ordinary = crate::container_storage::prepare_names(
        &fixture.engine.config,
        &fixture.engine.disk_resources,
        &projection,
        crate::disk::MAX_METADATA,
    )
    .err()
    .unwrap();
    assert_eq!(ordinary.code(), ErrorCode::DiskUnavailable);
    let candidate = crate::container_storage::prepare_recovered_names(
        &fixture.engine.config,
        &fixture.engine.disk_resources,
        &projection,
        crate::disk::MAX_METADATA,
    )
    .err()
    .unwrap();
    assert_eq!(candidate.code(), ErrorCode::DiskUnavailable);

    let repaired = crate::container_storage::prepare_layout_repair(
        &fixture.engine.config,
        &fixture.engine.disk_resources,
        &next,
        None,
        crate::database_install::InstallAdmission::DecidedRecovery,
    )
    .unwrap();
    assert!(repaired.cache.is_none());
    assert!(repaired.database_scopes.is_empty());
    assert!(!primary.root.exists());
    let retained_database = parked.join(relative_database);
    let retained_identity = std::fs::metadata(&retained_database).unwrap();
    assert_eq!(
        (retained_identity.dev(), retained_identity.ino()),
        (identity.dev(), identity.ino()),
    );
    assert_eq!(std::fs::read(retained_database).unwrap(), original);
}
