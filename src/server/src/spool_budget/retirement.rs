//! Retirement of redundant API copies, never of core publication evidence.
use super::*;
use crate::{Backend, HttpSpoolCandidate, InternalHttpSpoolRetirement, uploads};
use storage_contract::{Workspace, WorkspaceState};

struct NamedEntry {
    key: String,
    entry: EntryName,
}
impl NamedEntry {
    fn new(key: String) -> Result<Self, ApiError> {
        let entry = EntryName::new(&key).map_err(crate::upload_fs_error)?;
        Ok(Self { key, entry })
    }
}
struct RetirementNames {
    body: EntryName,
    current: NamedEntry,
    owner: NamedEntry,
    retained: NamedEntry,
}
enum MetadataMatch {
    Matching,
    TargetMismatch,
    AttributionChanged,
}
impl RetirementNames {
    fn new(record: &UploadRecord, current: &str) -> Result<Self, ApiError> {
        let key = current
            .strip_suffix(".json")
            .expect("invariant: current metadata has the canonical JSON suffix");
        Ok(Self {
            body: EntryName::new(&record.data_file).map_err(crate::upload_fs_error)?,
            current: NamedEntry::new(current.to_owned())?,
            owner: NamedEntry::new(format!("{}.owner.json", record.data_file))?,
            retained: NamedEntry::new(uploads::retained_name_for_key(key, record))?,
        })
    }
}

impl HttpSpoolCandidate {
    /// Match the exact completed API copy against durable committed authority.
    /// This does not check whether the provider's engine is currently blocked.
    pub fn matches(&self, workspace: &Workspace) -> bool {
        let record = &self.record;
        workspace.state == WorkspaceState::Committed
            && workspace.workspace_id == record.workspace_id
            && workspace.principal == record.principal
            && record.state == uploads::UploadState::Complete
            && record.generation != 0
            && record.persisted_bytes == record.logical_size
            && record
                .content_hash
                .is_some_and(|hash| hash.to_string() == record.persisted_hash)
            && workspace.files.get(&record.item_id).is_some_and(|file| {
                file.item_id == record.item_id
                    && file.path.as_str() == record.path
                    && file.dirty_generation == record.generation
                    && file.size == record.logical_size
                    && Some(file.content_hash) == record.content_hash
            })
    }
}

impl SpoolBudget {
    pub(crate) fn is_uncertain(&self) -> bool {
        lock(&self.ledger).uncertain
    }

    pub(crate) fn maintenance_issue_count(&self) -> usize {
        lock(&self.ledger).maintenance_issues.len()
    }

    pub(crate) fn has_candidates(
        &self,
        workspace: &str,
        item: Option<&uploads::CurrentName>,
    ) -> bool {
        let ledger = lock(&self.ledger);
        ledger
            .current_by_workspace
            .get(workspace)
            .is_some_and(|names| item.is_none_or(|item| names.contains(item.name())))
    }

    pub(crate) fn resolve_workspace_failure(
        &self,
        workspace: &str,
        item: Option<&uploads::CurrentName>,
    ) {
        let mut ledger = lock(&self.ledger);
        for name in ledger.current_names(workspace, item) {
            ledger.maintenance_issues.remove(&name);
        }
    }

    pub(crate) fn note_workspace_failure(
        &self,
        workspace: &str,
        item: Option<&uploads::CurrentName>,
        error: &ApiError,
    ) {
        let mut ledger = lock(&self.ledger);
        let names = ledger.current_names(workspace, item);
        let mut newly_observed = false;
        for name in names {
            newly_observed |= ledger.maintenance_issues.insert(name);
        }
        if newly_observed {
            tracing::warn!(error = ?error, "HTTP copy maintenance failed; workspace copies remain charged");
        }
    }

