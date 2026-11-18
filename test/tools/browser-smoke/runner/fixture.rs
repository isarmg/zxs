//! Test-only persisted file fixture. No product upload/download CLI is required.
//! The caller owns a fresh private TempDir; never accept a user pool path.
use super::{Read, Result};
use name_policy::Name;
use storage_engine::{CommitRequest, Engine};

const PRINCIPAL: &str = "smoke-admin";
const FILE_PATH: &str = "browser-fixtures/browser-smoke.txt";
const NAMING_FILE_PATH: &str = "browser-naming/browser-smoke.txt";
const FILE_BRANCH: &str = "browser-smoke-alternate";
const PAYLOADS: [&str; 2] = ["first real version\n", "second real version: 测试\n"];
pub(super) const NAMING_FOLDER: &str = "browser-naming";

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
    let paths = [
        (Name::parse("Fixture primary")?, discovery.join("P01")),
        (Name::parse("Fixture backup")?, discovery.join("B01")),
        (Name::parse("Fixture archive")?, discovery.join("C01")),
    ];
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
    let repository = engine.create_disk_root("P01", PRINCIPAL, storage_engine::OperationId::generate()?)?;
    // The backup browser must really see three independently stored copies.
    // Configure that ordinary path before its first write, not after the fact.
    engine.update_protection_policy(storage_engine::ProtectionPolicyUpdate {
        policy_id: &storage_engine::new_id("policy")?, repository_id: &repository.repository_id,
        path_prefix: "browser-fixtures", target_copies: 3, expected_generation: 0,
        principal: PRINCIPAL, operation_id: storage_engine::OperationId::generate()?,
    })?;
    for payload in PAYLOADS {
        let workspace = engine.create_workspace(&repository.main_branch_id, PRINCIPAL)?;
        engine.stage_bytes(&workspace.workspace_id, FILE_PATH, payload.as_bytes())?;
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
    let entry = entries.iter().find(|entry| entry.path() == FILE_PATH)
        .ok_or("ordinary fixture file missing before branching")?;
    // An explicit FILE branch binds an actual version without copying content.
    engine.create_file_branch_from_node(
        &entry.file_id,
        &entry.version_id,
        &entry.version_id,
        &storage_engine::BranchName::parse(FILE_BRANCH)?,
        PRINCIPAL,
        storage_engine::OperationId::generate().unwrap(),
    )?;
    seed_naming_scope(&mut engine, &repository)?;
    // Reopen from disk before serving: an in-memory-only fixture is insufficient.
    drop(engine);
    let engine = Engine::open(&pool)?;
    let entries = engine.list_tree(&repository.main_branch_id)?;
    let entry = entries.iter().find(|entry| entry.path() == FILE_PATH).ok_or("ordinary file missing after reopen")?;
    if engine.file_history(&entry.file_id)?.len() != 2 {
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

// A separate ordinary folder and FILE isolate naming edits from the history case.
fn seed_naming_scope(engine: &mut Engine, repository: &storage_engine::Repository) -> Result<()> {
    let workspace = engine.create_workspace(&repository.main_branch_id, PRINCIPAL)?;
    engine.stage_bytes(&workspace.workspace_id, NAMING_FILE_PATH, PAYLOADS[0].as_bytes())?;
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

#[cfg(test)]
#[path = "../unit/fixture_tests.rs"]
mod tests;
