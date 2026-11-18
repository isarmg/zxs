//! Primary-disk selection over one retained namespace, followed by bounded
//! current projection after admission and namespace identity are rechecked.
use super::pagination::{Page, PageRequest, PageSelector};
use super::*;
use crate::backend_listing::SortKey;
// Selection holds only this registered namespace and its current Unborn rows.
#[derive(Clone)]
enum Candidate {
    Entry(usize),
    Unborn(usize),
}

pub(in crate::engine_backend) enum ListingRead {
    Backup(Box<backup_namespace::BackupListingRead>),
    Primary(Box<PrimaryListing>),
}

struct DirectoryCapture {
    repository: Repository,
    within: String,
    within_key: String,
    namespace: storage_engine::NamespaceSnapshot,
    unborn: Vec<(storage_contract::file_draft::DraftLocation,
        storage_contract::file_draft::UnbornWorking)>,
}

struct ListingScope {
    #[cfg(test)]
    sequence: u64,
    slot_id: String,
    path: String,
    directory: Option<DirectoryCapture>,
    writable: bool,
}

pub(in crate::engine_backend) struct PrimaryListing {
    scope: ListingScope,
    request: PageRequest,
}

pub(in crate::engine_backend) struct SelectedListing {
    scope: ListingScope,
    page: Page<Candidate>,
    directory_file_count: usize,
}

pub(in crate::engine_backend) struct ProjectedEntries {
    entries: Vec<crate::ListingEntry>,
    selected_entry: Option<crate::ListingEntry>,
}

pub(in crate::engine_backend) fn capture(
    engine: &AdmissionEngine,
    input: crate::ReadListing,
) -> Result<ListingRead, ApiError> {
    let slot_id = input.slot_id().to_owned();
    let path = input.path().to_owned();
    // A cursor is checked before disk and path, but can never grant admission.
    let request = PageRequest::parse(engine, input)?;
    let selected_disk = disk(engine, &slot_id)?;
    if selected_disk.level != 1 {
        return backup_namespace::BackupListingRead::capture(engine, &slot_id, &path, request)
            .map(Box::new)
            .map(ListingRead::Backup);
    }
    let writable = selected_disk.registration_mode == storage_engine::RegistrationMode::Enabled;
    let directory = match engine.ordinary_metadata_root_for_slot(&slot_id).map_err(error)? {
        Some(repository) => Some(DirectoryCapture::capture(engine, repository.clone(), &path)?),
        None if path.is_empty() => None,
        None => return Err(empty_directory()),
    };
    Ok(ListingRead::Primary(Box::new(PrimaryListing {
        scope: ListingScope {
            #[cfg(test)]
            sequence: engine.state().sequence,
            slot_id,
            path,
            directory,
            writable,
        },
        request,
    })))
}

impl DirectoryCapture {
    fn capture(
        engine: &Engine,
        repository: Repository,
        within: &str,
    ) -> Result<Self, ApiError> {
        let namespace = engine.namespace_snapshot(&repository.main_branch_id).map_err(error)?;
        let within_key = name_policy::path_key(within);
        if !within.is_empty() && !namespace.tree().get_key(&within_key).is_some_and(|entry|
            entry.kind == storage_contract::NamespaceEntryKind::Directory)
        {
            return Err(ApiError::new(
                ::http::StatusCode::NOT_FOUND,
                storage_contract::ErrorCode::DirectoryNotFound,
                "directory not found",
            ));
        }
        let id = repository.repository_id.parse().map_err(ApiError::contract_input)?;
        // The owner supplies only currently admitted rows. It will not treat
        // an unresolved Unborn after a registry change as an empty namespace.
        let unborn = engine.native_current_entries(id).map_err(error)?.into_iter()
            .filter_map(|entry| match entry {
                storage_engine::NativeCurrentEntry::Unborn { target, working } => Some((target, working)),
                storage_engine::NativeCurrentEntry::Published(_) => None,
            }).collect();
        Ok(Self {
            repository, unborn,
            within: within.into(),
            within_key,
            namespace,
        })
    }
}

impl PrimaryListing {
    pub(in crate::engine_backend) fn select(self) -> Result<SelectedListing, ApiError> {
        #[cfg(test)]
        detached_tests::before_scan(self.scope.sequence);
        let Self { scope, request } = self;
        let mut page = PageSelector::new(request);
        let mut directory_file_count = 0;
        if let Some(directory) = &scope.directory {
            let prefix = if directory.within.is_empty() {
                String::new()
            } else {
                format!("{}/", directory.within_key)
            };
            for (index, (target, _)) in directory.unborn.iter().enumerate() {
                let Some(relative) = target.path.key().strip_prefix(&prefix) else { continue; };
                directory_file_count += 1;
                if relative.is_empty() || relative.contains('/') { continue; }
                page.consider(SortKey::new(false, target.path.file_name(),
                    target.path.as_str(), &target.entry.to_string()), Candidate::Unborn(index));
            }
            // Scan the retained flat namespace, keeping at most limit + 1
            // candidates plus the independently requested selected entry.
            for (index, _) in directory.namespace.entries().iter().enumerate() {
                let Some(entry) = directory.namespace.tree().visible_entry(index) else { continue; };
                let Some(relative) = entry.lookup_key().strip_prefix(&prefix) else {
                    continue;
                };
                if entry.kind == storage_contract::NamespaceEntryKind::File {
                    directory_file_count += 1;
                }
                if relative.contains('/') {
                    continue;
                }
                page.consider(SortKey::for_entry(entry, entry.path().to_owned()), Candidate::Entry(index));
            }
        }
        Ok(SelectedListing {
            page: page.finish()?,
            scope,
            directory_file_count,
        })
    }
}

