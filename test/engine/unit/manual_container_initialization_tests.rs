use crate::{CommitRequest, Config, Engine, OperationId};
use std::{collections::BTreeMap, fs, io::Read, os::unix::fs::{MetadataExt, PermissionsExt}};

fn roots() -> (tempfile::TempDir, std::path::PathBuf, Config) {
    let temp = tempfile::tempdir().unwrap();
    let system = temp.path().join("system");
    let disk = temp.path().join("disk");
    for path in [&system, &disk] {
        fs::create_dir(path).unwrap();
        fs::set_permissions(path, fs::Permissions::from_mode(0o700)).unwrap();
    }
    let config = Config::development([disk]).unwrap();
    (temp, system, config)
}

fn image(path: &std::path::Path) -> BTreeMap<std::path::PathBuf, (u64, u32, Vec<u8>)> {
    fn visit(root: &std::path::Path, path: &std::path::Path,
        values: &mut BTreeMap<std::path::PathBuf, (u64, u32, Vec<u8>)>) {
        let metadata = fs::symlink_metadata(path).unwrap();
        values.insert(path.strip_prefix(root).unwrap().to_owned(), (
            metadata.ino(), metadata.mode(),
            if metadata.is_file() { fs::read(path).unwrap() } else { Vec::new() },
        ));
        if metadata.is_dir() {
            for entry in fs::read_dir(path).unwrap() {
                visit(root, &entry.unwrap().path(), values);
            }
        }
    }
    let mut values = BTreeMap::new();
    visit(path, path, &mut values);
    values
}

#[test]
fn fresh_manual_format_cold_reads_real_content_and_retries_original_initialization() {
    let (temp, system, config) = roots();
    let operation = OperationId::generate().unwrap();
    let mut engine = Engine::initialize_manual_container_storage(&system, config.clone(), operation)
        .unwrap();
    let repo = engine.create_repository(
        &storage_contract::RepositoryName::parse("files").unwrap(), "P01", 1, "alice",
        OperationId::generate().unwrap(),
    ).unwrap();
    let workspace = engine.create_workspace(&repo.main_branch_id, "alice").unwrap();
    engine.stage_bytes(&workspace.workspace_id, "note.txt", b"new format bytes").unwrap();
    engine.commit_workspace(CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: OperationId::generate().unwrap(),
        principal: "alice".into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "first content".into(), request_digest: None,
    }).unwrap();
    let entry = engine.list_tree(&repo.main_branch_id).unwrap().remove(0);
    let decoded = crate::ValidatedConfig::try_from(config.clone()).unwrap();
    assert!(decoded.fresh_bootstrap().is_none());
    assert!(decoded.fresh_completion().is_none());
    let selector = fs::read(config.disks[0].root.join(
        crate::authority_selection::FRESH_DISK_SELECTOR,
    )).unwrap();
    let seal = crate::disk::decode_seal(&fs::read(config.disks[0].root.join(
        crate::persist::layout::DISK_IDENTITY.as_path(),
    )).unwrap()).unwrap();
    let before = image(temp.path());
    let admitted = crate::engine::container_drafts::format::admit_saved_genesis(
        decoded.clone(), &engine.config, &seal, &selector,
    ).unwrap();
    assert!(admitted.fresh_completion().is_some());
    assert_eq!(admitted.fresh_bootstrap(), engine.config.fresh_bootstrap());
    assert!(crate::engine::container_drafts::format::admit_saved_genesis(
        decoded.clone(), &decoded, &seal, &selector,
    ).is_err());
    let mut wrong_seal = seal.clone();
    wrong_seal.format = crate::disk::DiskSealFormat::ManagedDirectory;
    assert!(crate::engine::container_drafts::format::admit_saved_genesis(
        decoded, &engine.config, &wrong_seal, &selector,
    ).is_err());
    assert_eq!(image(temp.path()), before);
    drop(engine);
    let engine = Engine::open(&system).unwrap();
    let mut content = engine.open_content(&entry.version_id).unwrap();
    let mut bytes = Vec::new();
    content.file.read_to_end(&mut bytes).unwrap();
    assert_eq!(bytes, b"new format bytes");
    drop(content);
    drop(engine);
    let engine = Engine::initialize_manual_container_storage(&system, config, operation).unwrap();
    assert_eq!(engine.list_tree(&repo.main_branch_id).unwrap()[0].version_id, entry.version_id);
    let selector_path = engine.config.disks[0].root.join(
        crate::authority_selection::FRESH_DISK_SELECTOR,
    );
    drop(engine);
    let mut changed_selector = selector;
    changed_selector.push(b' ');
    fs::write(selector_path, changed_selector).unwrap();
    let before = image(temp.path());
    assert!(Engine::open(&system).is_err());
    assert_eq!(image(temp.path()), before);
}

