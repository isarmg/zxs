//! Default SMB namespace: fixed primary-disk roots and existing named repositories.
//! Routing never merges histories; each open handle retains its original session.
use super::session::DispatchError;
use super::*;
use crate::{
    ListingVersion, ReadListing, ResolveSmbPath, SmbPathIntent, SmbPathLocation, TopologySlot,
};
use smb_protocol::protocol::{FileMtime, NamespaceOperation, StatResult};
mod directory;
mod history_move;
use directory::VirtualDirectory;

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SessionKey {
    repository: String,
    branch: String,
    generation: u64,
}

pub(super) struct DefaultShare {
    shared: Arc<Shared>,
    identity: Identity,
    account_generation: u64,
    max_file_bytes: u64,
    sessions: BTreeMap<SessionKey, Session>,
    // Remember visited bindings even after their last handle closes. A later
    // disable/rebind must not revive an old connection.
    bindings: BTreeMap<String, SessionKey>,
    handles: BTreeMap<u64, RoutedHandle>,
    next_handle: u64,
}

enum RoutedHandle {
    Directory(VirtualDirectory),
    Repository { session: SessionKey, handle: u64 },
}

enum Location {
    Root,
    Disk(TopologySlot),
    Path {
        slot: TopologySlot,
        path: name_policy::LookupPath,
    },
}

fn failure(error: ApiError, operation_id: Option<String>) -> DispatchError {
    DispatchError {
        error,
        operation_id,
    }
}
fn missing() -> ApiError {
    ApiError::new(
        ::http::StatusCode::NOT_FOUND,
        storage_contract::ErrorCode::NotFound,
        "SMB directory or handle not found",
    )
}
fn changed() -> ApiError {
    ApiError::conflict(
        storage_contract::ErrorCode::ListingChanged,
        "default SMB directory changed; reopen or restart the directory",
    )
}
fn directory_only() -> ApiError {
    ApiError::new(
        ::http::StatusCode::CONFLICT,
        storage_contract::ErrorCode::IsDirectory,
        "the default share root and disk directories are not file contents",
    )
}
fn json_reply(value: impl data_serialization::Serialize) -> (Reply, Option<OwnedFd>) {
    (
        Reply::Json(json_codec::to_value(value).expect("invariant: SMB metadata serializes")),
        None,
    )
}

impl DefaultShare {
    pub(super) fn new(
        shared: Arc<Shared>,
        identity: Identity,
        account_generation: u64,
        max_file_bytes: u64,
    ) -> Result<Self, ApiError> {
        // Authentication/storage readiness is checked before accepting a root session.
        shared.backend.topology_snapshot(&identity)?;
        Ok(Self {
            shared,
            identity,
            account_generation,
            max_file_bytes,
            sessions: BTreeMap::new(),
            bindings: BTreeMap::new(),
            handles: BTreeMap::new(),
            next_handle: 1,
        })
    }

    fn locate(&self, path: &str) -> Result<Location, ApiError> {
        let topology = self.shared.backend.topology_snapshot(&self.identity)?;
        if path.is_empty() || path == "." {
            return Ok(Location::Root);
        }
        let (slot, within) = path.split_once('/').unwrap_or((path, ""));
        let key = name_policy::lookup_key(slot);
        let slot = topology
            .slots
            .into_iter()
            .find(|candidate| {
                candidate.level == 1
                    && candidate
                        .name
                        .as_ref()
                        .is_some_and(|name| name.key() == key)
            })
            .ok_or_else(missing)?;
        if within.is_empty() {
            return Ok(Location::Disk(slot));
        }
        let path = name_policy::LookupPath::parse(within).map_err(ApiError::invalid_source)?;
        Ok(Location::Path { slot, path })
    }

    fn resolve(&self, path: &str, intent: SmbPathIntent) -> Result<SmbPathLocation, ApiError> {
        let Location::Path { slot, path } = self.locate(path)? else {
            return Err(directory_only());
        };
        self.resolve_at(&slot, path, intent)
    }

