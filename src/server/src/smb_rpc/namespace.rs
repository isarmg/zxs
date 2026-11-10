//! Namespace type checks, workspace staging and recoverable commit submission.
use super::*;
use smb_protocol::protocol::NamespaceOperation;
mod directory_export;
pub(super) use directory_export::RetainedExport;

// Only namespace admission uses this registry. It spans the operation lookup,
// record lifetime and workspace submission without holding a mutex across I/O.
struct OperationReservation {
    shared: Arc<Shared>,
    operation_id: OperationId,
}
impl OperationReservation {
    fn acquire(shared: &Arc<Shared>, operation_id: OperationId) -> Result<Self, ApiError> {
        let mut operations = shared
            .namespace_operations
            .lock()
            .expect("service state lock poisoned");
        if !operations.insert(operation_id) {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::RecoveryRequired,
                "namespace operation is already active; query it instead of replaying",
            ));
        }
        Ok(Self {
            shared: shared.clone(),
            operation_id,
        })
    }
}
impl Drop for OperationReservation {
    fn drop(&mut self) {
        match self.shared.namespace_operations.lock() {
            Ok(mut operations) => {
                operations.remove(&self.operation_id);
            }
            Err(_) => {
                // Preserve ownership and poison: no subsequent admission may trust this index.
                tracing::error!("operations cleanup blocked by poisoned registry");
            }
        }
    }
}

pub(super) struct NamespaceInput {
    kind: NamespaceOperation,
    path: String,
    destination: Option<String>,
    replace: bool,
    change: storage_contract::NamespaceOperation,
    source_path: name_policy::LookupPath,
    destination_path: Option<name_policy::PortablePath>,
}
impl NamespaceInput {
    pub(super) fn parse(
        kind: NamespaceOperation,
        path: String,
        destination: Option<String>,
        replace: bool,
    ) -> Result<Self, ApiError> {
        let (change, source_path, destination_path) = match kind {
            NamespaceOperation::Mkdir => {
                let path = name_policy::PortablePath::parse(&path)
                    .map_err(|source| contract(source.into()))?;
                let lookup = name_policy::LookupPath::from(&path);
                (
                    storage_contract::NamespaceOperation::Mkdir { path },
                    lookup,
                    None,
                )
            }
            NamespaceOperation::Unlink | NamespaceOperation::Rmdir => {
                let path = name_policy::LookupPath::parse(&path)
                    .map_err(|source| contract(source.into()))?;
                (
                    storage_contract::NamespaceOperation::Delete { path: path.clone() },
                    path,
                    None,
                )
            }
            NamespaceOperation::Rename => {
                let source = name_policy::LookupPath::parse(&path)
                    .map_err(|source| contract(source.into()))?;
                let target = destination
                    .as_deref()
                    .ok_or_else(|| ApiError::invalid("rename needs a destination"))?;
                let target = name_policy::PortablePath::parse(target)
                    .map_err(|source| contract(source.into()))?;
                (
                    storage_contract::NamespaceOperation::Rename {
                        source: source.clone(),
                        destination: target.clone(),
                    },
                    source,
                    Some(target),
                )
            }
        };
        if kind != NamespaceOperation::Rename && (destination.is_some() || replace) {
            return Err(ApiError::invalid(
                "only rename accepts a destination or replacement",
            ));
        }
        Ok(Self {
            kind,
            path,
            destination,
            replace,
            change,
            source_path,
            destination_path,
        })
    }
}

pub(super) struct NamespaceAdmission {
    record: PathBuf,
    _budget: budget::Lease,
    // Fields drop in declaration order. Keep the operation reserved until its
    // budget lease has removed or retained the durable recovery evidence.
    reservation: OperationReservation,
}
impl NamespaceAdmission {
    pub(super) fn operation_id(&self) -> OperationId {
        self.reservation.operation_id
    }

    pub(super) fn record_intent(&self, value: &Value) -> Result<(), ApiError> {
        write_namespace_record(&self.record, value)
    }

    pub(super) fn finish_published(&self) {
        cleanup_namespace_record(&self.record);
    }

