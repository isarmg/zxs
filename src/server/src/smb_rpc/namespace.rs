//! Namespace type checks, workspace staging and recoverable commit submission.
use super::*;
use platform_io::fs_safety::{EntryName, PrivateDirectory};
use smb_protocol::protocol::NamespaceOperation;

#[cfg(not(test))]
use platform_io::fs_safety::AtomicFile;
#[cfg(test)]
#[path = "../../../../test/server/unit/smb_rpc/namespace/persistence.rs"]
mod persist;
#[cfg(test)]
use persist::AtomicFile;
#[cfg(all(test, feature = "core"))]
pub(super) use persist::{fail_before_record_write, fail_next_record_sync};

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
        if let Some(mut operations) = crate::lock_for_terminal_release(
            &self.shared.namespace_operations,
            "SMB namespace operations",
        ) {
            operations.remove(&self.operation_id);
        }
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
        let _operation = OperationReservation::acquire(&self.shared, operation_id)?;
        // Namespace is not a replay API. An existing operation or recovery
        // record must be reconciled, not staged into a new workspace.
        match self
            .shared
            .backend
            .operation_outcome(&self.identity, operation_id)
        {
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
        let record_name = EntryName::new(format!("{operation_id}.namespace.json"))
            .expect("invariant: operation ID forms one namespace record name");
        let record = self
            .shared
            .budget
            .directory
            .path()
            .join(record_name.as_path());
        match self.shared.budget.directory.entry_metadata(&record_name) {
            Ok(_)
            | Err(platform_io::fs_safety::Error::UnsafeFileType(_))
            | Err(platform_io::fs_safety::Error::MultipleLinks(_)) => {
                return Err(ApiError::conflict(
                    storage_contract::ErrorCode::RecoveryRequired,
                    "namespace recovery evidence already exists for this operation",
                ));
            }
            Err(platform_io::fs_safety::Error::Io(error))
                if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(namespace_record_error(error)),
        }
        let budget = self.shared.budget.reserve(
            &self.identity.principal,
            &self.shared.spool.join(operation_id.to_string()),
            0,
            0,
        )?;
        let mut affected = vec![path.as_str()];
        if let Some(target) = destination.as_deref() {
            affected.push(target);
        }
        let reserved = match self.reserve_paths(&affected) {
            Ok(reserved) => reserved,
            Err(error) => {
                if let Err(cleanup) = budget.cleanup(budget::Evidence::Reservation) {
                    tracing::warn!(error = ?cleanup, %operation_id, "SMB namespace admission cleanup incomplete");
                }
                return Err(error);
            }
        };
        let mut pending_workspace = None;
        let mut submission_started = false;
        let mut record_durable = false;
        let mut result = (|| {
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
                &self.shared.budget.directory,
                &record_name,
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
            // A later HTTP mutation must not change the meaning of these
            // checks: every read below names the workspace's own base head.
            self.shared
                .backend
                .snapshot_at(&self.identity, &self.branch, &workspace.base_head)?;
            let find = |key: &str| {
                self.shared.backend.snapshot_entry(
                    &self.identity,
                    &self.branch,
                    &workspace.base_head,
                    key,
                )
            };
            let nonempty = |entry: &Entry| -> Result<bool, ApiError> {
                Ok(
                    entry.kind == storage_contract::NamespaceEntryKind::Directory
                        && self.shared.backend.snapshot_directory_has_children(
                            &self.identity,
                            &self.branch,
                            &workspace.base_head,
                            entry.lookup_key(),
                        )?,
                )
            };
            if kind == NamespaceOperation::Unlink || kind == NamespaceOperation::Rmdir {
                let entry = find(source_path.key())?.ok_or_else(|| {
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
                if nonempty(&entry)? {
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
                && let Some(target_entry) = find(target.key())?
            {
                let source = find(source_path.key())?.ok_or_else(|| {
                    ApiError::new(
                        ::http::StatusCode::NOT_FOUND,
                        storage_contract::ErrorCode::NotFound,
                        "rename source missing",
                    )
                })?;
                if source.kind != target_entry.kind {
                    return Err(ApiError::invalid(
                        "rename replacement requires matching entry kinds",
                    ));
                }
                // A case-only rename addresses the same stable entry.
                if source.entry_id != target_entry.entry_id {
                    if nonempty(&target_entry)? {
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
            if let Err(error) = budget.cleanup(budget::Evidence::Namespace) {
                result = Err(publication::cleanup_error(error));
            }
        } else if !submission_started && let Some(workspace) = pending_workspace.as_ref() {
            match self
                .shared
                .backend
                .abort_workspace(&self.identity, workspace)
            {
                Ok(_) if record_durable => {
                    if let Err(error) = budget.cleanup(budget::Evidence::Namespace) {
                        tracing::warn!(error = ?error, %operation_id, "SMB namespace preparation cleanup incomplete");
                    }
                }
                Ok(_) => {
                    tracing::warn!(%operation_id, workspace, path=%record.display(), "SMB namespace record durability failed; retaining available recovery evidence after abort")
                }
                Err(error) => {
                    tracing::warn!(%operation_id, workspace, path=%record.display(), error = ?error, "SMB namespace preparation abort failed; recovery required")
                }
            }
        } else if pending_workspace.is_none()
            && let Err(error) = budget.cleanup(budget::Evidence::Reservation)
        {
            tracing::warn!(error = ?error, %operation_id, "SMB namespace workspace admission cleanup incomplete");
        }
        // Once submission starts, even a business or transport error is not
        // proof of non-publication. Leave workspace and record untouched.
        self.release_paths(&reserved);
        result
    }
}

fn namespace_record_error(error: platform_io::fs_safety::Error) -> ApiError {
    ApiError::new(
        ::http::StatusCode::INTERNAL_SERVER_ERROR,
        storage_contract::ErrorCode::IoError,
        "local storage operation failed",
    )
    .with_source(error)
}

fn write_namespace_record(
    directory: &PrivateDirectory,
    name: &EntryName,
    value: &Value,
) -> Result<(), ApiError> {
    directory
        .verify_identity()
        .map_err(namespace_record_error)?;
    // Keep the complete intent available even if either sync fails. No core
    // commit may begin until both the file and its directory are durable.
    // A collision never replaces or removes another request's evidence.
    let bytes = json_codec::to_vec(value).expect("invariant: internal JSON serialization succeeds");
    AtomicFile::create_recovery_intent(directory, name, &bytes).map_err(namespace_record_error)
}

#[cfg(test)]
#[path = "../../../../test/server/unit/smb_rpc/namespace/tests.rs"]
mod tests;
