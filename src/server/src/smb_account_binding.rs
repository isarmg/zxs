//! Durable selection among operator-provisioned Samba accounts. This module has
//! no passdb, Unix-account, smb.conf, command execution or elevation authority.
use crate::{ApiError, SmbConfig};
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::{
    AdvisoryLock, AtomicCommitError, AtomicFile, EntryName, PrivateDirectory, RelativePath,
};
use std::{
    collections::BTreeMap,
    io,
    os::unix::net::UnixStream,
    sync::{Arc, Mutex, RwLock, RwLockReadGuard, Weak},
};

const MAX_CANDIDATES: usize = 16;
const MAX_RECORD_BYTES: usize = 4096;
type ConnectionMap = Mutex<BTreeMap<u64, UnixStream>>;
type Connections = Arc<ConnectionMap>;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct Selection {
    version: u8,
    pub(crate) username: String,
    #[serialization(with = "storage_contract::u64_string")]
    pub(crate) generation: u64,
}

pub(crate) struct Authority {
    candidates: Vec<String>,
    state: RwLock<Store>,
    // Presentation only: this lock never spans SMB I/O, authorization, disk
    // publication or socket shutdown. A snapshot is not an admission proof.
    committed: Mutex<Option<Arc<Selection>>>,
    connections: Mutex<Option<Weak<ConnectionMap>>>,
}
pub(crate) struct Store {
    selection: Arc<Selection>,
    directory: PrivateDirectory,
    _lock: AdvisoryLock,
    blocked: bool,
}
impl Store {
    pub(crate) fn selection(&self) -> &Selection {
        &self.selection
    }
    pub(crate) fn require_generation(&self, generation: u64) -> Result<(), ApiError> {
        if self.selection.generation != generation {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::SmbRebindRequired,
                "SMB account binding changed; reconnect or refresh before retrying",
            ));
        }
        Ok(())
    }
}

fn unavailable(message: &'static str) -> ApiError {
    let mut error = ApiError::new(
        http::StatusCode::SERVICE_UNAVAILABLE,
        storage_contract::ErrorCode::SmbUnavailable,
        message,
    );
    error.retryable = false;
    error
}
fn name() -> EntryName {
    EntryName::new("selection.json").expect("invariant: static entry name")
}
fn publication(source: platform_io::fs_safety::Error) -> ApiError {
    unavailable("SMB account selection could not be durably published; refresh after recovery")
        .with_source(source)
}

pub(crate) fn candidates(config: &SmbConfig) -> Result<Vec<String>, ApiError> {
    let mut candidates = vec![config.username.clone()];
    candidates.extend(config.additional_users.iter().cloned());
    if candidates.len() > MAX_CANDIDATES {
        return Err(ApiError::invalid(
            "at most 16 provisioned SMB accounts are allowed",
        ));
    }
    for username in &candidates {
        smb_protocol::protocol::validate_username(username).map_err(ApiError::invalid_source)?;
        if username.eq_ignore_ascii_case("root") {
            return Err(ApiError::invalid("root cannot be a selectable SMB account"));
        }
    }
    let mut unique = std::collections::BTreeSet::new();
    if candidates
        .iter()
        .any(|name| !unique.insert(name.to_ascii_lowercase()))
    {
        return Err(ApiError::invalid(
            "SMB account candidates must be distinct ignoring ASCII case",
        ));
    }
    Ok(candidates)
}
impl Authority {
    pub(crate) fn open(config: &SmbConfig) -> Result<Arc<Self>, ApiError> {
        let candidates = candidates(config)?;
        let path = config
            .spool_dir
            .parent()
            .ok_or_else(|| ApiError::invalid("SMB spool requires a parent"))?
            .join("smb-account");
        let path = std::path::absolute(path).map_err(ApiError::io)?;
        // An existing authority directory with missing state never resets to
        // --smb-user. Interrupted first setup also requires explicit recovery.
        let new_directory = match std::fs::symlink_metadata(&path) {
            Ok(_) => false,
            Err(error) if error.kind() == io::ErrorKind::NotFound => true,
            Err(error) => return Err(ApiError::io(error)),
        };
        let directory = PrivateDirectory::create_trusted(&path).map_err(publication)?;
        let held_lock = AdvisoryLock::acquire(
            &directory,
            &RelativePath::new("selection.lock").expect("invariant: static lock name"),
        )
        .map_err(publication)?;
        let selection = match directory.read_bounded(&name(), MAX_RECORD_BYTES) {
            Ok(bytes) => storage_contract::decode_strict::<Selection>(&bytes)
                .map_err(ApiError::invalid_source)?,
            Err(platform_io::fs_safety::Error::Io(error))
                if error.kind() == io::ErrorKind::NotFound && new_directory =>
            {
                let selection = Selection {
                    version: 1,
                    username: config.username.clone(),
                    generation: 1,
                };
                let bytes =
                    json_codec::to_vec(&selection).expect("invariant: selection serialization");
                AtomicFile::create(&directory, &name(), &bytes).map_err(publication)?;
                selection
            }
            Err(error) => {
                return Err(unavailable(
                    "SMB account selection is missing or unreadable; restore it without resetting to the startup account",
                ).with_source(error));
            }
        };
        if selection.version != 1
            || selection.generation == 0
            || !candidates.contains(&selection.username)
        {
            return Err(ApiError::invalid(
                "persisted SMB selection must be a current provisioned candidate with a positive generation",
            ));
        }
        directory.verify_identity().map_err(publication)?;
        let selection = Arc::new(selection);
        Ok(Arc::new(Self {
            candidates,
            committed: Mutex::new(Some(Arc::clone(&selection))),
            state: RwLock::new(Store {
                selection,
                directory,
                _lock: held_lock,
                blocked: false,
            }),
            connections: Mutex::new(None),
        }))
    }
    pub(crate) fn candidates(&self) -> &[String] {
        &self.candidates
    }
    /// Return the last committed presentation without joining the operation
    /// gate. Pending switches retain the old snapshot; uncertain publication
    /// removes it. Authorization must continue using read()/require_generation().
    pub(crate) fn committed_selection(&self) -> Result<Arc<Selection>, ApiError> {
        let selection = crate::lock(&self.committed).clone();
        selection.ok_or_else(|| {
            unavailable(
                "SMB account publication is uncertain; restart after verifying durable state",
            )
        })
    }

