//! Explicit user steps for fixtures that need a clean directory repository.
//! File saves remain separate and never initialize or commit a directory.
use super::{DirectoryCommitRequest, Engine, Repository};
use storage_contract::{
    OperationId,
    directory_git::{
        DirectoryGitInitializeBody, DirectoryGitStageChildBody, DirectoryGitStageOwnedBody,
        DirectoryGitStatus,
    },
};

pub(super) fn checkpoint(
    engine: &mut Engine,
    repository: &Repository,
    path: &str,
) -> DirectoryGitStatus {
    checkpoint_as(engine, repository, path, "admin")
}

pub(super) fn checkpoint_as(
    engine: &mut Engine,
    repository: &Repository,
    path: &str,
    principal: &str,
) -> DirectoryGitStatus {
    let mut status = engine
        .directory_git_status(&repository.repository_id, path)
        .unwrap();
    if !status.initialized {
        let head = engine.state().branches[&repository.main_branch_id]
            .head_commit_id
            .clone();
        engine
            .initialize_directory_git(
                &repository.repository_id,
                &DirectoryGitInitializeBody {
                    path: path.into(),
                    expected_namespace_head: head,
                    operation_id: OperationId::generate().unwrap(),
                },
                principal,
            )
            .unwrap();
        status = engine
            .directory_git_status(&repository.repository_id, path)
            .unwrap();
    }
    // Stage already committed children explicitly. Never recursively commit a
    // child's dirty working tree on behalf of the parent fixture.
    for child in status.children.clone() {
        let Some(head) = child.head_tree_id else {
            continue;
        };
        if child.staged_tree_id.as_deref() == Some(head.as_str()) {
            continue;
        }
        engine
            .stage_directory_git_child(
                &repository.repository_id,
                &DirectoryGitStageChildBody {
                    directory_id: status.directory_id.clone(),
                    directory_branch_id: status.directory_branch_id.clone().unwrap(),
                    expected_index_generation: status.index_generation,
                    child_directory_id: child.directory_id,
                    expected_child_head_tree_id: head,
                    operation_id: OperationId::generate().unwrap(),
                },
                principal,
            )
            .unwrap();
        status = engine
            .directory_git_status(&repository.repository_id, path)
            .unwrap();
    }
    // Review after pinning children, including a projection-only refresh.
    engine
        .stage_directory_git_owned(
            &repository.repository_id,
            &DirectoryGitStageOwnedBody {
                directory_id: status.directory_id.clone(),
                directory_branch_id: status.directory_branch_id.clone().unwrap(),
                expected_tree_id: status.working_tree_id.clone().unwrap(),
                expected_index_generation: status.index_generation,
                operation_id: OperationId::generate().unwrap(),
            },
            principal,
        )
        .unwrap();
    status = engine
        .directory_git_status(&repository.repository_id, path)
        .unwrap();
    if status.staged {
        engine
            .commit_directory_changes(DirectoryCommitRequest {
                repository_id: &repository.repository_id,
                directory_branch_id: status.directory_branch_id.as_deref().unwrap(),
                expected_tree_id: status.working_tree_id.as_deref().unwrap(),
                expected_index_generation: status.index_generation,
                message: "Explicit fixture directory checkpoint",
                principal,
                operation_id: OperationId::generate().unwrap(),
            })
            .unwrap();
        status = engine
            .directory_git_status(&repository.repository_id, path)
            .unwrap();
    }
    assert!(!status.staged && !status.needs_restage);
    status
}
