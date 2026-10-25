//! Per-user session, namespace dispatch and writer-lock ownership.
use super::*;

pub(super) struct Session {
    pub(super) shared: Arc<Shared>,
    pub(super) identity: Identity,
    pub(super) branch: String,
    pub(super) handles: BTreeMap<u64, Handle>,
    pub(super) next_handle: u64,
    pub(super) writer_locks: BTreeSet<String>,
    pub(super) max_file_bytes: u64,
}

#[derive(Debug)]
pub(super) struct DispatchError {
    pub(super) error: ApiError,
    pub(super) operation_id: Option<String>,
}
impl std::ops::Deref for DispatchError {
    type Target = ApiError;
    fn deref(&self) -> &Self::Target {
        &self.error
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        if let Ok(mut writers) = self.shared.file_writers.lock() {
            for path in &self.writer_locks {
                writers.remove(&(self.branch.clone(), path.clone()));
            }
        }
        // Every live writer has handed out a mutable FD. The peer can have
        // changed it before a Dirty request reached us, including after a
        // successful Flush. A disconnect therefore cannot prove clean bytes.
        // Retain both data and the recovery record without hashing in Drop or
        // publishing a new generation. Explicit successful Close still cleans up.
    }
}

impl Session {
    // Operations implemented without an engine action (the synthetic share
    // root) still have to validate their entrance. Mutations also revalidate
    // inside Backend, not just at this preliminary check.
    pub(super) fn authorize_binding(&self) -> Result<(), ApiError> {
        if let Some(binding) = &self.identity.smb_binding {
            self.call(
                crate::BackendAction::Repository,
                json!({"repository_id":binding.repository_id}),
            )?;
        }
        Ok(())
    }
    pub(super) fn call(
        &self,
        action: crate::BackendAction,
        input: Value,
    ) -> Result<Value, ApiError> {
        self.shared.backend.call(action, &self.identity, input)
    }
    pub(super) fn find(&self, path: &str) -> Result<Option<Entry>, ApiError> {
        json_codec::from_value(self.call(
            crate::BackendAction::SmbLookup,
            json!({"branch_id":self.branch,"path":path}),
        )?)
        .map_err(ApiError::invalid_source)
    }
    pub(super) fn workspace(&self) -> Result<Workspace, ApiError> {
        json_codec::from_value(self.call(
            crate::BackendAction::CreateWorkspace,
            json!({"branch_id":self.branch}),
        )?)
        .map_err(ApiError::invalid_source)
    }
    pub(super) fn reserve_paths(&mut self, paths: &[&str]) -> Result<Vec<String>, ApiError> {
        let keys = paths
            .iter()
            .map(|path| name_policy::path_key(path))
            .collect::<BTreeSet<_>>();
        let overlaps = |left: &str, right: &str| {
            left == right
                || left.starts_with(&format!("{right}/"))
                || right.starts_with(&format!("{left}/"))
        };
        let mut writers = self
            .shared
            .file_writers
            .lock()
            .expect("service state lock poisoned");
        if writers.iter().any(|(branch, existing)| {
            branch == &self.branch && keys.iter().any(|key| overlaps(existing, key))
        }) {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::FileWriteBusy,
                "an affected SMB file has an active write handle",
            ));
        }
        for key in &keys {
            writers.insert((self.branch.clone(), key.clone()));
            self.writer_locks.insert(key.clone());
        }
        Ok(keys.into_iter().collect())
    }
    pub(super) fn release_paths(&mut self, paths: &[String]) {
        if let Ok(mut writers) = self.shared.file_writers.lock() {
            for path in paths {
                writers.remove(&(self.branch.clone(), path.clone()));
                self.writer_locks.remove(path);
            }
        }
    }
    pub(super) fn dispatch(
        &mut self,
        request: Request,
    ) -> Result<(Value, Option<OwnedFd>), DispatchError> {
        let is_namespace = matches!(&request, Request::Namespace { .. });
        let dirty_handle = match &request {
            Request::Dirty { handle } => Some(*handle),
            _ => None,
        };
        let mut operation_id = match &request {
            Request::Namespace { operation_id, .. } => {
                storage_contract::validate_id(operation_id, "op").map_err(|error| {
                    DispatchError {
                        error: contract(error),
                        operation_id: None,
                    }
                })?;
                Some(operation_id.clone())
            }
            Request::Flush { handle } | Request::Close { handle } | Request::Dirty { handle } => {
                match self.handles.get(handle) {
                    Some(Handle::Write(writer)) => Some(writer.operation_id.clone()),
                    _ => None,
                }
            }
            _ => None,
        };
        let result = self.dispatch_request(request);
        // Dirty can create a new generation and submit it for write-through.
        // Report that generation, not the previously published operation.
        if let Some(handle) = dirty_handle
            && let Some(Handle::Write(writer)) = self.handles.get(&handle)
        {
            operation_id = Some(writer.operation_id.clone());
        }
        result.map_err(|mut error| {
            // Namespace has no replay protocol. Even a transient error must
            // not advertise that an uncertain mutation can be issued again.
            if is_namespace {
                error.retryable = false;
            }
            DispatchError {
                error,
                operation_id,
            }
        })
    }

    fn dispatch_request(&mut self, request: Request) -> Result<(Value, Option<OwnedFd>), ApiError> {
        let value = match request {
            Request::Hello { .. } => return Err(ApiError::invalid("hello is allowed only once")),
            Request::Open {
                path,
                writable,
                create,
                truncate,
                exclusive,
                write_through,
            } => return self.open(path, writable, create, truncate, exclusive, write_through),
            Request::AuthorizeRead { handle } => self.authorize_read(handle)?,
            Request::PrepareWrite { handle, end, bytes } => {
                let Some(Handle::Write(writer)) = self.handles.get(&handle) else {
                    return Err(ApiError::forbidden());
                };
                self.call(
                    crate::BackendAction::WorkspaceSelectionAuthorize,
                    json!({"workspace_id":writer.workspace.workspace_id,"path":writer.path}),
                )?;
                let current = writer.file.metadata().map_err(ApiError::io)?.len();
                writer
                    .reservation
                    .before_write(end, bytes.max(end.saturating_sub(current)))?;
                json!({"ready":true})
            }
            Request::Dirty { handle } => self.dirty(handle)?,
            Request::Flush { handle } => self.flush(handle)?,
            Request::Close { handle } => self.close(handle, false)?,
            Request::Detach { handle } => self.detach(handle)?,
            Request::Abort { handle } => self.close(handle, true)?,
            Request::Stat { path } => {
                if path.is_empty() || path == "." {
                    self.authorize_binding()?;
                    json!({"path":"","kind":"directory","size":"0","entry_id":self.branch})
                } else {
                    name_policy::validate_lookup_path(&path).map_err(contract)?;
                    json_codec::to_value(self.find(&path)?.ok_or_else(|| {
                        ApiError::new(
                            ::http::StatusCode::NOT_FOUND,
                            storage_contract::ErrorCode::NotFound,
                            "entry missing",
                        )
                    })?)
                    .expect("invariant: internal JSON serialization succeeds")
                }
            }
            Request::ReadDirectory { handle, restart } => self.read_directory(handle, restart)?,
            Request::Namespace {
                kind,
                path,
                destination,
                replace,
                operation_id,
            } => self.namespace(kind, path, destination, replace, &operation_id)?,
        };
        Ok((value, None))
    }
}