fn changed() -> ApiError {
    ApiError::conflict(
        storage_contract::ErrorCode::ListingChanged,
        "directory listing changed; restart from the first page",
    )
}

impl SelectedListing {
    fn authorize_current(&self, engine: &AdmissionEngine) -> Result<(), ApiError> {
        if engine.listing_snapshot() != self.page.snapshot {
            return Err(changed());
        }
        // Re-acquire original registration under the same final authorization
        // lock; a physical rebind is not required to have changed the journal.
        let live = disk(engine, &self.scope.slot_id)?.registration_mode == storage_engine::RegistrationMode::Enabled;
        if live != self.scope.writable { return Err(changed()); }
        let observed = engine.ordinary_metadata_root_for_slot(&self.scope.slot_id).map_err(error)?;
        match (&self.scope.directory, observed) {
            (Some(row), Some(current)) if current == &row.repository
                && engine.namespace_snapshot_matches(&row.namespace.witness()) => (),
            (None, None) => (),
            _ => return Err(changed()),
        }
        Ok(())
    }

    /// The caller has refreshed current identity under this same engine lock.
    pub(in crate::engine_backend) fn project(
        &self,
        engine: &AdmissionEngine,
    ) -> Result<ProjectedEntries, ApiError> {
        self.authorize_current(engine)?;
        let project = |candidate: &Candidate| match candidate {
            Candidate::Unborn(index) => {
                let directory = self.scope.directory.as_ref().ok_or_else(changed)?;
                let (target, working) = &directory.unborn[*index];
                // listing_snapshot() was compared under this same Engine lock:
                // the captured current owner was not replaced between scan and projection.
                Ok(crate::ListingEntry {
                    entry_id: target.entry.to_string(), name: target.path.file_name().into(),
                    path: target.path.as_str().into(),
                    kind: storage_contract::NamespaceEntryKind::File,
                    size: working.logical_size(), file_id: target.file.to_string(),
                    version: crate::ListingVersion::Unborn { target: target.clone() },
                    location: Some(crate::ListingLocation {
                        repository_id: target.repository.to_string(),
                        repository_path: Some(target.path.as_str().into()),
                        container: true, selected_branch_id: None,
                    }),
                    note: None, node_id: None, read_only: Some(false),
                    backup: false, historical_only: None,
                })
            }
            Candidate::Entry(index) => {
                let directory = self.scope.directory.as_ref().ok_or_else(changed)?;
                listed_current_entry(engine, &directory.repository, *index, self.scope.writable)
            }
        };
        let entries = self
            .page
            .values
            .iter()
            .map(project)
            .collect::<Result<Vec<_>, ApiError>>()?;
        let selected_entry = self.page.selected.as_ref().map(project).transpose()?;
        Ok(ProjectedEntries {
            entries,
            selected_entry,
        })
    }

    /// Consume summaries and release the retained generation after unlocking.
    pub(in crate::engine_backend) fn finish(
        self,
        page: ProjectedEntries,
    ) -> crate::ListingSnapshot {
        let context = self
            .scope
            .directory
            .as_ref()
            .map(|directory| crate::ListingContext {
                repository_id: directory.repository.repository_id.clone(),
                repository_path: directory.within.clone(),
                branch_id: directory.repository.main_branch_id.clone(),
                base_head: directory.namespace.head_commit_id().into(),
            });
        crate::ListingSnapshot {
            slot_id: self.scope.slot_id,
            path: self.scope.path,
            backup: false,
            writable: self.scope.writable,
            entries: page.entries,
            selected_entry: page.selected_entry,
            context,
            directory_file_count: self.directory_file_count,
            total_entries: self.page.total,
            snapshot: self.page.snapshot,
            previous_cursor: self.page.previous,
            next_cursor: self.page.next,
        }
    }
}

#[cfg(test)]
#[path = "../../../../../test/server/unit/engine_backend/file_manager/listing_detached_tests.rs"]
mod detached_tests;

#[cfg(test)]
#[path = "../../../../../test/server/unit/engine_backend/file_manager/planned_offline_listing.rs"]
mod planned_offline_tests;
