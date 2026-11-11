use crate::{ApiError, auth::Identity, engine_backend::EngineBackend};
use std::ops::Bound;
use storage_contract::file_deletion_review::{
    BranchHead, BranchPage, PAGE_SIZE, ParentPage, Summary,
};
use storage_contract::{BranchId, FileDeletionId, FileDeletionNode, FileId};

fn exact(
    engine: &storage_engine::Engine,
    file: FileId,
    deletion: FileDeletionId,
) -> Result<&FileDeletionNode, ApiError> {
    engine
        .file_deletion(&file.to_string())
        .filter(|node| node.node_id == deletion.to_string())
        .ok_or_else(|| {
            ApiError::new(
                http::StatusCode::NOT_FOUND,
                storage_contract::ErrorCode::FileNodeNotFound,
                "exact file deletion is no longer active",
            )
        })
}

impl EngineBackend {
    pub(in crate::engine_backend) fn file_deletion_summary_admitted(
        &self,
        identity: &Identity,
        file: FileId,
        deletion: FileDeletionId,
    ) -> Result<Summary, ApiError> {
        let (engine, current) = self.lock_current(identity)?;
        if current.smb_binding.is_some() {
            return Err(ApiError::forbidden());
        }
        Ok(crate::http_file_deletion::projection::summary(exact(
            &engine, file, deletion,
        )?))
    }

    pub(in crate::engine_backend) fn file_deletion_branches_admitted(
        &self,
        identity: &Identity,
        file: FileId,
        deletion: FileDeletionId,
        after: Option<BranchId>,
    ) -> Result<BranchPage, ApiError> {
        let (engine, current) = self.lock_current(identity)?;
        if current.smb_binding.is_some() {
            return Err(ApiError::forbidden());
        }
        let node = exact(&engine, file, deletion)?;
        let after = after.map(|id| id.to_string());
        let lower = after.as_deref().map_or(Bound::Unbounded, Bound::Excluded);
        let mut range = node.branch_heads.range::<str, _>((lower, Bound::Unbounded));
        let entries = range
            .by_ref()
            .take(PAGE_SIZE)
            .map(|(branch, head)| BranchHead {
                branch_id: branch
                    .parse()
                    .expect("invariant: admitted deletion branch ID"),
                node_id: head.parse().expect("invariant: admitted deletion head ID"),
            })
            .collect::<Vec<_>>();
        let next_after = range.next().map(|_| {
            entries
                .last()
                .expect("invariant: another branch follows a full page")
                .branch_id
        });
        Ok(BranchPage {
            file_id: file,
            deletion_node_id: deletion,
            entries,
            next_after,
        })
    }

    pub(in crate::engine_backend) fn file_deletion_parents_admitted(
        &self,
        identity: &Identity,
        file: FileId,
        deletion: FileDeletionId,
        offset: u64,
    ) -> Result<ParentPage, ApiError> {
        let (engine, current) = self.lock_current(identity)?;
        if current.smb_binding.is_some() {
            return Err(ApiError::forbidden());
        }
        let node = exact(&engine, file, deletion)?;
        if offset > node.parent_node_ids.len() as u64 {
            return Err(ApiError::invalid("parent offset exceeds deletion frontier"));
        }
        let start = offset as usize;
        let count = (node.parent_node_ids.len() - start).min(PAGE_SIZE);
        let end = start + count;
        let entries = node.parent_node_ids[start..end]
            .iter()
            .map(|id| id.parse().expect("invariant: admitted deletion parent ID"))
            .collect();
        Ok(ParentPage {
            file_id: file,
            deletion_node_id: deletion,
            offset,
            entries,
            next_offset: (end < node.parent_node_ids.len()).then_some(end as u64),
        })
    }
}