    // Reuse this request's parsed display location. Backend resolution and the
    // eventual publication still check live disk/repository identity.
    fn resolve_at(
        &self,
        slot: &TopologySlot,
        path: name_policy::LookupPath,
        intent: SmbPathIntent,
    ) -> Result<SmbPathLocation, ApiError> {
        self.shared.backend.resolve_smb_path(
            &self.identity,
            ResolveSmbPath {
                slot_id: slot.slot_id.clone(),
                path,
                intent,
            },
        )
    }

    fn pin_binding(
        &mut self,
        repository: storage_contract::Repository,
    ) -> Result<SessionKey, ApiError> {
        if !repository.smb_enabled {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::SmbRebindRequired,
                "this namespace requires an explicit SMB binding after history migration",
            ));
        }
        let key = SessionKey {
            repository: repository.repository_id,
            branch: repository.main_branch_id,
            generation: repository.smb_binding_generation,
        };
        if let Some(previous) = self.bindings.get(&key.repository) {
            if previous != &key {
                return Err(ApiError::conflict(
                    storage_contract::ErrorCode::SmbRebindRequired,
                    "SMB namespace binding changed; reconnect before starting another operation",
                ));
            }
        } else {
            if self.bindings.len() >= MAX_HANDLES {
                return Err(ApiError::limit(
                    "too many visited SMB namespaces; reconnect before opening another",
                ));
            }
            self.bindings.insert(key.repository.clone(), key.clone());
        }
        Ok(key)
    }

    fn bound_identity(&self, key: &SessionKey) -> Identity {
        let mut identity = self.identity.clone();
        identity.smb_binding = Some(crate::auth::SmbBinding {
            repository_id: key.repository.clone(),
            branch_id: key.branch.clone(),
            generation: key.generation,
        });
        identity
    }

    fn session_key(
        &mut self,
        repository: storage_contract::Repository,
    ) -> Result<SessionKey, ApiError> {
        let key = self.pin_binding(repository)?;
        self.sessions.entry(key.clone()).or_insert_with(|| {
            let mut identity = self.identity.clone();
            identity.smb_binding = Some(crate::auth::SmbBinding {
                repository_id: key.repository.clone(),
                branch_id: key.branch.clone(),
                generation: key.generation,
            });
            Session {
                shared: self.shared.clone(),
                identity,
                account_generation: self.account_generation,
                branch: key.branch.clone(),
                handles: BTreeMap::new(),
                next_handle: 1,
                max_file_bytes: self.max_file_bytes,
                read_cache: None,
            }
        });
        Ok(key)
    }

    pub(super) fn dispatch(
        &mut self,
        request: Request,
    ) -> Result<(Reply, Option<OwnedFd>), DispatchError> {
        let request = request
            .parse_operation_id()
            .map_err(|error| failure(ApiError::invalid_source(error), None))?;
        let operation = match &request {
            Request::Namespace { operation_id, .. } => Some(operation_id.to_string()),
            _ => None,
        };
        let result = match request {
            Request::Open {
                path,
                writable,
                create,
                truncate,
                exclusive,
                write_through,
            } => self
                .open(path, writable, create, truncate, exclusive, write_through)
                .map_err(|error| failure(error, None)),
            Request::Stat { path } => self.stat(&path).map_err(|error| failure(error, None)),
            Request::Namespace {
                kind,
                path,
                destination,
                replace,
                operation_id,
            } => self.namespace(kind, path, destination, replace, operation_id),
            Request::Hello { .. } | Request::HelloDefault { .. } => Err(failure(
                ApiError::invalid("hello is allowed only once"),
                operation,
            )),
            request => self.handle(request),
        };
        // Empty sub-sessions own no descriptors or write locks. Do not grow a
        // per-connection cache with every visited repository or binding revision.
        self.sessions
            .retain(|_, session| !session.handles.is_empty());
        result
    }

    fn open(
        &mut self,
        path: String,
        writable: bool,
        create: bool,
        truncate: bool,
        exclusive: bool,
        write_through: bool,
    ) -> Result<(Reply, Option<OwnedFd>), ApiError> {
        let account = self.shared.account.clone();
        let binding = account.read()?;
        binding.require_generation(self.account_generation)?;
        if self.handles.len() >= MAX_HANDLES {
            return Err(ApiError::limit("too many SMB handles"));
        }
        let options = super::handles::OpenOptions::parse(
            writable,
            create,
            truncate,
            exclusive,
            write_through,
        )?;
        let location = self.locate(&path)?;
        let id = self.next_handle;
        self.next_handle = self
            .next_handle
            .checked_add(1)
            .ok_or_else(|| ApiError::limit("SMB handle identity exhausted"))?;
        let (mut opened, descriptor, route) = match location {
            Location::Root | Location::Disk(_) => {
                if writable || create || truncate {
                    return Err(directory_only());
                }
                self.open_directory(location, id)?
            }
            Location::Path { slot, path: within } => {
                let location = self.resolve_at(
                    &slot,
                    within,
                    if create {
                        SmbPathIntent::CreateFile
                    } else {
                        SmbPathIntent::Lookup
                    },
                )?;
                let root_entry_id = matches!(&location.relative_path, SmbRelativePath::Root)
                    .then(|| location.repository.repository_id.clone());
                let prefix = if location.repository.file_manager_root {
                    slot.directory_name()?.as_str().to_owned()
                } else {
                    format!(
                        "{}/{}",
                        slot.directory_name()?.as_str(),
                        location.repository.name
                    )
                };
                let key = self.session_key(location.repository)?;
                let session = self
                    .sessions
                    .get_mut(&key)
                    .expect("invariant: selected session was inserted");
                let (mut opened, descriptor) =
                    session.open_typed(location.relative_path, options)?;
                opened.path = full_path(&prefix, &opened.path);
                if let Some(entry_id) = root_entry_id {
                    opened.entry_id = entry_id;
                }
                let local = opened.handle;
                (
                    opened,
                    descriptor,
                    RoutedHandle::Repository {
                        session: key,
                        handle: local,
                    },
                )
            }
        };
        opened.handle = id;
        self.handles.insert(id, route);
        Ok((
            Reply::Json(json_codec::to_value(opened).expect("invariant: open metadata serializes")),
            descriptor,
        ))
    }

    fn stat(&mut self, path: &str) -> Result<(Reply, Option<OwnedFd>), ApiError> {
        let account = self.shared.account.clone();
        let binding = account.read()?;
        binding.require_generation(self.account_generation)?;
        let (entry_id, canonical_path, name) = match self.locate(path)? {
            Location::Root => ("smb-root".into(), String::new(), String::new()),
            Location::Disk(slot) => {
                let name = slot.directory_name()?.as_str().to_owned();
                (slot.disk_id, name.clone(), name)
            }
            Location::Path { slot, path: within } => {
                let location = self.resolve_at(&slot, within, SmbPathIntent::Lookup)?;
                let root_entry_id = matches!(&location.relative_path, SmbRelativePath::Root)
                    .then(|| location.repository.repository_id.clone());
                let prefix = if location.repository.file_manager_root {
                    slot.directory_name()?.as_str().to_owned()
                } else {
                    format!(
                        "{}/{}",
                        slot.directory_name()?.as_str(),
                        location.repository.name
                    )
                };
                let key = self.session_key(location.repository)?;
                let mut result = self.sessions[&key].stat_typed(&location.relative_path)?;
                result.path = full_path(&prefix, &result.path);
                if let Some(entry_id) = root_entry_id {
                    result.entry_id = entry_id;
                }
                if result.name.is_empty() {
                    result.name = prefix
                        .rsplit('/')
                        .next()
                        .expect("invariant: prefix contains a slot")
                        .into();
                }
                return Ok(json_reply(result));
            }
        };
        Ok(json_reply(StatResult {
            entry_id,
            kind: storage_contract::NamespaceEntryKind::Directory,
            path: canonical_path,
            name,
            size: 0,
            mtime: FileMtime::NotFile {},
        }))
    }

    fn namespace(
        &mut self,
        kind: NamespaceOperation,
        path: String,
        destination: Option<String>,
        replace: bool,
        operation_id: OperationId,
    ) -> Result<(Reply, Option<OwnedFd>), DispatchError> {
        (|| {
            let account = self.shared.account.clone();
            let binding = account.read()?;
            binding.require_generation(self.account_generation)?;
            if (kind == NamespaceOperation::Rename) != destination.is_some()
                || (replace && kind != NamespaceOperation::Rename)
            {
                return Err(ApiError::invalid(
                    "only rename accepts a destination or replacement",
                ));
            }
            if kind == NamespaceOperation::Mkdir {
                let Location::Path { path, .. } = self.locate(&path)? else {
                    return Err(directory_only());
                };
                name_policy::PortablePath::try_from(path).map_err(ApiError::invalid_source)?;
            }
            if kind == NamespaceOperation::Rename
                && let Some(reply) = self.resume_object_move(
                    &path,
                    destination
                        .as_deref()
                        .expect("invariant: rename destination was validated"),
                    replace,
                    operation_id,
                )?
            {
                return Ok((Reply::Json(reply), None));
            }
            let admission = super::namespace::NamespaceAdmission::acquire(
                &self.shared,
                &self.identity,
                operation_id,
            )?;
            let intent = if kind == NamespaceOperation::Mkdir {
                SmbPathIntent::CreateDirectory(operation_id)
            } else {
                SmbPathIntent::Lookup
            };
            let source = self.resolve(&path, intent)?;
            // Root namespaces cannot be renamed/deleted through the contents API.
            if let Some(path) = destination {
                let reply = self.move_object(source, &path, admission, replace)?;
                return Ok((Reply::Json(reply), None));
            }
            let SmbRelativePath::Entry(path) = source.relative_path else {
                return Err(ApiError::unsupported());
            };
            let key = self.session_key(source.repository)?;
            let input = super::namespace::NamespaceInput::new(kind, path, None, replace)?;
            let result = self
                .sessions
                .get_mut(&key)
                .expect("invariant: selected session was inserted")
                .namespace_admitted(input, admission)?;
            Ok((Reply::Json(result), None))
        })()
        .map_err(|mut error| {
            // Namespace requests never permit automatic replay, including
            // failures during root resolution or submission admission.
            error.retryable = false;
            failure(error, Some(operation_id.to_string()))
        })
    }

    fn handle(
        &mut self,
        mut request: Request<OperationId>,
    ) -> Result<(Reply, Option<OwnedFd>), DispatchError> {
        let handle = match &mut request {
            Request::AuthorizeRead { handle }
            | Request::Read { handle, .. }
            | Request::PrepareWrite { handle, .. }
            | Request::Dirty { handle }
            | Request::Flush { handle }
            | Request::Close { handle }
            | Request::Detach { handle }
            | Request::Abort { handle }
            | Request::ReadDirectory { handle, .. } => handle,
            _ => unreachable!("invariant: path and handshake requests dispatched separately"),
        };
        let id = *handle;
        let route = self
            .handles
            .get(&id)
            .ok_or_else(|| failure(missing(), None))?;
        if let RoutedHandle::Repository {
            session,
            handle: local,
        } = route
        {
            let key = session.clone();
            let local = *local;
            *handle = local;
            let session = self
                .sessions
                .get_mut(&key)
                .expect("invariant: bound handle owns its sub-session");
            let result = session.dispatch_admitted(request);
            if !session.handles.contains_key(&local) {
                self.handles.remove(&id);
            }
            return result;
        }
        self.virtual_handle(id, request)
            .map_err(|error| failure(error, None))
    }
}

fn full_path(prefix: &str, relative: &str) -> String {
    if relative.is_empty() {
        prefix.into()
    } else {
        format!("{prefix}/{relative}")
    }
}
