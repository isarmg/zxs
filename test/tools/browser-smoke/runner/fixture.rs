//! Test-only persisted file fixture. No product upload/download CLI is required.
//! The caller owns a fresh private TempDir; never accept a user pool path.
use super::{Read, Result};
use storage_engine::{CommitRequest, Engine};

const PRINCIPAL: &str = "smoke-admin";
const FILE: &str = "browser-smoke.txt";
const FILE_BRANCH: &str = "browser-smoke-alternate";
const PAYLOADS: [&str; 2] = ["first real version\n", "second real version: 测试\n"];
pub(super) const NAMING_REPOSITORY: &str = "browser-naming";

/// Build a declared three-tier test pool and publish a real management authority.
/// First activation now creates independent roots, so it must not be repurposed
/// or have its durable configuration rewritten to manufacture this graph.
pub(super) fn initialize_management(temporary: &tempfile::TempDir, password: &str) -> Result<()> {
    use super::{Write, fs, wait_for, with_cleanup};
    use std::{
        os::unix::fs::{DirBuilderExt, OpenOptionsExt},
        sync::{Arc, Mutex},
    };
    use storage_server::Backend;

    let root = temporary.path();
    let authority = root.join("authority-fixture");
    let authority_management = authority.join("management");
    for path in [&authority, &authority_management] {
        fs::DirBuilder::new().mode(0o700).create(path)?;
    }
    // Account creation remains the real, protected production transition. The
    // account-only source belongs to this TempDir and is never a supplied pool.
    let (backend, auth) =
        storage_server::management::open_service(&authority, &authority_management)?;
    let auth = Arc::new(Mutex::new(auth));
    backend.start_management(auth.clone());
    with_cleanup(
        || {
            wait_for("fixture account storage observation", || {
                if !backend.management_status().recovering() {
                    Ok(())
                } else {
                    Err("fixture account discovery still running".into())
                }
            })?;
            backend.setup_operation(
                "initialize",
                json_codec::json!({"username":PRINCIPAL,"password":password}),
                None,
                None,
            )?;
            Ok(())
        },
        || Ok(backend.drain_management()?),
    )?;
    let snapshot = auth.lock().unwrap().snapshot()?;
    drop(backend);
    drop(auth);

    let pool = root.join("pool");
    let discovery = root.join("disks");
    fs::DirBuilder::new().mode(0o700).create(&discovery)?;
    let paths = ["P01", "B01", "C01"].map(|slot| discovery.join(slot));
    let mut engine = Engine::initialize(&pool, storage_engine::Config::development(paths)?)?;
    let management = pool.join("management");
    fs::DirBuilder::new().mode(0o700).create(&management)?;
    let mut credentials = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(management.join("management-auth.json"))?;
    credentials.write_all(&fs::read(
        authority_management.join("management-auth.json"),
    )?)?;
    credentials.sync_all()?;
    fs::File::open(&management)?.sync_all()?;
    storage_server::management::claim_authority(&pool)?;
    engine.persist_auth_state(
        snapshot,
        PRINCIPAL,
        storage_engine::OperationId::generate()?,
    )?;
    drop(engine);
    // All topology and credential replica evidence must survive a real reopen.
    drop(Engine::open(&pool)?);
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
    // Explicit directory commits must exist before and after the content writes,
    // so the browser verifies user-history edges, never internal save checkpoints.
    // This is an explicit mutation of this private, test-owned pool only.
    initialize_directory_fixture(&mut engine, &repository, "")?;
    commit_directory_fixture(&mut engine, &repository.repository_id, "")?;
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
        commit_directory_fixture(&mut engine, &repository.repository_id, "")?;
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
    // Reopen from disk before serving: an in-memory-only fixture is insufficient.
    drop(engine);
    let engine = Engine::open(&pool)?;
    let entries = engine.list_tree(&repository.main_branch_id)?;
    let [entry] = entries.as_slice() else {
        return Err("fixture must persist exactly one file".into());
    };
    if entry.path.as_str() != FILE || engine.file_history(&entry.file_id)?.len() != 2 {
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

// Batch naming still consumes an established directory source. Seed it in this
// private pool; the Web no longer initializes or commits directory repositories.
// Keep this identity separate from the file-history fixture.
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
    initialize_directory_fixture(engine, &repository, "")?;
    commit_directory_fixture(engine, &repository.repository_id, "")?;
    Ok(())
}

pub(super) fn initialize_directory_fixture(
    engine: &mut Engine,
    repository: &storage_engine::Repository,
    path: &str,
) -> Result<()> {
    let head = engine.state().branches[&repository.main_branch_id]
        .head_commit_id
        .clone();
    engine.initialize_directory_git(
        &repository.repository_id,
        &storage_engine::directory_git::DirectoryGitInitializeBody {
            path: path.into(),
            expected_namespace_head: head,
            operation_id: storage_engine::OperationId::generate()?,
        },
        PRINCIPAL,
    )?;
    Ok(())
}

pub(super) fn commit_directory_fixture(
    engine: &mut Engine,
    repository: &str,
    path: &str,
) -> Result<()> {
    let status = engine.directory_git_status(repository, path)?;
    if !status.initialized || status.needs_restage {
        return Err(
            "directory fixture must explicitly initialize and stage its own repository".into(),
        );
    }
    engine.stage_directory_git_owned(
        repository,
        &storage_engine::directory_git::DirectoryGitStageOwnedBody {
            directory_id: status.directory_id,
            directory_branch_id: status.directory_branch_id.unwrap(),
            expected_tree_id: status.working_tree_id.unwrap(),
            expected_index_generation: status.index_generation,
            operation_id: storage_engine::OperationId::generate()?,
        },
        PRINCIPAL,
    )?;
    let status = engine.directory_git_status(repository, path)?;
    engine.commit_directory_changes(storage_engine::DirectoryCommitRequest {
        repository_id: repository,
        directory_branch_id: status
            .directory_branch_id
            .as_deref()
            .ok_or("initialized fixture lacks branch")?,
        expected_tree_id: status
            .working_tree_id
            .as_deref()
            .ok_or("initialized fixture lacks working tree")?,
        expected_index_generation: status.index_generation,
        message: "Reviewed browser directory fixture",
        principal: PRINCIPAL,
        operation_id: storage_engine::OperationId::generate()?,
    })?;
    Ok(())
}

#[cfg(test)]
#[path = "../unit/fixture_tests.rs"]
mod tests;
