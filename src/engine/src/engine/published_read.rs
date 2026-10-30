//! One immutable publication's activity and namespace sources, detached from the writer.
use super::{
    Engine,
    operations::{ReceiptsDescending, receipt_by_sequence},
};
use crate::{
    DiskConfig, NamespaceTree, OperationId, OperationResult, Result, State,
    disk::DiskResources,
    segment_store::{ReceiptLookup, SegmentCaches, SegmentReader},
    state_projection::DerivedOwners,
};
use std::{borrow::Cow, sync::Arc};

/// Owns the admitted root and its exact read resources. It grants no authority.
/// Published immutable metadata has no runtime garbage collection; the request
/// owner bounds concurrent plans and cancels work before releasing its lease.
pub struct PublishedReadView {
    state: Arc<State>,
    resources: Arc<DiskResources>,
    caches: Arc<SegmentCaches>,
    members: Vec<DiskConfig>,
    locator_root: Option<crate::metadata_map::Reference>,
    owners: Option<DerivedOwners>,
}

impl Engine {
    /// Capture without reads or constructing a cold owner index. Existing owner
    /// membership is accepted only by `available_history_owners`' exact-root proof.
    pub fn published_read_view(&self) -> PublishedReadView {
        PublishedReadView {
            state: Arc::clone(&self.state),
            resources: Arc::clone(&self.disk_resources),
            caches: Arc::clone(&self.segment_caches),
            members: self.members(),
            locator_root: self.operation_locator_root.clone(),
            owners: self.available_history_owners().cloned(),
        }
    }
}

impl PublishedReadView {
    pub fn state(&self) -> &State {
        &self.state
    }

    /// The frozen source selected by this publication, not the writer's current head.
    pub fn pending_file_source(&self, branch_id: &str, file_id: &str) -> Option<&str> {
        crate::business::pending_file_source(&self.state, branch_id, file_id)
    }

    /// Resolve an entry from this publication's current main commit. The caller
    /// must not substitute an entry from another snapshot or repository.
    pub fn file_context_for_current_entry<'a>(
        &'a self,
        repository: &str,
        entry: &storage_contract::Entry,
    ) -> Option<Cow<'a, storage_contract::FileContext>> {
        crate::business::directory::file_context_for_current_entry(&self.state, repository, entry)
    }

    fn reader<'a>(&'a self, observe: &'a dyn Fn() -> Result<()>) -> SegmentReader<'a> {
        SegmentReader {
            resources: &self.resources,
            members: &self.members,
            caches: &self.caches,
            observe: Some(observe),
        }
    }

    /// Resolve the same smallest-version owner as the live Engine. Warm reads
    /// use the captured shared index; cold reads remain cancellable State scans.
    pub fn file_history_owner(
        &self,
        file: &str,
        observe: &dyn Fn() -> Result<()>,
    ) -> Result<Option<&str>> {
        observe()?;
        if let Some(owners) = &self.owners {
            return Ok(owners.first_version(file).map(|id| {
                self.state
                    .versions
                    .get(id)
                    .expect("invariant: captured owner membership references an existing version")
                    .history_tree_id
                    .as_str()
            }));
        }
        for version in self.state.versions.values() {
            observe()?;
            if version.file_id == file {
                return Ok(Some(&version.history_tree_id));
            }
        }
        Ok(None)
    }

    pub fn commit_namespace<'a>(
        &'a self,
        commit_id: &str,
        observe: &'a dyn Fn() -> Result<()>,
    ) -> Option<NamespaceTree<'a>> {
        self.state
            .commits
            .get(commit_id)
            .map(|commit| NamespaceTree::captured(commit.namespace.clone(), self.reader(observe)))
    }

    /// Preserves live branch admission, including migrated/foreign-root errors.
    pub fn namespace_tree<'a>(
        &'a self,
        branch_id: &str,
        observe: &'a dyn Fn() -> Result<()>,
    ) -> Result<NamespaceTree<'a>> {
        observe()?;
        let commit = crate::business::tree_commit(&self.state, branch_id)?;
        Ok(NamespaceTree::captured(
            commit.namespace.clone(),
            self.reader(observe),
        ))
    }

    /// Read immutable publication evidence, never current replica health.
    pub fn published_receipt(
        &self,
        id: OperationId,
        observe: &dyn Fn() -> Result<()>,
    ) -> Result<Option<OperationResult>> {
        observe()?;
        ReceiptLookup {
            state: &self.state,
            reader: self.reader(observe),
            locator_root: self.locator_root.as_ref(),
        }
        .find_view(id)
        .map(|entry| {
            entry.map(|entry| match entry {
                Cow::Borrowed(entry) => entry.result.clone(),
                Cow::Owned(entry) => entry.result,
            })
        })
    }

    pub fn receipt_by_sequence(
        &self,
        sequence: u64,
        observe: &dyn Fn() -> Result<()>,
    ) -> Result<Option<storage_contract::OperationRecordEntry>> {
        receipt_by_sequence(&self.state, &self.reader(observe), sequence)
    }

    pub fn receipts_descending<'a>(
        &'a self,
        observe: &'a dyn Fn() -> Result<()>,
    ) -> ReceiptsDescending<'a> {
        ReceiptsDescending::captured(&self.state, self.reader(observe))
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/published_read_tests.rs"]
mod tests;
