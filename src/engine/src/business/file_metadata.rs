//! Public file-branch labels are metadata, independent of immutable content.
use super::*;
use json_codec::json;

impl Engine {
    pub fn rename_file_branch(
        &mut self,
        file_id: &str,
        branch_id: &str,
        name: &storage_contract::BranchName,
        principal: &str,
        operation: OperationId,
    ) -> Result<OperationResult> {
        self.mutate(
            "file.branch.rename",
            principal,
            operation,
            json!({"file_id":file_id,"branch_id":branch_id,"name":name.as_str()}),
            |engine, state, _| {
                let current = state.branches.get(branch_id)
                    .filter(|branch| branch.file_id.as_deref()==Some(file_id)&&branch.state!=BranchState::Migrated)
                    .ok_or_else(||Error::new(storage_contract::ErrorCode::FileBranchNotFound,"file branch missing"))?;
                let repository=engine.repository_for_history(&current.history_tree_id)?;

                if current.state!=BranchState::Active || state.file_branch_aliases.contains_key(branch_id) {
                    return Err(Error::new(storage_contract::ErrorCode::BranchNotActive,"merged file aliases are read-only"));
                }
                if state.branches.values().any(|branch|branch.file_id.as_deref()==Some(file_id)
                    && branch.branch_id!=branch_id
                    && name_policy::lookup_key(&branch.name)==name.key()) {
                    return Err(Error::new(storage_contract::ErrorCode::NameCollision,"a public branch of this file already uses that name"));
                }
                let previous=current.name.clone();
                if previous!=name.as_str() {
                    state.branches.get_mut(branch_id).expect("invariant: the selected branch remains in this mutation candidate").name=name.as_str().into();
                }
                Ok(json!({"file_id":file_id,"branch_id":branch_id,"history_tree_id":repository.history_tree_id,
                    "previous_name":previous,"name":name.as_str(),"no_change":previous==name.as_str()}))
            },
        )
    }
}
