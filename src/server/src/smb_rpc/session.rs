//! Per-user session, namespace dispatch and writer-lock ownership.
use super::*;

pub(super) struct Session {
    pub(super) shared: Arc<Shared>,
    pub(super) account_generation: u64,
    pub(super) identity: Identity,
    pub(super) branch: String,
    pub(super) handles: BTreeMap<u64, Handle>,
    pub(super) next_handle: u64,
    pub(super) max_file_bytes: u64,
    pub(super) read_cache: Option<(u64, crate::verified_chunk::VerifiedChunk)>,
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
    pub(super) fn container_context(&self, entry: &Entry)
        -> Result<Option<crate::ReadContainerEdit>, ApiError>
    {
        let repository = if let Some(binding) = &self.identity.smb_binding {
            Some(binding.repository_id.clone())
        } else {
            // Internal historical sessions retain historical semantics. Only an
            // exact current repository branch has shared-container authority.
            self.shared.backend.repositories_snapshot(&self.identity)?.into_iter()
                .find(|repository| repository.main_branch_id == self.branch)
                .map(|repository| repository.repository_id)
        };
        repository.map(|repository| Ok(crate::ReadContainerEdit {
            file: entry.file_id.parse().map_err(ApiError::contract_input)?,
            repository: repository.parse().map_err(ApiError::contract_input)?,
            path: entry.path.clone(),
        })).transpose()
    }

    pub(super) fn find(&self, path: &name_policy::LookupPath) -> Result<Option<Entry>, ApiError> {
        self.shared
            .backend
            .lookup_current(&self.identity, &self.branch, path)
    }
    pub(super) fn dispatch(&mut self, request: Request) -> Result<Reply, DispatchError> {
        let request = request
            .parse_operation_id()
            .map_err(|error| DispatchError {
                error: ApiError::invalid_source(error),
                operation_id: None,
            })?;
        self.dispatch_admitted(request)
    }

    pub(super) fn dispatch_admitted(
        &mut self,
        request: Request<OperationId>,
    ) -> Result<Reply, DispatchError> {
        let is_namespace = matches!(&request, Request::Namespace { .. });
        let dirty_handle = match &request {
            Request::Dirty { handle } => Some(*handle),
            _ => None,
        };
        let mut operation_id = match &request {
            Request::Namespace { operation_id, .. }
            | Request::RecoverManualDraft { operation_id, .. }
            | Request::RecoverManualWorking { operation_id, .. } => Some(operation_id.to_string()),
            Request::Flush { handle } | Request::Close { handle } | Request::Dirty { handle } => {
                match self.handles.get(handle) {
                    Some(Handle::Write(writer, _)) => Some(writer.operation_id.to_string()),
                    _ => None,
                }
            }
            _ => None,
        };
        let account = self.shared.account.clone();
        let result = (|| {
            let binding = account.read()?;
            binding.require_generation(self.account_generation)?;
            self.dispatch_request(request)
        })();
        // Dirty can create a new generation and submit it for write-through.
        // Report that generation, not the previously published operation.
        if let Some(handle) = dirty_handle
            && let Some(Handle::Write(writer, _)) = self.handles.get(&handle)
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

    fn dispatch_request(&mut self, request: Request<OperationId>) -> Result<Reply, ApiError> {
        let value = match request {
            Request::Hello { .. } | Request::HelloDefault { .. } => {
                return Err(ApiError::invalid("hello is allowed only once"));
            }
            Request::RecoverManualWorking { operation_id, record, resume } => {
                return working_recovery::recover(
                    &self.shared, &self.identity, operation_id, &record, self.max_file_bytes, resume,
                ).map(Reply::Json);
            }
            Request::RecoverManualDraft { operation_id, resume } => {
                return manual_recovery::recover(
                    &self.shared, &self.identity, operation_id, self.max_file_bytes, resume,
                ).map(Reply::Json);
            }
            Request::Open {
                path,
                writable,
                create,
                truncate,
                exclusive,
                write_through,
            } => {
                return self
                    .open(path, writable, create, truncate, exclusive, write_through)
                    .map(|(metadata, descriptor)| Reply::Open {
                        metadata,
                        descriptor,
                    });
            }
            Request::AuthorizeRead { handle } => {
                return self
                    .handle_metadata(self.authorize_read(handle)?)
                    .map(Reply::Metadata);
            }
            Request::Read {
                handle,
                offset,
                length,
            } => {
                return self.read(handle, offset, length).map(Reply::Read);
            }
            Request::PrepareWrite { handle, end, bytes } => {
                self.prepare_write(handle, end, bytes)?
            }
            Request::Dirty { handle } => self.dirty(handle)?,
            Request::Flush { handle } => self.flush(handle)?,
            Request::Close { handle } => self.close(handle, false)?,
            Request::Detach { handle } => self.detach(handle)?,
            Request::Abort { handle } => self.close(handle, true)?,
            Request::Stat { path } => {
                return self.stat(&path).map(Reply::Stat);
            }
            Request::ReadDirectory { handle, restart } => {
                return self.read_directory(handle, restart).map(Reply::Directory);
            }
            Request::Namespace {
                kind,
                path,
                destination,
                replace,
                operation_id,
            } => self.namespace(kind, path, destination, replace, operation_id)?,
        };
        Ok(Reply::Json(value))
    }
}