    /// The startup owner has not yet admitted uploads. Walk the derived index
    /// one record at a time; never hold the ledger while asking core authority.
    pub(crate) fn retire_startup(&self, backend: &dyn Backend) -> Result<bool, ApiError> {
        let mut after = None;
        loop {
            let next = {
                let ledger = lock(&self.ledger);
                if ledger.uncertain {
                    return Ok(true);
                }
                use std::ops::Bound::{Excluded, Unbounded};
                let lower = after.as_ref().map_or(Unbounded, Excluded);
                ledger
                    .metadata
                    .range::<String, _>((lower, Unbounded))
                    .find_map(|(name, reference)| match reference {
                        MetadataReference::Upload {
                            record,
                            role: UploadRole::Current,
                        } if record.state == uploads::UploadState::Complete => Some((
                            name.clone(),
                            HttpSpoolCandidate {
                                record: record.clone(),
                            },
                        )),
                        _ => None,
                    })
            };
            let Some((name, candidate)) = next else {
                return Ok(true);
            };
            after = Some(name);
            let current = after
                .as_deref()
                .expect("invariant: selected metadata has a name");
            match backend.qualify_http_spool_copy(&InternalHttpSpoolRetirement(()), &candidate) {
                Ok(crate::HttpSpoolQualification::Eligible) => {
                    self.retire_copy(&candidate.record, current)?
                }
                Ok(crate::HttpSpoolQualification::Retain) => {
                    lock(&self.ledger).maintenance_issues.remove(current);
                }
                Ok(crate::HttpSpoolQualification::Unavailable) => return Ok(false),
                Err(error) => {
                    lock(&self.ledger).maintenance_failure(current, &error);
                }
            }
        }
    }

    /// The caller has just obtained this workspace through current user auth.
    /// A busy upload keeps its files; a later authenticated replay can retry.
    pub(crate) fn retire_workspace(
        &self,
        workspace: &Workspace,
        claims: &Arc<Mutex<BTreeSet<String>>>,
    ) -> Result<(), ApiError> {
        if workspace.state != WorkspaceState::Committed {
            return Ok(());
        }
        for item in workspace.files.keys() {
            let current = uploads::CurrentName::new(&workspace.workspace_id, item);
            self.retire_item(workspace, &current, claims)?;
        }
        Ok(())
    }

    pub(crate) fn retire_item(
        &self,
        workspace: &Workspace,
        current: &uploads::CurrentName,
        claims: &Arc<Mutex<BTreeSet<String>>>,
    ) -> Result<(), ApiError> {
        let key = current.key().to_owned();
        if !lock(claims).insert(key.clone()) {
            return Ok(());
        }
        let _claim = uploads::Claim {
            claims: claims.clone(),
            key,
        };
        let candidate = {
            let ledger = lock(&self.ledger);
            match ledger.metadata.get(current.name()) {
                Some(MetadataReference::Upload {
                    record,
                    role: UploadRole::Current,
                }) => Some(HttpSpoolCandidate {
                    record: record.clone(),
                }),
                _ => None,
            }
        };
        if let Some(candidate) = candidate {
            if candidate.matches(workspace) {
                self.retire_copy(&candidate.record, current.name())?;
            } else {
                lock(&self.ledger).maintenance_issues.remove(current.name());
            }
        }
        Ok(())
    }

    fn retire_copy(&self, record: &UploadRecord, current: &str) -> Result<(), ApiError> {
        let mut ledger = lock(&self.ledger);
        if ledger.uncertain {
            return Ok(());
        }
        let plan = match RetirementNames::new(record, current) {
            Ok(plan) => plan,
            Err(error) => {
                ledger.maintenance_failure(current, &error);
                return Ok(());
            }
        };
        let current = &plan.current.key;
        let owner = &plan.owner.key;
        let retained = &plan.retained.key;
        let Some(charge) = ledger.files.get(&record.data_file) else {
            return Ok(());
        };
        if charge.reserved != 0 {
            return Ok(());
        }
        if charge.owner.as_deref() != Some(record.principal.as_str())
            || ledger.metadata.contains_key(&record.data_file)
        {
            ledger.maintenance_issues.remove(current);
            return Ok(());
        }
        let Some(names) = ledger.references.get(&record.data_file) else {
            return Ok(());
        };
        if [&plan.owner.key, &plan.retained.key].iter().any(|name| {
            ledger.metadata.contains_key(*name) && !names.contains(*name)
        }) || !names.contains(current)
            || names.iter().any(|name| {
                if ledger.references.contains_key(name) {
                    return true;
                }
                if name == owner {
                    !matches!(ledger.metadata.get(name), Some(MetadataReference::Owner(actual))
                        if actual.data_file == record.data_file && actual.principal == record.principal)
                } else {
                    (name != current && name != retained)
                        || !matches!(ledger.metadata.get(name), Some(MetadataReference::Upload { record: actual, .. }) if actual == record)
                }
            })
        {
            ledger.maintenance_issues.remove(current);
            return Ok(());
        }
        if let Err(error) = self
            .directory
            .verify_identity()
            .map_err(crate::upload_fs_error)
        {
            ledger.uncertain = true;
            return Err(error);
        }
        match self.copy_metadata_matches(record, &plan, names) {
            Ok(MetadataMatch::Matching) => {}
            Ok(MetadataMatch::AttributionChanged) => {
                ledger.uncertain = true;
                return Err(invalid_state(
                    "HTTP spool metadata attribution changed; restart and inventory required",
                ));
            }
            Ok(MetadataMatch::TargetMismatch) => {
                ledger.maintenance_failure(
                    current,
                    &invalid_state("target retirement metadata differs from its inventoried copy"),
                );
                return Ok(());
            }
            Err(error) => {
                ledger.maintenance_failure(current, &error);
                return Ok(());
            }
        }
        if let Err(error) = self.remove_copy(record, &plan, names) {
            ledger.uncertain = true;
            return Err(error);
        }
        let names = ledger
            .references
            .remove(&record.data_file)
            .expect("invariant: retirement retains its reference set until durable deletion");
        for name in names {
            if let Some(reference) = ledger.metadata.remove(&name) {
                ledger.remove_current(&name, &reference);
            }
            // Recovery may have charged this name separately before a later
            // durable publication replaced its opaque bytes with this record.
            if ledger
                .files
                .get(&name)
                .is_some_and(|charge| charge.owner.is_none())
            {
                if let Err(error) = ledger.replace(&name, None) {
                    ledger.uncertain = true;
                    return Err(error);
                }
            }
        }
        if let Err(error) = ledger.replace(&record.data_file, None) {
            ledger.uncertain = true;
            return Err(error);
        }
        Ok(())
    }