#[test]
fn old_or_unknown_roots_are_not_upgraded_or_repaired() {
    let (temp, system, config) = roots();
    let mut old = Engine::initialize_storage(
        &system, config.clone(), OperationId::generate().unwrap(),
    ).unwrap();
    let repo = old.create_repository(
        &storage_contract::RepositoryName::parse("old files").unwrap(), "P01", 1, "alice",
        OperationId::generate().unwrap(),
    ).unwrap();
    let workspace = old.create_workspace(&repo.main_branch_id, "alice").unwrap();
    old.stage_bytes(&workspace.workspace_id, "note.txt", b"original history").unwrap();
    old.commit_workspace(CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: OperationId::generate().unwrap(),
        principal: "alice".into(), expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token, message: "original".into(), request_digest: None,
    }).unwrap();
    let entry = old.list_tree(&repo.main_branch_id).unwrap().remove(0);
    let file = entry.file_id.parse().unwrap();
    let admitted_base = old.current_container_draft_base(
        file, repo.repository_id.parse().unwrap(), entry.entry_id.parse().unwrap(),
    ).unwrap();
    old.config = crate::ValidatedConfig::try_from(config.clone()).unwrap();
    let principal = storage_contract::Principal::new("alice".into()).unwrap();
    let source = temp.path().join("retained-draft-input");
    fs::write(&source, b"unpublished").unwrap();
    let revision = storage_contract::container_edit::DraftRevision {
        operation: OperationId::generate().unwrap(),
        expected_generation: storage_contract::Generation::new(0),
        principal: principal.clone(),
        base: admitted_base,
        content_hash: storage_contract::ContentHash::from_digest(blake3::hash(b"unpublished")),
        size: 11,
    };
    let before = image(temp.path());
    let state = old.state().clone();
    for result in [
        old.set_container_commit_mode(&storage_contract::container_edit::SetCommitMode {
            file, expected_generation: storage_contract::Generation::new(0),
            mode: storage_contract::container_edit::CommitMode::Manual,
        }, &principal, OperationId::generate().unwrap()),
        old.save_container_draft(file, &revision, fs::File::open(&source).unwrap()),
        old.discard_container_draft(file, storage_contract::Generation::new(0), revision.operation,
            &principal, OperationId::generate().unwrap()),
    ] {
        assert_eq!(result.unwrap_err().code(), storage_contract::ErrorCode::FormatUnsupported);
        assert_eq!(old.state(), &state);
        assert_eq!(image(temp.path()), before);
    }
    drop(old);
    let before = image(temp.path());
    assert!(Engine::initialize_manual_container_storage(
        &system, config, OperationId::generate().unwrap(),
    ).is_err());
    assert_eq!(image(temp.path()), before);

    let (temp, system, config) = roots();
    fs::write(system.join("initialized.json"), b"unknown original format").unwrap();
    let before = image(temp.path());
    assert!(Engine::initialize_manual_container_storage(
        &system, config, OperationId::generate().unwrap(),
    ).is_err());
    assert_eq!(image(temp.path()), before);
}

