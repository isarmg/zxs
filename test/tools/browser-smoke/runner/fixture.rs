//! Test-only persisted file fixture. No product upload/download CLI is required.
//! The caller owns a fresh private TempDir; never accept a user pool path.
use super::{Read, Result};
use storage_engine::{CommitRequest, Engine};

const PRINCIPAL: &str = "smoke-admin";
const FILE: &str = "browser-smoke.txt";
const FILE_BRANCH: &str = "browser-smoke-alternate";
const PAYLOADS: [&str; 2] = ["first real version\n", "second real version: 测试\n"];
pub(super) const NAMING_REPOSITORY: &str = "browser-naming";
pub(super) const DIRECTORY_REPOSITORY: &str = "browser-directory-history";
pub(super) const DIRECTORY_PATH: &str = "nested-history";
pub(super) const DIRECTORY_PREVIEW: &str = "readonly-history-preview";

/// Use the same explicit authority publication as production setup. A pool
/// credential snapshot alone is deliberately insufficient for daemon startup.
pub(super) fn initialize_management(temporary: &tempfile::TempDir, password: &str) -> Result<()> {
    use super::{fs, wait_for};
    use std::{
        collections::BTreeMap,
        os::unix::fs::DirBuilderExt,
        sync::{Arc, Mutex},
    };
    use storage_server::Backend;

    let root = temporary.path();
    let pool = root.join("pool");
    let management = pool.join("management");
    fs::DirBuilder::new().mode(0o700).create(&pool)?;
    fs::DirBuilder::new().mode(0o700).create(&management)?;
    let discovery = root.join("disks");
    fs::DirBuilder::new().mode(0o700).create(&discovery)?;
    let mut disks = BTreeMap::new();
    for slot in ["P01", "B01", "C01"] {
        let path = discovery.join(slot);
        fs::DirBuilder::new().mode(0o700).create(&path)?;
        disks.insert(slot.to_owned(), path);
    }
    let (backend, auth) = storage_server::management::open_service_with_options(
        &pool,
        &management,
        storage_server::management::ManagementOptions {
            allow_development_setup: true,
            ..Default::default()
        },
    )?;
    let auth = Arc::new(Mutex::new(auth));
    backend.start_management(auth.clone());
    wait_for("fixture initial storage discovery", || {
        if !backend.management_status().recovering() {
            Ok(())
        } else {
            Err("fixture discovery still running".into())
        }
    })?;
    backend.setup_operation(
        "initialize",
        json_codec::json!({"username":PRINCIPAL,"password":password}),
        None,
        None,
    )?;
    let identity = {
        let mut auth = auth.lock().unwrap();
        let login = auth.login(PRINCIPAL, password)?;
        auth.authenticate(&login.token)?
    };
    backend.setup_operation(
        "set_discovery_root",
        json_codec::json!({"expected_discovery_root":"/zxs/data/disks","discovery_root":discovery}),
        None,
        Some(&identity),
    )?;
    let mut selections = Vec::new();
    // Preserve the declared primary/backup order; BTreeMap's sort is not topology.
    for slot in ["P01", "B01", "C01"] {
        let root_path = disks[slot].to_str().ok_or("fixture root is not UTF-8")?;
        let listing = storage_server::storage_directories::browse_within(
            discovery.to_str().ok_or("non-UTF8 discovery root")?,
            root_path,
        )?;
        selections.push(storage_server::storage_directories::inspect_within(
            discovery.to_str().ok_or("non-UTF8 discovery root")?,
            &storage_engine::storage_directories::InspectRequest {
                mount_path: listing.mount_path,
                root_path: root_path.into(),
            },
            &[pool.clone(), management.clone()],
        )?);
    }
    backend.setup_operation(
        "activate",
        json_codec::json!({"selections":selections,"mode":"development"}),
        None,
        Some(&identity),
    )?;
    wait_for("fixture explicit management initialization", || {
        let status = backend.management_status();
        if status.state() == storage_server::StorageState::Ready && !status.recovering() {
            Ok(())
        } else {
            Err("fixture management not ready".into())
        }
    })?;
    drop(backend);
    drop(auth);
    // Background observation can briefly retain the engine after its public
    // handle is dropped. Reopening verifies durable state and lock release.
    drop(wait_for("fixture initialization lock release", || {
        Ok(Engine::open(&pool)?)
    })?);
    Ok(())
}

