//! Namespace type checks, workspace staging and recoverable commit submission.
use super::*;

// Only namespace admission uses this registry. It spans the operation lookup,
// record lifetime and workspace submission without holding a mutex across I/O.
struct OperationReservation {
    shared: Arc<Shared>,
    operation_id: String,
}
impl OperationReservation {
    fn acquire(shared: &Arc<Shared>, operation_id: &str) -> Result<Self, ApiError> {
        let mut operations = shared
            .namespace_operations
            .lock()
            .map_err(|_| ApiError::new(500, "INTERNAL_ERROR", "SMB operation registry poisoned"))?;
        if !operations.insert(operation_id.to_owned()) {
            return Err(ApiError::conflict(
                "RECOVERY_REQUIRED",
                "namespace operation is already active; query it instead of replaying",
            ));
        }
        Ok(Self {
            shared: shared.clone(),
            operation_id: operation_id.to_owned(),
        })
    }
}
impl Drop for OperationReservation {
    fn drop(&mut self) {
        if let Ok(mut operations) = self.shared.namespace_operations.lock() {
            operations.remove(&self.operation_id);
        }
    }
}

impl Session {
    pub(super) fn namespace(
        &mut self,
        kind: String,
        path: String,
        destination: Option<String>,
        replace: bool,
        operation_id: &str,
    ) -> Result<Value, ApiError> {
        // Validate before this client-provided identifier can reach a path.
        storage_contract::validate_id(operation_id, "op").map_err(contract)?;
        if !["mkdir", "rename", "unlink", "rmdir"].contains(&kind.as_str()) {
            return Err(ApiError::unsupported());
        }
        if kind == "mkdir" {
            name_policy::validate_path(&path).map_err(contract)?;
        } else {
            name_policy::validate_lookup_path(&path).map_err(contract)?;
        }
        if kind == "rename" {
            let target = destination
                .as_deref()
                .ok_or_else(|| ApiError::invalid("rename needs a destination"))?;
            name_policy::validate_path(target).map_err(contract)?;
        } else if destination.is_some() || replace {
            return Err(ApiError::invalid(
                "only rename accepts a destination or replacement",
            ));
        }
        let _operation = OperationReservation::acquire(&self.shared, operation_id)?;
        // Namespace is not a replay API. An existing operation or recovery
        // record must be reconciled, not staged into a new workspace.
        match self.call(
            crate::BackendAction::Operation,
            json!({"operation_id":operation_id}),
        ) {
            Ok(_) => {
                return Err(ApiError::conflict(
                    "OPERATION_ID_CONFLICT",
                    "query the existing operation instead of replaying namespace",
                ));
            }
            Err(error) if error.code == "NOT_FOUND" || error.code == "OPERATION_NOT_FOUND" => {}
            Err(error) => return Err(error),
        }
        let record = self
            .shared
            .spool
            .join(format!("{operation_id}.namespace.json"));
        match fs::symlink_metadata(&record) {
            Ok(_) => {
                return Err(ApiError::conflict(
                    "RECOVERY_REQUIRED",
                    "namespace recovery evidence already exists for this operation",
                ));
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(ApiError::io(error)),
        }
        let _budget = self.shared.budget.reserve(
            &self.identity.principal,
            &self.shared.spool.join(operation_id),
            0,
            0,
        )?;
        let mut affected = vec![path.as_str()];
        if let Some(target) = destination.as_deref() {
            affected.push(target);
        }
        let reserved = self.reserve_paths(&affected)?;
        let mut pending_workspace = None;
        let mut submission_started = false;
        let mut record_durable = false;
        let result = (|| {
            let workspace = self.workspace()?;
            pending_workspace = Some(workspace.workspace_id.clone());
            let commit = json!({
                "workspace_id":workspace.workspace_id,"operation_id":operation_id,
                "expected_head":workspace.base_head,"fencing_token":workspace.fencing_token.to_string(),
                "message":"SMB namespace change"
            });
            // Record intent before staging: if preparation and Abort both
            // fail, the workspace still has a recoverable association. A record
            // is not proof that staging or submission started; never replay it.
            write_namespace_record(
                &record,
                &json!({
                    "format":"smb-protocol-namespace","principal":self.identity.principal,
                    "branch_id":self.branch,"kind":kind,"path":path,
                    "destination":destination,"replace":replace,"operation_id":operation_id,
                    "publication_outcome":"unconfirmed","commit_request":commit
                }),
            )?;
            record_durable = true;
            // Check exactly the immutable namespace used by this workspace.
            // A later HTTP mutation must not change the meaning of these checks.
            let tree = self.call(
                crate::BackendAction::Tree,
                json!({"branch_id":self.branch,"query":{"commit_id":workspace.base_head}}),
            )?;
            let entries: Vec<Entry> =
                json_codec::from_value(tree["entries"].clone()).map_err(ApiError::invalid)?;
            let find = |path: &str| {
                let key = name_policy::path_key(path);
                entries.iter().find(|entry| entry.lookup_key == key)
            };
            let nonempty = |entry: &Entry| {
                let prefix = format!("{}/", entry.lookup_key);
                entry.kind == "directory"
                    && entries
                        .iter()
                        .any(|child| child.lookup_key.starts_with(&prefix))
            };
            if kind == "unlink" || kind == "rmdir" {
                let entry = find(&path)
                    .ok_or_else(|| ApiError::new(404, "NOT_FOUND", "remove target missing"))?;
                if kind == "unlink" && entry.kind == "directory" {
                    return Err(ApiError::new(
                        409,
                        "IS_DIRECTORY",
                        "unlink cannot remove a directory",
                    ));
                }
                if kind == "rmdir" && entry.kind != "directory" {
                    return Err(ApiError::new(
                        409,
                        "NOT_DIRECTORY",
                        "rmdir requires a directory",
                    ));
                }
                if nonempty(entry) {
                    return Err(ApiError::new(
                        409,
                        "DIRECTORY_NOT_EMPTY",
                        "SMB directory removal requires an empty directory",
                    ));
                }
            }
            if kind == "rename"
                && replace
                && let Some(target) = destination.as_deref()
                && let Some(target_entry) = find(target)
            {
                let source = find(&path)
                    .ok_or_else(|| ApiError::new(404, "NOT_FOUND", "rename source missing"))?;
                if source.kind != target_entry.kind {
                    return Err(ApiError::invalid(
                        "rename replacement requires matching entry kinds",
                    ));
                }
                // A case-only rename addresses the same stable entry.
                if source.entry_id != target_entry.entry_id {
                    if nonempty(target_entry) {
                        return Err(ApiError::new(
                            409,
                            "DIRECTORY_NOT_EMPTY",
                            "rename replacement directory is not empty",
                        ));
                    }
                    self.call(crate::BackendAction::Namespace, json!({
                        "workspace_id":workspace.workspace_id,"kind":"delete","path":target,"destination":null
                    }))?;
                }
            }
            let core_kind = if kind == "unlink" || kind == "rmdir" {
                "delete"
            } else {
                &kind
            };
            self.call(crate::BackendAction::Namespace, json!({
                "workspace_id":workspace.workspace_id,"kind":core_kind,"path":path,"destination":destination
            }))?;
            submission_started = true;
            self.call(crate::BackendAction::CommitWorkspaceInternal, commit)
        })();
        if result.is_ok() {
            cleanup_namespace_record(&record);
        } else if !submission_started && let Some(workspace) = pending_workspace {
            match self.call(
                crate::BackendAction::AbortWorkspace,
                json!({"workspace_id":workspace}),
            ) {
                Ok(_) if record_durable => cleanup_namespace_record(&record),
                Ok(_) => {
                    tracing::warn!(operation_id, workspace, path=%record.display(), "SMB namespace record durability failed; retaining available recovery evidence after abort")
                }
                Err(error) => {
                    tracing::warn!(operation_id, workspace, path=%record.display(), code=%error.code, message=%error.message, "SMB namespace preparation abort failed; recovery required")
                }
            }
        }
        // Once submission starts, even a business or transport error is not
        // proof of non-publication. Leave workspace and record untouched.
        self.release_paths(&reserved);
        result
    }
}

#[cfg(test)]
thread_local! {
    static FAIL_NEXT_RECORD_SYNC: std::cell::Cell<Option<bool>> = const { std::cell::Cell::new(None) };
}

#[cfg(test)]
pub(super) fn fail_next_record_sync(directory: bool) {
    FAIL_NEXT_RECORD_SYNC.set(Some(directory));
}

fn sync_namespace_record(file: &File, _directory: bool) -> Result<(), ApiError> {
    #[cfg(test)]
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
        .write_all(&json_codec::to_vec(value).map_err(ApiError::invalid)?)
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
            tracing::warn!(%error, path=%path.display(), "SMB namespace recovery record cleanup failed");
            false
        }
    };
    if removed
        && let Some(parent) = path.parent()
        && let Err(error) = File::open(parent).and_then(|directory| directory.sync_all())
    {
        tracing::warn!(%error, path=%path.display(), "SMB namespace recovery record cleanup was not synced");
    }
}

#[cfg(test)]
#[path = "../../../../test/server/unit/smb_rpc/namespace/tests.rs"]
mod tests;
