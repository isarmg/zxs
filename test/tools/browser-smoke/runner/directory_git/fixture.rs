//! Persistent parent/child states in the browser runner's private disposable pool.
use crate::{
    Result,
    fixture::{commit_directory_fixture, initialize_directory_fixture},
};
use storage_engine::{CommitRequest, Engine, OperationId, RepositoryName};

pub(super) const REPOSITORY: &str = "browser-directory-git";
const PRINCIPAL: &str = "smoke-admin";
const CHILD_FILE: &str = "child/note.txt";
const COMMITTED: &[u8] = b"child committed content\n";
const WORKING: &[u8] = b"child durable but uncommitted content\n";

pub(crate) fn seed(engine: &mut Engine) -> Result<()> {
    let repository = engine.create_repository(
        &RepositoryName::parse(REPOSITORY)?,
        "P01",
        3,
        PRINCIPAL,
        OperationId::generate()?,
    )?;
    let workspace = engine.create_workspace(&repository.main_branch_id, PRINCIPAL)?;
    for (path, bytes) in [
        ("parent.txt", b"parent content\n".as_slice()),
        ("ordinary/note.txt", b"ordinary descendant\n".as_slice()),
        (CHILD_FILE, COMMITTED),
        ("unborn/note.txt", b"unborn child work\n".as_slice()),
    ] {
        engine.stage_bytes(&workspace.workspace_id, path, bytes)?;
    }
    engine.commit_workspace(CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: OperationId::generate()?,
        principal: PRINCIPAL.into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "Directory Git browser fixture working files".into(),
        request_digest: None,
    })?;
    // The parent's first commit owns both ordinary descendants. Initializing
    // children later invalidates its reviewed boundary without committing them.
    initialize_directory_fixture(engine, &repository, "")?;
    commit_directory_fixture(engine, &repository.repository_id, "")?;
    initialize_directory_fixture(engine, &repository, "child")?;
    commit_directory_fixture(engine, &repository.repository_id, "child")?;
    initialize_directory_fixture(engine, &repository, "unborn")?;
    let workspace = engine.create_workspace(&repository.main_branch_id, PRINCIPAL)?;
    engine.stage_bytes(&workspace.workspace_id, CHILD_FILE, WORKING)?;
    engine.commit_workspace(CommitRequest {
        workspace_id: workspace.workspace_id,
        operation_id: OperationId::generate()?,
        principal: PRINCIPAL.into(),
        expected_head: workspace.base_head,
        fencing_token: workspace.fencing_token,
        message: "Keep independent child working content uncommitted".into(),
        request_digest: None,
    })?;
    Ok(())
}

#[cfg(test)]
#[path = "../../unit/directory_git_fixture_tests.rs"]
mod tests;