pub(super) fn seed_files(temporary: &tempfile::TempDir) -> Result<()> {
    let pool = temporary.path().join("pool");
    let mut engine = Engine::open(&pool)?;
    if !engine.state().repositories.is_empty() {
        return Err("browser fixture requires a fresh disposable pool with no repositories".into());
    }
    let repository = engine.create_repository(
        &storage_engine::RepositoryName::parse("browser-fixtures").unwrap(),
        "P01",
        3,
        PRINCIPAL,
        storage_engine::OperationId::generate().unwrap(),
    )?;
    // Real directory history must exist before the content writes, so the
    // browser can verify actual parent connectors rather than a lone snapshot.
    // This is an explicit mutation of this private, test-owned pool only.
    engine.initialize_directory_scope(
        &repository.repository_id,
        None,
        PRINCIPAL,
        storage_engine::OperationId::generate().unwrap(),
    )?;
    for payload in PAYLOADS {
        let workspace = engine.create_workspace(&repository.main_branch_id, PRINCIPAL)?;
        engine.stage_bytes(&workspace.workspace_id, FILE, payload.as_bytes())?;
        engine.commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_engine::OperationId::generate().unwrap(),
            principal: PRINCIPAL.into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "Browser metadata fixture".into(),
            request_digest: None,
        })?;
    }
    let entries = engine.list_tree(&repository.main_branch_id)?;
    let [entry] = entries.as_slice() else {
        return Err("fixture must contain exactly one file before branching".into());
    };
    // A directory commit alone does not create independent file branches. Bind
    // this pending branch to a real node without adding a version or graph node.
    engine.create_file_branch_from_node(
        &entry.file_id,
        &entry.version_id,
        &entry.version_id,
        &storage_engine::BranchName::parse(FILE_BRANCH)?,
        PRINCIPAL,
        storage_engine::OperationId::generate().unwrap(),
    )?;
    seed_naming_scope(&mut engine)?;
    seed_directory_history(&mut engine)?;
    // Reopen from disk before serving: an in-memory-only fixture is insufficient.
    drop(engine);
    let engine = Engine::open(&pool)?;
    let entries = engine.list_tree(&repository.main_branch_id)?;
    let [entry] = entries.as_slice() else {
        return Err("fixture must persist exactly one file".into());
    };
    if entry.path != FILE || engine.file_history(&entry.file_id)?.len() != 2 {
        return Err("fixture file path or two-version history did not survive reopen".into());
    }
    let mut content = engine.open_content(&entry.version_id)?;
    let mut bytes = Vec::new();
    content.file.read_to_end(&mut bytes)?;
    if bytes != PAYLOADS[1].as_bytes() {
        return Err("fixture current bytes did not survive reopen".into());
    }
    Ok(())
}

// Naming exercises explicit registration from an empty directory-branch scope.
// Keep it separate from the already registered graph/parent-edge fixture.
fn seed_naming_scope(engine: &mut Engine) -> Result<()> {
    let repository = engine.create_repository(
        &storage_engine::RepositoryName::parse(NAMING_REPOSITORY)?,
        "P01",
        3,
        PRINCIPAL,
        storage_engine::OperationId::generate().unwrap(),
    )?;
    let workspace = engine.create_workspace(&repository.main_branch_id, PRINCIPAL)?;
    engine.stage_bytes(&workspace.workspace_id, FILE, PAYLOADS[0].as_bytes())?;
    engine.commit_workspace(CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: storage_engine::OperationId::generate().unwrap(),
        principal: PRINCIPAL.into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "Browser naming registration fixture".into(),
        request_digest: None,
    })?;
    Ok(())
}

// A real parent preview is needed to select a read-only child directory. The
// disk-root listing itself is never a directory-branch preview.
fn seed_directory_history(engine: &mut Engine) -> Result<()> {
    let repository = engine.create_repository(
        &storage_engine::RepositoryName::parse(DIRECTORY_REPOSITORY)?,
        "P01",
        3,
        PRINCIPAL,
        storage_engine::OperationId::generate()?,
    )?;
    let path = format!("{DIRECTORY_PATH}/{FILE}");
    let initial = write_directory_history_version(
        engine,
        &repository.main_branch_id,
        &path,
        "initial directory version\n",
    )?;
    let directory = initial
        .path
        .parent()
        .expect("invariant: the history fixture file is nested")
        .into();
    engine.initialize_directory_scope(
        &repository.repository_id,
        Some(&directory),
        PRINCIPAL,
        storage_engine::OperationId::generate()?,
    )?;
    for payload in PAYLOADS {
        write_directory_history_version(engine, &repository.main_branch_id, &path, payload)?;
    }
    // Both child labels reference the same real head; registration invents no
    // extra history node. Capture that child in the parent's real preview.
    engine.create_directory_branches(
        &repository.repository_id,
        Some(&directory),
        &storage_engine::BranchName::parse("shared-history-head")?,
        PRINCIPAL,
        storage_engine::OperationId::generate()?,
    )?;
    engine.create_directory_branches(
        &repository.repository_id,
        None,
        &storage_engine::BranchName::parse(DIRECTORY_PREVIEW)?,
        PRINCIPAL,
        storage_engine::OperationId::generate()?,
    )?;
    Ok(())
}

fn write_directory_history_version(
    engine: &mut Engine,
    branch: &str,
    path: &str,
    payload: &str,
) -> Result<storage_engine::StagedFile> {
    let workspace = engine.create_workspace(branch, PRINCIPAL)?;
    let staged = engine.stage_bytes(&workspace.workspace_id, path, payload.as_bytes())?;
    engine.commit_workspace(CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: storage_engine::OperationId::generate()?,
        principal: PRINCIPAL.into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "Real read-only directory history fixture".into(),
        request_digest: None,
    })?;
    Ok(staged)
}

#[cfg(test)]
#[path = "../unit/fixture_tests.rs"]
mod tests;