    /// Native move and retaining directory-export adapters consume these
    /// records. Generic namespace/workspace operations remain non-replayable.
    pub(super) fn retain_move(
        shared: &Arc<Shared>,
        identity: &Identity,
        operation_id: OperationId,
    ) -> Result<Option<Self>, ApiError> {
        let reservation = OperationReservation::acquire(shared, operation_id)?;
        let record = shared.spool.join(format!("{operation_id}.namespace.json"));
        match fs::symlink_metadata(&record) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(ApiError::io(error)),
            Ok(metadata) if metadata.is_file() && metadata.nlink() == 1 => {}
            Ok(_) => {
                return Err(ApiError::conflict(
                    storage_contract::ErrorCode::RecoveryRequired,
                    "retained native move record is not one regular file",
                ));
            }
        }
        let budget = shared.budget.retain_namespace(
            &identity.principal,
            &shared.spool.join(operation_id.to_string()),
        )?;
        Ok(Some(Self {
            record,
            _budget: budget,
            reservation,
        }))
    }

    pub(super) fn retained_move_bytes(&self) -> Result<Vec<u8>, ApiError> {
        use platform_io::fs_safety::{EntryName, PrivateDirectory};
        let parent = self
            .record
            .parent()
            .expect("invariant: namespace record is in the spool");
        let directory = PrivateDirectory::open_trusted(parent).map_err(|source| {
            ApiError::conflict(
                storage_contract::ErrorCode::RecoveryRequired,
                "retained move spool is unsafe",
            )
            .with_source(source)
        })?;
        let name = EntryName::new(
            self.record
                .file_name()
                .expect("invariant: namespace has a filename"),
        )
        .map_err(ApiError::invalid_source)?;
        directory.read_bounded(&name, 64 * 1024).map_err(|source| {
            ApiError::conflict(
                storage_contract::ErrorCode::RecoveryRequired,
                "retained move record is unavailable",
            )
            .with_source(source)
        })
    }

    pub(super) fn acquire(
        shared: &Arc<Shared>,
        identity: &Identity,
        operation_id: OperationId,
    ) -> Result<Self, ApiError> {
        let reservation = OperationReservation::acquire(shared, operation_id)?;
        // Namespace is not a replay API. An existing operation or recovery
        // record must be reconciled, not staged into a new workspace.
        match shared.backend.operation_outcome(identity, operation_id) {
            Ok(_) => {
                return Err(ApiError::conflict(
                    storage_contract::ErrorCode::OperationIdConflict,
                    "query the existing operation instead of replaying namespace",
                ));
            }
            Err(error)
                if error.code == storage_contract::ErrorCode::NotFound
                    || error.code == storage_contract::ErrorCode::OperationNotFound => {}
            Err(error) => return Err(error),
        }
        let record = shared.spool.join(format!("{operation_id}.namespace.json"));
        match fs::symlink_metadata(&record) {
            Ok(_) => {
                return Err(ApiError::conflict(
                    storage_contract::ErrorCode::RecoveryRequired,
                    "namespace recovery evidence already exists for this operation",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(ApiError::io(error)),
        }
        let budget = shared.budget.reserve(
            &identity.principal,
            &shared.spool.join(operation_id.to_string()),
            0,
            0,
        )?;
        Ok(Self {
            record,
            _budget: budget,
            reservation,
        })
    }
}

impl Session {
    pub(super) fn namespace(
        &mut self,
        kind: NamespaceOperation,
        path: String,
        destination: Option<String>,
        replace: bool,
        operation_id: OperationId,
    ) -> Result<Value, ApiError> {
        let input = NamespaceInput::parse(kind, path, destination, replace)?;
        if input.kind == NamespaceOperation::Rename
            && let Some(retained) =
                RetainedExport::read(&self.shared, &self.identity, operation_id)?
        {
            return self.retry_directory_export(&input, retained);
        }
        let admission = NamespaceAdmission::acquire(&self.shared, &self.identity, operation_id)?;
        self.namespace_admitted(input, admission)
    }

    fn soft_delete_unlink(
        &self,
        path: &name_policy::LookupPath,
        operation_id: OperationId,
        record: &Path,
    ) -> Result<Value, ApiError> {
        let entry = self.find(path)?.ok_or_else(|| {
            ApiError::new(
                http::StatusCode::NOT_FOUND,
                storage_contract::ErrorCode::FileNotFound,
                "unlink target missing",
            )
        })?;
        if entry.kind != storage_contract::NamespaceEntryKind::File {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::IsDirectory,
                "unlink cannot remove a directory",
            ));
        }
        let file_id = storage_contract::history_window::HistoryFileId::parse(&entry.file_id)
            .map_err(ApiError::contract_input)?;
        // Preserve the exact identity before submission. A later failure does
        // not prove non-publication; query this operation instead of replaying
        // the path against a different file or constructing another workspace.
        write_namespace_record(
            record,
            &json!({
                "format":"smb-file-soft-deletion", "principal":self.identity.principal,
                "branch_id":self.branch, "operation_id":operation_id,
                "publication_outcome":"unconfirmed",
                "delete_request":{"path":path.as_str(),"file_id":file_id.as_str(),"entry_id":entry.entry_id}
            }),
        )?;
        let result = self
            .shared
            .backend
            .delete_current_file(
                &self.identity,
                crate::DeleteCurrentFile {
                    branch_id: self.branch.clone(),
                    path: path.clone(),
                    expected_entry_id: entry.entry_id,
                    file_id,
                    operation_id,
                },
            )
            .and_then(|result| crate::operation_wire::operation_wire(result.into()));
        if result.is_ok() {
            cleanup_namespace_record(record);
        }
        result
    }

    fn soft_delete_rmdir(
        &self,
        path: &name_policy::LookupPath,
        operation_id: OperationId,
        record: &Path,
    ) -> Result<Option<Value>, ApiError> {
        let entry = self.find(path)?.ok_or_else(|| {
            ApiError::new(
                http::StatusCode::NOT_FOUND,
                storage_contract::ErrorCode::DirectoryNotFound,
                "rmdir target missing",
            )
        })?;
        if entry.kind != storage_contract::NamespaceEntryKind::Directory {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::NotDirectory,
                "rmdir requires a directory",
            ));
        }
        let captured = self.shared.backend.prepare_current_directory_deletion(
            &self.identity,
            crate::PrepareCurrentDirectoryDeletion {
                branch_id: self.branch.clone(),
                path: path.clone(),
                expected_entry_id: entry.entry_id,
            },
        )?;
        let Some(captured) = captured else {
            return Ok(None);
        };
        write_namespace_record(
            record,
            &json!({
                "format": "smb-directory-soft-deletion", "principal": self.identity.principal,
                "branch_id": self.branch, "operation_id": operation_id,
                "publication_outcome": "unconfirmed", "delete_request": captured,
            }),
        )?;
        let result = self
            .shared
            .backend
            .delete_current_directory(
                &self.identity,
                crate::DeleteCurrentDirectory {
                    captured,
                    operation_id,
                },
            )
            .and_then(|result| crate::operation_wire::operation_wire(result.into()));
        if result.is_ok() {
            cleanup_namespace_record(record);
        }
        result.map(Some)
    }

    pub(super) fn namespace_admitted(
        &mut self,
        input: NamespaceInput,
        admission: NamespaceAdmission,
    ) -> Result<Value, ApiError> {
        let NamespaceInput {
            kind,
            path,
            destination,
            replace,
            change,
            source_path,
            destination_path,
        } = input;
        let operation_id = admission.reservation.operation_id;
        let record = &admission.record;
        let mut affected = vec![path.as_str()];
        if let Some(target) = destination.as_deref() {
            affected.push(target);
        }
        // Disk-root files and ordinary directories inside initialized owners
        // use their native reversible lifecycle. Named legacy shares retain
        // their policy; no failed lifecycle request falls back to hard delete.
        let soft_delete = if matches!(kind, NamespaceOperation::Unlink | NamespaceOperation::Rmdir)
        {
            match &self.identity.smb_binding {
                Some(binding) => {
                    self.shared
                        .backend
                        .repository_snapshot(&self.identity, &binding.repository_id)?
                        .file_manager_root
                }
                None => false,
            }
        } else {
            false
        };
        let reserved = self.reserve_paths(&affected)?;
        if kind == NamespaceOperation::Rename {
            let target = destination_path
                .as_ref()
                .expect("invariant: rename input has a typed destination");
            match self.try_directory_file_export(&source_path, target, replace, &admission) {
                Ok(None) => {}
                result => {
                    self.release_paths(&reserved);
                    return result.map(|reply| reply.expect("invariant: None was handled"));
                }
            }
        }
        if soft_delete && kind == NamespaceOperation::Rmdir {
            match self.soft_delete_rmdir(&source_path, operation_id, record) {
                Ok(None) => {}
                result => {
                    self.release_paths(&reserved);
                    return result.map(|reply| reply.expect("invariant: None was handled"));
                }
            }
        }
        if soft_delete && kind == NamespaceOperation::Unlink {
            let result = self.soft_delete_unlink(&source_path, operation_id, record);
            self.release_paths(&reserved);
            return result;
        }
        let mut pending_workspace = None;
        let mut submission_started = false;
        let mut record_durable = false;
        let result = (|| {
            let workspace = self.workspace()?;
            pending_workspace = Some(workspace.workspace_id.clone());
            let commit = crate::CommitWorkspace {
                workspace_id: workspace.workspace_id.clone(),
                operation_id,
                expected_head: workspace.base_head.clone(),
                fencing_token: workspace.fencing_token,
                message: "SMB namespace change".into(),
                request_digest: None,
            };
            // Record intent before staging: if preparation and Abort both
            // fail, the workspace still has a recoverable association. A record
            // is not proof that staging or submission started; never replay it.
            write_namespace_record(
                record,
                &json!({
                    "format":"smb-protocol-namespace","principal":self.identity.principal,
                    "branch_id":self.branch,"kind":kind,"path":path,
                    "destination":destination,"replace":replace,"operation_id":operation_id,
                    "publication_outcome":"unconfirmed","commit_request":{
                        "workspace_id":commit.workspace_id,"operation_id":commit.operation_id,
                        "expected_head":commit.expected_head,"fencing_token":commit.fencing_token.to_string(),
                        "message":commit.message
                    }
                }),
            )?;
            record_durable = true;
            // Check exactly the immutable namespace used by this workspace.
            // A later HTTP mutation must not change the meaning of these checks.
            let snapshot = self.shared.backend.snapshot_at(
                &self.identity,
                &self.branch,
                &workspace.base_head,
            )?;
            let entries = &snapshot.entries;
            let find = |key: &str| {
                entries
                    .binary_search_by(|entry| entry.lookup_key().cmp(key))
                    .ok()
                    .map(|index| &entries[index])
            };
            let nonempty = |entry: &Entry| {
                let prefix = format!("{}/", entry.lookup_key());
                entry.kind == storage_contract::NamespaceEntryKind::Directory
                    && entries
                        .iter()
                        .any(|child| child.lookup_key().starts_with(&prefix))
            };
            if kind == NamespaceOperation::Unlink || kind == NamespaceOperation::Rmdir {
                let entry = find(source_path.key()).ok_or_else(|| {
                    ApiError::new(
                        ::http::StatusCode::NOT_FOUND,
                        storage_contract::ErrorCode::NotFound,
                        "remove target missing",
                    )
                })?;
                if kind == NamespaceOperation::Unlink
                    && entry.kind == storage_contract::NamespaceEntryKind::Directory
                {
                    return Err(ApiError::new(
                        ::http::StatusCode::CONFLICT,
                        storage_contract::ErrorCode::IsDirectory,
                        "unlink cannot remove a directory",
                    ));
                }
                if kind == NamespaceOperation::Rmdir
                    && entry.kind != storage_contract::NamespaceEntryKind::Directory
                {
                    return Err(ApiError::new(
                        ::http::StatusCode::CONFLICT,
                        storage_contract::ErrorCode::NotDirectory,
                        "rmdir requires a directory",
                    ));
                }
                if nonempty(entry) {
                    return Err(ApiError::new(
                        ::http::StatusCode::CONFLICT,
                        storage_contract::ErrorCode::DirectoryNotEmpty,
                        "SMB directory removal requires an empty directory",
                    ));
                }
            }
            if kind == NamespaceOperation::Rename
                && replace
                && let Some(target) = destination_path.as_ref()
                && let Some(target_entry) = find(target.key())
            {
                let source = find(source_path.key()).ok_or_else(|| {
                    ApiError::new(
                        ::http::StatusCode::NOT_FOUND,
                        storage_contract::ErrorCode::NotFound,
                        "rename source missing",
                    )
                })?;
                if source.kind != target_entry.kind {
                    let code = match target_entry.kind {
                        storage_contract::NamespaceEntryKind::Directory => {
                            storage_contract::ErrorCode::IsDirectory
                        }
                        storage_contract::NamespaceEntryKind::File => {
                            storage_contract::ErrorCode::NotDirectory
                        }
                    };
                    return Err(ApiError::conflict(
                        code,
                        "rename replacement requires matching entry kinds",
                    ));
                }
                // A case-only rename addresses the same stable entry.
                if source.entry_id != target_entry.entry_id {
                    if nonempty(target_entry) {
                        return Err(ApiError::new(
                            ::http::StatusCode::CONFLICT,
                            storage_contract::ErrorCode::DirectoryNotEmpty,
                            "rename replacement directory is not empty",
                        ));
                    }
                    self.shared.backend.stage_namespace(
                        &self.identity,
                        crate::StageNamespace {
                            workspace_id: workspace.workspace_id.clone(),
                            change: storage_contract::NamespaceOperation::Delete {
                                path: name_policy::LookupPath::from(target),
                            },
                        },
                    )?;
                }
            }
            self.shared.backend.stage_namespace(
                &self.identity,
                crate::StageNamespace {
                    workspace_id: workspace.workspace_id.clone(),
                    change,
                },
            )?;
            submission_started = true;
            self.shared
                .backend
                .commit_workspace(&self.identity, commit)
                .and_then(|result| crate::operation_wire::operation_wire(result.into()))
        })();
        if result.is_ok() {
            cleanup_namespace_record(record);
        } else if !submission_started && let Some(workspace) = pending_workspace {
            match self
                .shared
                .backend
                .abort_workspace(&self.identity, &workspace)
            {
                Ok(_) if record_durable => cleanup_namespace_record(record),
                Ok(_) => {
                    tracing::warn!(%operation_id, workspace, path=%record.display(), "SMB namespace record durability failed; retaining available recovery evidence after abort")
                }
                Err(error) => {
                    tracing::warn!(%operation_id, workspace, path=%record.display(), error = ?error, "SMB namespace preparation abort failed; recovery required")
                }
            }
        }
        // Once submission starts, even a business or transport error is not
        // proof of non-publication. Leave workspace and record untouched.
        self.release_paths(&reserved);
        result
    }
}

