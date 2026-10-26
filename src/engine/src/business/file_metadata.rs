//! Public file-branch labels are metadata, independent of immutable content.
use super::*;
use json_codec::json;

impl Engine {
    pub fn rename_file_branch(
        &mut self,
        file_id: &str,
        branch_id: &str,
        name: &str,
        principal: &str,
        operation: &str,
    ) -> Result<OperationResult> {
        validate_branch_name(name)?;
        self.mutate(
            "file.branch.rename",
            principal,
            operation,
            json!({"file_id":file_id,"branch_id":branch_id,"name":name}),
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
                    && name_policy::lookup_key(&branch.name)==name_policy::lookup_key(name)) {
                    return Err(Error::new(storage_contract::ErrorCode::NameCollision,"a public branch of this file already uses that name"));
                }
                let previous=current.name.clone();
                if previous!=name {
                    state.branches.get_mut(branch_id).expect("checked branch").name=name.into();
                }
                Ok(json!({"file_id":file_id,"branch_id":branch_id,"history_tree_id":repository.history_tree_id,
                    "previous_name":previous,"name":name,"no_change":previous==name}))
            },
        )
    }
}
