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
        {
            let mut writers = self
                .shared
                .file_writers
                .lock()
                .expect("service state lock poisoned");
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
            self.shared
                .backend
                .authorize_repository(&self.identity, &binding.repository_id)?;
        }
        Ok(())
    }
    pub(super) fn find(&self, path: &name_policy::LookupPath) -> Result<Option<Entry>, ApiError> {
        self.shared
            .backend
            .lookup_current(&self.identity, &self.branch, path)
    }
    pub(super) fn workspace(&self) -> Result<Workspace, ApiError> {
        self.shared.backend.create_workspace(
            &self.identity,
            crate::CreateWorkspace {
                branch_id: self.branch.clone(),
                source_commit_id: None,
            },
        )
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
        {
            let mut writers = self
                .shared
                .file_writers
                .lock()
                .expect("service state lock poisoned");
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
        let request = request
            .parse_operation_id()
            .map_err(|error| DispatchError {
                error: ApiError::invalid_source(error),
                operation_id: None,
            })?;
        let is_namespace = matches!(&request, Request::Namespace { .. });
        let dirty_handle = match &request {
            Request::Dirty { handle } => Some(*handle),
            _ => None,
        };
        let mut operation_id = match &request {
            Request::Namespace { operation_id, .. } => Some(operation_id.to_string()),
            Request::Flush { handle } | Request::Close { handle } | Request::Dirty { handle } => {
                match self.handles.get(handle) {
                    Some(Handle::Write(writer)) => Some(writer.operation_id.to_string()),
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
            operation_id = Some(writer.operation_id.to_string());
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

    fn dispatch_request(
        &mut self,
        request: Request<OperationId>,
    ) -> Result<(Value, Option<OwnedFd>), ApiError> {
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
                self.prepare_write(handle, end, bytes)?
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
                    let path = name_policy::LookupPath::parse(&path)
                        .map_err(|source| contract(source.into()))?;
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
            } => self.namespace(kind, path, destination, replace, operation_id)?,
        };
        Ok((value, None))
    }
}