    fn copy_metadata_matches(
        &self,
        record: &UploadRecord,
        plan: &RetirementNames,
        names: &BTreeSet<String>,
    ) -> Result<MetadataMatch, ApiError> {
        // Re-read the finite evidence set. Missing phase-one files are valid
        // after an interrupted deletion; current remains the final locator.
        for name in [&plan.current, &plan.owner, &plan.retained] {
            match self
                .directory
                .read_bounded(&name.entry, MAX_METADATA as usize)
            {
                Ok(bytes) => {
                    let matches = if name.key == plan.owner.key {
                        let actual: Owner = storage_contract::decode_strict(&bytes)
                            .map_err(ApiError::invalid_source)?;
                        if actual.data_file != record.data_file
                            || actual.principal != record.principal
                        {
                            return Ok(MetadataMatch::AttributionChanged);
                        }
                        true
                    } else {
                        let actual: UploadRecord = storage_contract::decode_strict(&bytes)
                            .map_err(ApiError::invalid_source)?;
                        if actual.data_file != record.data_file
                            || actual.principal != record.principal
                        {
                            return Ok(MetadataMatch::AttributionChanged);
                        }
                        actual == *record
                    };
                    if !matches || !names.contains(&name.key) {
                        return Ok(MetadataMatch::TargetMismatch);
                    }
                }
                Err(platform_io::fs_safety::Error::Io(error))
                    if error.kind() == std::io::ErrorKind::NotFound
                        && name.key != plan.current.key => {}
                Err(error) => return Err(crate::upload_fs_error(error)),
            }
        }
        Ok(MetadataMatch::Matching)
    }

    fn remove_copy(
        &self,
        record: &UploadRecord,
        plan: &RetirementNames,
        names: &BTreeSet<String>,
    ) -> Result<(), ApiError> {
        match self.directory.entry_metadata(&plan.body) {
            Ok(EntryMetadata::File { bytes }) if bytes == record.logical_size => {}
            Err(platform_io::fs_safety::Error::Io(error))
                if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => return Err(invalid_state("retirement payload changed")),
            Err(error) => return Err(crate::upload_fs_error(error)),
        }
        let mut first = vec![plan.body.clone()];
        // Delete only names attributed to this copy by the inventory/publication
        // index. An absent owner after restart is not permission to remove a new one.
        for name in [&plan.owner, &plan.retained] {
            if names.contains(&name.key) {
                first.push(name.entry.clone());
            }
        }
        self.directory
            .remove_files(&first)
            .map_err(crate::upload_fs_error)?;
        self.directory
            .remove_files(std::slice::from_ref(&plan.current.entry))
            .map_err(crate::upload_fs_error)?;
        Ok(())
    }
}
