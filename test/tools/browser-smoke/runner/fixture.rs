//! Test-only persisted file fixture. No product upload/download CLI is required.
//! The caller owns a fresh private TempDir; never accept a user pool path.
use super::{Read, Result};
use storage_engine::{CommitRequest, Engine};

const PRINCIPAL: &str = "smoke-admin";
const FILE: &str = "browser-smoke.txt";
const PAYLOADS: [&str; 2] = ["first real version\n", "second real version: 测试\n"];

/// Use the same explicit authority publication as production setup. A pool
/// credential snapshot alone is deliberately insufficient for daemon startup.
pub(super) fn initialize_management(temporary: &tempfile::TempDir, password: &str) -> Result<()> {
    use super::{File, OpenOptions, OpenOptionsExt, Write, fs, wait_for};
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
    let mut entropy = [0u8; 32];
    File::open("/dev/urandom")?.read_exact(&mut entropy)?;
    let key: String = entropy.iter().map(|b| format!("{b:02x}")).collect();
    OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(management.join("bootstrap.key"))?
        .write_all(key.as_bytes())?;
    let mut disks = BTreeMap::new();
    for slot in ["P01", "B01", "C01"] {
        let path = root.join(slot);
        fs::DirBuilder::new().mode(0o700).create(&path)?;
        disks.insert(slot.to_owned(), path);
    }
    let (backend, auth) =
        storage_server::management::open_service_with_disks(&pool, &management, disks, true)?;
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
        json_codec::json!({"disk_ids":["P01","B01","C01"],"mode":"development",
            "username":PRINCIPAL,"password":password}),
        Some(&key),
        None,
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
        storage_engine::OperationId::generate(),
    )?;
    for payload in PAYLOADS {
        let workspace = engine.create_workspace(&repository.main_branch_id, PRINCIPAL)?;
        engine.stage_bytes(&workspace.workspace_id, FILE, payload.as_bytes())?;
        engine.commit_workspace(CommitRequest {
            workspace_id: workspace.workspace_id,
            operation_id: storage_engine::OperationId::generate(),
            principal: PRINCIPAL.into(),
            expected_head: workspace.base_head,
            fencing_token: workspace.fencing_token,
            message: "Browser metadata fixture".into(),
            request_digest: None,
        })?;
    }
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

#[cfg(test)]
#[path = "../unit/fixture_tests.rs"]
mod tests;
