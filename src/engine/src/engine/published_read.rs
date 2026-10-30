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
use std::{
    borrow::Cow,
    sync::{Arc, OnceLock},
};

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

/// One backup configuration observation with its exact admitted topology.
/// This owner grants no authority; callers bound its lifetime and revalidate
/// current authorization before exposing the completed result.
pub struct BackupConfigurationReadView {
    view: PublishedReadView,
    config: crate::ValidatedConfig,
}

impl BackupConfigurationReadView {
    /// Read scope, policy and all relevant historical/future routes.
    ///
    /// # Errors
    /// Preserves scope, namespace and route errors, including their sources;
    /// also propagates cancellation from `observe` without a partial snapshot.
    pub fn read(
        &self,
        repository: &str,
        path: &str,
        observe: &dyn Fn() -> Result<()>,
    ) -> Result<storage_contract::BackupConfigurationSnapshot> {
        observe()?;
        let reader = self.view.reader(observe);
        let source =
            crate::namespace::LoaderSource(|reference: &storage_contract::NamespaceReference| {
                reader.namespace_node(reference)
            });
        crate::protection::backup_configuration(
            &self.view.state,
            &self.config,
            repository,
            path,
            &source,
            Some(observe),
        )
    }
}

/// A file-history read owns the same captured resources and the existing
/// generation-bound cold index cell. It grants no authority.
pub struct FileHistoryReadView {
    view: PublishedReadView,
    owners: Arc<OnceLock<DerivedOwners>>,
}

impl FileHistoryReadView {
    pub fn state(&self) -> &State {
        self.view.state()
    }

    /// Construct the existing projection outside the writer lock. A failed or
    /// cancelled rebuild installs nothing; concurrent successful builders may
    /// duplicate work, but never hold the cell initializer across a scan.
    ///
    /// # Errors
    /// Propagates namespace I/O/integrity failures and the observer's cancellation.
    pub fn file_history_scope<'a>(
        &'a self,
        file_id: &'a str,
        history_id: &'a str,
        observe: &'a dyn Fn() -> Result<()>,
    ) -> Result<crate::business::file_history_pages::FileHistoryScope<'a>> {
        observe()?;
        let owners = if let Some(owners) = &self.view.owners {
            owners
        } else if let Some(owners) = self.owners.get() {
            owners
        } else {
            let reader = self.view.reader(observe);
            let source = crate::namespace::LoaderSource(
                |reference: &storage_contract::NamespaceReference| reader.namespace_node(reference),
            );
            let completed = DerivedOwners::from_state_observed(&self.view.state, &source, observe)?;
            observe()?;
            self.owners.get_or_init(|| completed)
        };
        Ok(crate::business::file_history_pages::FileHistoryScope {
            resources: &self.view.resources,
            caches: &self.view.caches,
            members: Cow::Borrowed(&self.view.members),
            observe: Some(observe),
            state: &self.view.state,
            owners,
            file_id,
            history_id,
        })
    }
}

impl Engine {
    /// Capture the same publication and admitted topology without namespace I/O.
    /// Configuration cloning shares its immutable admission; member descriptors
    /// still follow the existing published-view capture budget.
    pub fn backup_configuration_read_view(&self) -> BackupConfigurationReadView {
        BackupConfigurationReadView {
            view: self.published_read_view(),
            config: self.config.clone(),
        }
    }

    /// Capture one exact admitted catalog's existing index owner without I/O.
    ///
    /// # Errors
    /// Returns `RecoveryRequired` if no catalog binds the current State.
    pub fn file_history_read_view(&self) -> Result<FileHistoryReadView> {
        let owners = self.history_owner_cell()?;
        Ok(FileHistoryReadView {
            view: self.published_read_view(),
            owners,
        })
    }

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
    /// Compare two complete namespaces using only this publication's resources.
    /// Both identities resolve before I/O; left namespace errors precede right.
    /// The response budget does not bound these O(left + right) entry owners.
    pub fn compare_commits(
        &self,
        left: &str,
        right: &str,
        observe: Option<&dyn Fn() -> storage_contract::Result<()>>,
    ) -> Result<storage_contract::CommitComparison> {
        let left = self.state.commits.get_shared(left).ok_or_else(|| {
            crate::Error::new(
                storage_contract::ErrorCode::CommitNotFound,
                "left commit missing",
            )
        })?;
        let right = self.state.commits.get_shared(right).ok_or_else(|| {
            crate::Error::new(
                storage_contract::ErrorCode::CommitNotFound,
                "right commit missing",
            )
        })?;
        let checkpoint = || -> Result<()> {
            if let Some(observe) = observe {
                observe()?;
            }
            Ok(())
        };
        let reader = SegmentReader {
            resources: &self.resources,
            members: &self.members,
            caches: &self.caches,
            observe: observe.map(|_| &checkpoint as &dyn Fn() -> Result<()>),
        };
        let source =
            crate::namespace::LoaderSource(|reference: &storage_contract::NamespaceReference| {
                reader.namespace_node(reference)
            });
        let entries = |namespace: &storage_contract::CommitNamespace| {
            let mut entries = Vec::new();
            checkpoint()?;
            crate::namespace::NamespaceView::new(namespace, &source).walk(|location, entry| {
                checkpoint()?;
                entries.push(crate::namespace::materialized_entry(
                    location.clone(),
                    entry,
                ));
                Ok(true)
            })?;
            Ok::<_, crate::Error>(entries)
        };
        let left_entries = entries(&left.namespace)?;
        let right_entries = entries(&right.namespace)?;
        storage_contract::CommitComparison::between_observed(
            left,
            left_entries,
            right,
            right_entries,
            observe,
        )
        .map_err(Into::into)
    }

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

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/backup_configuration_read_tests.rs"]
mod backup_configuration_tests;