#[cfg(all(test, feature = "core"))]
thread_local! {
    static FAIL_NEXT_RECORD_SYNC: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

#[cfg(all(test, feature = "core"))]
pub(super) fn fail_next_record_sync(directory: bool) {
    FAIL_NEXT_RECORD_SYNC.set(Some(directory));
}

fn sync_namespace_record(file: &File, _directory: bool) -> Result<(), ApiError> {
    #[cfg(all(test, feature = "core"))]
    if FAIL_NEXT_RECORD_SYNC.get() == Some(_directory) {
        FAIL_NEXT_RECORD_SYNC.set(None);
        return Err(ApiError::io(io::Error::other(
            "injected namespace record sync failure",
        )));
    }
    file.sync_all().map_err(ApiError::io)
}

fn write_namespace_record(path: &Path, value: &Value) -> Result<(), ApiError> {
    let parent = path
        .parent()
        .ok_or_else(|| ApiError::invalid("namespace record parent missing"))?;
    let mut temporary = tempfile::Builder::new()
        .prefix(".smb-namespace-")
        .tempfile_in(parent)
        .map_err(ApiError::io)?;
    temporary
        .write_all(
            &json_codec::to_vec(value).expect("invariant: internal JSON serialization succeeds"),
        )
        .map_err(ApiError::io)?;
    // Keep the complete intent available even if either sync fails. No core
    // commit may begin until both the file and its directory are durable.
    // A collision never replaces or removes another request's evidence.
    let file = temporary
        .persist_noclobber(path)
        .map_err(|error| ApiError::io(error.error))?;
    sync_namespace_record(&file, false)?;
    sync_namespace_record(&File::open(parent).map_err(ApiError::io)?, true)
}

fn cleanup_namespace_record(path: &Path) {
    let removed = match fs::remove_file(path) {
        Ok(()) => true,
        Err(error) if error.kind() == io::ErrorKind::NotFound => false,
        Err(error) => {
            tracing::warn!(error = ?error, path=%path.display(), "SMB namespace recovery record cleanup failed");
            false
        }
    };
    if removed
        && let Some(parent) = path.parent()
        && let Err(error) = File::open(parent).and_then(|directory| directory.sync_all())
    {
        tracing::warn!(error = ?error, path=%path.display(), "SMB namespace recovery record cleanup was not synced");
    }
}

#[cfg(test)]
#[path = "../../../../test/server/unit/smb_rpc/namespace/tests.rs"]
mod tests;

#[cfg(all(test, feature = "core"))]
#[path = "../../../../test/server/unit/smb_rpc/reservation_drop_tests.rs"]
mod drop_tests;
