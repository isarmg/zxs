//! Selection-of-existing-body merges must not change content or fake metadata.
use crate::business::file_tags::invalid;
use crate::engine::CurrentNamespaces;
use crate::{Result, State};

impl CurrentNamespaces {
    pub(super) fn verify_merge_metadata(&self, previous: &State, next: &State) -> Result<()> {
        if !previous.file_metadata_nodes.is_empty() || !next.file_metadata_nodes.is_empty() {
            return Err(invalid());
        }
        let changes = next.file_branch_merges.changes_since(&previous.file_branch_merges);
        let check = |id:&str| -> Result<()> {
            let candidate=next.file_branch_merges.get(id).ok_or_else(invalid)?;
            if let Some(original)=previous.file_branch_merges.get(id) {
                return if original==candidate {Ok(())} else {Err(invalid())};
            }
            if !candidate.is_existing_adoption(){return Err(invalid())}
            // Copied/restore-scoped identities retain an upstream operation, not
            // a second native decision. The applicable copy/restore verifier owns those.
            if let Some(receipt)=next.operations.get(&candidate.operation_id)
                && crate::persist::file_adoption::is_kind(&receipt.kind)
                && receipt.result["merge_id"].as_str()==Some(id) {
                crate::persist::file_adoption::transition(previous,next,receipt,false)?;
            }
            Ok(())
        };
        match changes {
            storage_contract::MapChanges::Keys(keys) => {for id in keys {check(&id)?}}
            storage_contract::MapChanges::All => {
                for id in previous.file_branch_merges.keys() {check(id)?}
                for id in next.file_branch_merges.keys().filter(|id|!previous.file_branch_merges.contains_key(*id)){check(id)?}
            }
        }
        Ok(())
    }
}