    pub(crate) fn read(&self) -> Result<RwLockReadGuard<'_, Store>, ApiError> {
        let state = self.state.read().expect("SMB binding lock poisoned");
        if state.blocked {
            return Err(unavailable(
                "SMB account publication is uncertain; restart after verifying durable state",
            ));
        }
        Ok(state)
    }
    pub(crate) fn attach_connections(&self, connections: &Connections) {
        *crate::lock(&self.connections) = Some(Arc::downgrade(connections));
    }
    fn disconnect(&self) {
        if let Some(connections) = crate::lock(&self.connections)
            .as_ref()
            .and_then(Weak::upgrade)
        {
            for stream in crate::lock(&connections).values() {
                if let Err(error) = stream.shutdown(std::net::Shutdown::Both)
                    && error.kind() != io::ErrorKind::NotConnected
                {
                    tracing::warn!(error = ?error, "revoked SMB connection shutdown failed");
                }
            }
        }
    }
    pub(crate) fn select<T>(
        &self,
        expected: u64,
        username: &str,
        authorize: impl FnOnce() -> Result<T, ApiError>,
    ) -> Result<Selection, ApiError> {
        if !self
            .candidates
            .iter()
            .any(|candidate| candidate == username)
        {
            return Err(ApiError::forbidden());
        }
        // Read guards span complete SMB requests and password mutations. This
        // lock drains admitted operations before the durable account switch.
        let mut state = self.state.write().expect("SMB binding lock poisoned");
        if state.blocked {
            return Err(unavailable("SMB account selection requires recovery"));
        }
        state.require_generation(expected)?;
        let _authorization = authorize()?;
        if state.selection.username == username {
            return Ok(state.selection.as_ref().clone());
        }
        let selection = Selection {
            version: 1,
            username: username.to_owned(),
            generation: state
                .selection
                .generation
                .checked_add(1)
                .ok_or_else(|| unavailable("SMB account generation exhausted"))?,
        };
        state.directory.verify_identity().map_err(publication)?;
        let bytes = json_codec::to_vec(&selection).expect("invariant: selection serialization");
        let prepared = AtomicFile::stage(&state.directory, &name())
            .map_err(publication)?
            .prepare(&bytes)
            .map_err(publication)?;
        state.directory.verify_identity().map_err(publication)?;
        match prepared.replace() {
            Ok(_) => {
                state.selection = Arc::new(selection);
                *crate::lock(&self.committed) = Some(Arc::clone(&state.selection));
                self.disconnect();
                Ok(state.selection.as_ref().clone())
            }
            Err(error) => {
                if matches!(
                    error,
                    AtomicCommitError::Unknown(_) | AtomicCommitError::Published(_)
                ) {
                    state.blocked = true;
                    *crate::lock(&self.committed) = None;
                    self.disconnect();
                }
                Err(publication(error.into_source()))
            }
        }
    }
}

#[cfg(test)]
#[path = "../../../test/server/unit/smb_account_binding.rs"]
mod tests;