#[test]
fn incomplete_or_replaced_writer_is_preserved_without_cold_sidecar_recreation() {
    for missing in ["complete", "system-writer", "bootstrap-writer", "writer-bytes"] {
        let (temp, system, config) = roots();
        let engine = Engine::initialize_manual_container_storage(
            &system, config.clone(), OperationId::generate().unwrap(),
        ).unwrap();
        drop(engine);
        if missing == "complete" {
            fs::remove_file(system.join(
                "fresh-directory-bootstrap-complete-v1.json",
            )).unwrap();
        } else if missing == "writer-bytes" {
            fs::write(system.join(crate::persist::layout::CONTROL_WRITER_LOCK.as_path()), b"unknown")
                .unwrap();
        } else {
            let parent = if missing == "bootstrap-writer" {
                system.join(crate::engine::disk_lifecycle::bootstrap::runtime::DIRECTORY)
            } else {
                system.clone()
            };
            let writer = parent.join(crate::persist::layout::CONTROL_WRITER_LOCK.as_path());
            fs::rename(&writer, parent.join("original-writer-retained")).unwrap();
            fs::write(&writer, b"").unwrap();
            fs::set_permissions(writer, fs::Permissions::from_mode(0o600)).unwrap();
        }
        let before = image(temp.path());
        assert!(Engine::open(&system).is_err());
        assert_eq!(image(temp.path()), before);
    }
}

#[test]
fn bootstrap_replacement_after_preflight_cannot_redirect_cold_writer_admission() {
    use platform_io::fs_safety::{PrivateDirectory, linux::{MountPolicy, OpenAt2Root}};
    use crate::engine::disk_lifecycle::bootstrap::{runtime, store::BootstrapStore};

    for replace_root in [true, false] {
        let (temp, system, config) = roots();
        let engine = Engine::initialize_manual_container_storage(
            &system, config.clone(), OperationId::generate().unwrap(),
        ).unwrap();
        drop(engine);
        // Capture the original completion commitment before adversarial replacement.
        let decoded = crate::ValidatedConfig::try_from(config.clone()).unwrap();
        let selection = crate::authority_selection::SelectionGuard::acquire(&system).unwrap()
            .recover_fresh(&decoded).unwrap();
        let admitted = decoded.with_fresh_bootstrap(selection.fresh_selection().unwrap()).unwrap();
        let proof = crate::engine::snapshot_initialization::bootstrap::require_complete(
            &system, &admitted,
        ).unwrap();
        let admitted = admitted.with_fresh_completion(proof);
        drop(selection);
        let bootstrap = system.join(runtime::DIRECTORY);
        if replace_root {
            fs::rename(&bootstrap, system.join("original-bootstrap-retained")).unwrap();
            fs::create_dir(&bootstrap).unwrap();
            fs::set_permissions(&bootstrap, fs::Permissions::from_mode(0o700)).unwrap();
            let before = image(temp.path());
            assert!(runtime::open_store_for_config(&system, &admitted).is_err());
            assert_eq!(image(temp.path()), before);
        } else {
            let system_handle = PrivateDirectory::open_existing(&system).unwrap();
            let bootstrap_handle = OpenAt2Root::open(&bootstrap, MountPolicy::SameMount).unwrap();
            let expected = crate::engine::snapshot_initialization::bootstrap::admit_bootstrap_handle(
                &system_handle, &admitted, bootstrap_handle.identity(),
            ).unwrap();
            let writer = bootstrap.join(crate::persist::layout::CONTROL_WRITER_LOCK.as_path());
            fs::rename(&writer, bootstrap.join("original-writer-retained")).unwrap();
            fs::write(&writer, b"").unwrap();
            fs::set_permissions(&writer, fs::Permissions::from_mode(0o600)).unwrap();
            let before = image(temp.path());
            assert!(BootstrapStore::hold_existing(
                bootstrap_handle.into_directory(), bootstrap, expected,
            ).is_err());
            assert_eq!(image(temp.path()), before);
        }
    }
}
