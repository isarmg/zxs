//! Explicit setup for server fixtures that need a committed directory repository.
//! Reads and ordinary file saves remain separate from these user steps.
use storage_contract::{
    OperationId, Repository,
    directory_git::{
        DirectoryGitInitializeBody, DirectoryGitStageChildBody, DirectoryGitStageOwnedBody,
        DirectoryGitStatus,
    },
};
use storage_engine::{DirectoryCommitRequest, Engine};

pub(crate) fn checkpoint(
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
    if status.staged || status.needs_restage {
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
    }
    // Pin only already committed children. Never recursively commit their work.
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
    if status.staged {
        engine
            .commit_directory_changes(DirectoryCommitRequest {
                repository_id: &repository.repository_id,
                directory_branch_id: status.directory_branch_id.as_deref().unwrap(),
                expected_tree_id: status.working_tree_id.as_deref().unwrap(),
                expected_index_generation: status.index_generation,
                message: "Explicit server fixture directory checkpoint",
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
