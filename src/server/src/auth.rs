//! The single administrator and session policy built on local credential primitives.
//! Credentials and token hashes are local protected control data, never public API data.
use crate::ApiError;
use authentication as primitives;
use data_serialization::{Deserialize, Serialize};
use platform_io::fs_safety::{AdvisoryLock, EntryName, PrivateDirectory, RelativePath};
use std::{
    collections::BTreeMap,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    sync::{
        Arc, RwLock, Weak,
        atomic::{AtomicBool, Ordering},
    },
    time::{SystemTime, UNIX_EPOCH},
};

pub(crate) const SESSION_TTL_SECONDS: u64 = 12 * 60 * 60;
const MAX_SESSIONS: usize = 64;
/// Successful writes must remain admissible to the same bounded restart reader.
const MAX_STORE_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Administrator {
    username: String,
    password_hash: String,
    #[serialization(with = "storage_contract::u64_string")]
    generation: u64,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Session {
    #[serialization(with = "storage_contract::u64_string")]
    generation: u64,
    #[serialization(with = "storage_contract::u64_string")]
    expires_at: u64,
}

/// A required singleton, never an account collection or a migration target.
#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Database {
    administrator: Administrator,
    sessions: BTreeMap<String, Session>,
}

impl Database {
    fn from_snapshot(snapshot: &json_codec::Value) -> Result<Self, ApiError> {
        // Serde also supports positional sequences for structs. Protected
        // credentials admit only the documented named-object representation.
        let object = snapshot
            .as_object()
            .ok_or_else(|| ApiError::invalid("credential state must be an object"))?;
        if !object
            .get("administrator")
            .is_some_and(json_codec::Value::is_object)
        {
            return Err(ApiError::invalid(
                "exactly one administrator object is required",
            ));
        }
        let sessions = object
            .get("sessions")
            .and_then(json_codec::Value::as_object)
            .ok_or_else(|| ApiError::invalid("session state must be an object"))?;
        if sessions.values().any(|session| !session.is_object()) {
            return Err(ApiError::invalid("session records must be objects"));
        }
        let database = Self::deserialize(snapshot).map_err(ApiError::invalid_source)?;
        database.validate()?;
        Ok(database)
    }

    /// Every authority reader uses the same policy, including engine snapshots.
    fn validate(&self) -> Result<(), ApiError> {
        if self.sessions.len() > MAX_SESSIONS {
            return Err(ApiError::invalid("invalid credential store session count"));
        }
        let administrator = &self.administrator;
        primitives::require_canonical_administrator_username(&administrator.username)
            .map_err(ApiError::invalid_source)?;
        primitives::require_current_password_hash(&administrator.password_hash)
            .map_err(ApiError::invalid_source)?;
        if administrator.generation == 0 {
            return Err(ApiError::invalid(
                "administrator generation must be positive",
            ));
        }
        if self.sessions.keys().any(|key| {
            key.len() != primitives::TOKEN_HASH_BYTES * 2
                || !key
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        }) {
            return Err(ApiError::invalid("invalid session token hash"));
        }
        Ok(())
    }

    fn identity(&self, principal: &str) -> Result<Identity, ApiError> {
        if self.administrator.username != principal {
            return Err(ApiError::unauthorized());
        }
        Ok(Identity {
            admission_session: None,
            smb_binding: None,
            principal: self.administrator.username.clone(),
        })
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct Identity {
    /// Internal session hash, never part of the wire identity.
    #[serialization(skip)]
    pub admission_session: Option<String>,
    /// Internal SMB entrance constraint. Admission refresh must preserve it;
    /// the engine validates it while holding the same lock as the operation.
    #[serialization(skip)]
    pub smb_binding: Option<SmbBinding>,
    pub principal: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SmbBinding {
    pub repository_id: String,
    pub branch_id: String,
    pub generation: u64,
}

/// Schema-validated admission projection. Password hashes are discarded;
/// session hashes and generations remain necessary to enforce revocation.
#[cfg(feature = "core")]
pub(crate) struct AdmissionSnapshot {
    principal: String,
    generation: u64,
    sessions: BTreeMap<String, Session>,
}

#[cfg(feature = "core")]
impl AdmissionSnapshot {
    pub(crate) fn from_snapshot(snapshot: &json_codec::Value) -> Result<Self, ApiError> {
        let database = Database::from_snapshot(snapshot)?;
        Ok(Self {
            principal: database.administrator.username,
            generation: database.administrator.generation,
            sessions: database.sessions,
        })
    }

    pub(crate) fn revalidate_identity(&self, caller: &Identity) -> Result<Identity, ApiError> {
        refresh_identity(&self.principal, self.generation, &self.sessions, caller)
    }
}

#[derive(Serialize, Deserialize)]
pub struct LoginResult {
    pub token: String,
    pub csrf_token: String,
    pub principal: String,
    #[serialization(with = "storage_contract::u64_string")]
    pub expires_at: u64,
}

pub type BackupHook = Arc<dyn Fn(json_codec::Value) -> Result<(), ApiError> + Send + Sync>;

pub struct AuthStore {
    path: PathBuf,
    // An unavailable control plane has no credential authority, not a dummy account.
    // Writer-owned candidate; never exposed through AuthReader before persistence.
    db: Option<Database>,
    _lock: Option<AdvisoryLock>,
    local_authority: bool,
    backup: Option<BackupHook>,
    blocked: Arc<AtomicBool>,
    committed: Arc<RwLock<Option<Arc<Database>>>>,
    #[cfg(test)]
    password_work_hook: Option<Arc<dyn Fn() + Send + Sync>>,
}

impl Drop for AuthStore {
    fn drop(&mut self) {
        // A concurrent reader may briefly own the channel Arc. Closing admission
        // explicitly prevents its weak handles extending authority past the OS lock.
        self.blocked.store(true, Ordering::SeqCst);
    }
}

/// A live view of committed authority, not a reusable admission proof. Each call
/// takes a fresh snapshot. Weak ownership cannot keep credentials usable after
/// the store (and its process-wide file lock) has been dropped.
#[derive(Clone)]
pub(crate) struct AuthReader {
    committed: Weak<RwLock<Option<Arc<Database>>>>,
    blocked: Weak<AtomicBool>,
}

impl AuthReader {
    /// Capture the same committed credentials for admission classification and hashing.
    pub(crate) fn login_challenge(&self, username: &str) -> Result<LoginChallenge, ApiError> {
        // HTTP rejects invalid candidates before retaining any admission state.
        let username = primitives::normalize_administrator_username(username)
            .map_err(|source| ApiError::unauthorized().with_source(source))?;
        Ok(LoginChallenge::new(
            &self.database()?.administrator,
            Some(username),
        ))
    }

    /// Map a trusted protocol handshake to the current sole administrator.
    /// This snapshot is not authentication or a reusable authorization proof.
    pub(crate) fn sole_identity(&self) -> Result<Identity, ApiError> {
        let database = self.database()?;
        database.identity(&database.administrator.username)
    }

    fn database(&self) -> Result<Arc<Database>, ApiError> {
        let committed = self
            .committed
            .upgrade()
            .ok_or_else(ApiError::unauthorized)?;
        let blocked = self.blocked.upgrade().ok_or_else(ApiError::unauthorized)?;
        let database = committed.read().expect("service state lock poisoned");
        require_available(&blocked)?;
        database
            .as_ref()
            .map(Arc::clone)
            .ok_or_else(AuthStore::missing_authority)
    }

    pub(crate) fn authenticate(&self, token: &str) -> Result<Identity, ApiError> {
        authenticate_database(self.database()?.as_ref(), token)
    }

    pub(crate) fn revalidate_identity(&self, caller: &Identity) -> Result<Identity, ApiError> {
        let database = self.database()?;
        refresh_identity(
            &database.administrator.username,
            database.administrator.generation,
            &database.sessions,
            caller,
        )
    }
}

fn authenticate_database(database: &Database, token: &str) -> Result<Identity, ApiError> {
    if !primitives::is_token_shape(token) {
        return Err(ApiError::unauthorized());
    }
    let key = primitives::token_hash_hex(token);
    let current_time = now()?;
    database
        .sessions
        .get(&key)
        .filter(|session| {
            session.expires_at > current_time
                && session.generation == database.administrator.generation
        })
        .ok_or_else(ApiError::unauthorized)?;
    let mut identity = database.identity(&database.administrator.username)?;
    identity.admission_session = Some(key);
    Ok(identity)
}

/// Immutable password challenge. Expensive verification needs no store lock;
/// successful proofs remain bound to the exact administrator generation and hash.
pub(crate) struct LoginChallenge {
    username: Option<String>,
    password_hash: String,
    generation: Option<u64>,
}
pub(crate) struct VerifiedLogin {
    username: String,
    password_hash: String,
    generation: u64,
}
impl LoginChallenge {
    /// The caller already applied the canonical username policy at its boundary.
    fn new(administrator: &Administrator, username: Option<String>) -> Self {
        Self {
            // Unknown names perform exactly the same password verification,
            // but cannot receive a proof even if the supplied password matches.
            generation: (username.as_deref() == Some(administrator.username.as_str()))
                .then_some(administrator.generation),
            username,
            password_hash: administrator.password_hash.clone(),
        }
    }

    pub(crate) fn is_known_account(&self) -> bool {
        self.generation.is_some()
    }

    pub(crate) fn canonical_username(&self) -> Option<&str> {
        self.username.as_deref()
    }

    pub(crate) fn verify(self, password: &str) -> Result<VerifiedLogin, ApiError> {
        let valid = primitives::verify_password(password, &self.password_hash);
        let generation = self
            .generation
            .filter(|_| valid)
            .ok_or_else(ApiError::unauthorized)?;
        Ok(VerifiedLogin {
            username: self
                .username
                .expect("invariant: a verified account has a canonical username"),
            password_hash: self.password_hash,
            generation,
        })
    }
}

/// A session-authorized credential baseline; preparing its replacement requires
/// no store lock and does not authorize publication after logout or rotation.
pub(crate) struct PasswordChangeChallenge {
    identity: Identity,
    previous: Administrator,
    #[cfg(test)]
    password_work_hook: Option<Arc<dyn Fn() + Send + Sync>>,
}

/// Both Argon2 operations have succeeded, but the live authority and original
/// session must still be revalidated before the replacement can be published.
pub(crate) struct PreparedPasswordChange {
    identity: Identity,
    previous: Administrator,
    password_hash: String,
    generation: u64,
}

impl PasswordChangeChallenge {
    pub(crate) fn prepare(
        self,
        current_password: &str,
        new_password: &str,
    ) -> Result<PreparedPasswordChange, ApiError> {
        #[cfg(test)]
        if let Some(hook) = &self.password_work_hook {
            hook();
        }
        if !primitives::verify_password(current_password, &self.previous.password_hash) {
            return Err(ApiError::new(
                ::http::StatusCode::FORBIDDEN,
                storage_contract::ErrorCode::CurrentPasswordInvalid,
                "current password is incorrect",
            ));
        }
        let generation = self.previous.generation.checked_add(1).ok_or_else(|| {
            ApiError::conflict(
                storage_contract::ErrorCode::AuthStateInvalid,
                "administrator generation has been exhausted",
            )
        })?;
        #[cfg(test)]
        if let Some(hook) = &self.password_work_hook {
            hook();
        }
        let password_hash =
            primitives::hash_password(new_password).map_err(ApiError::invalid_source)?;
        Ok(PreparedPasswordChange {
            identity: self.identity,
            previous: self.previous,
            password_hash,
            generation,
        })
    }
}

/// Shared authority check for live stores and recovered engine snapshots.
fn refresh_identity(
    principal: &str,
    generation: u64,
    sessions: &BTreeMap<String, Session>,
    caller: &Identity,
) -> Result<Identity, ApiError> {
    if caller.principal != principal {
        return Err(ApiError::unauthorized());
    }
    if let Some(key) = &caller.admission_session {
        let current_time = now()?;
        sessions
            .get(key)
            .filter(|session| session.expires_at > current_time && session.generation == generation)
            .ok_or_else(ApiError::unauthorized)?;
    }
    Ok(caller.clone())
}

fn now() -> Result<u64, ApiError> {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .map_err(|source| {
            ApiError::new(
                ::http::StatusCode::INTERNAL_SERVER_ERROR,
                storage_contract::ErrorCode::InternalError,
                "system clock precedes the Unix epoch",
            )
            .with_source(source)
        })
}

// Relative deployment paths remain supported, but resolving them never follows
// symlinks or changes permissions on an existing directory.
fn absolute_root(root: &Path) -> Result<PathBuf, ApiError> {
    std::path::absolute(root).map_err(ApiError::io)
}

fn private_directory(root: &Path) -> Result<PrivateDirectory, ApiError> {
    let root = absolute_root(root)?;
    #[cfg(unix)]
    let directory = PrivateDirectory::open_trusted(root);
    #[cfg(not(unix))]
    let directory = PrivateDirectory::open_existing(root);
    directory.map_err(ApiError::invalid_source)
}

fn read_credentials(path: &Path) -> Result<Vec<u8>, ApiError> {
    let directory = private_directory(
        path.parent()
            .ok_or_else(|| ApiError::invalid("missing credential parent"))?,
    )?;
    let name = EntryName::new(
        path.file_name()
            .ok_or_else(|| ApiError::invalid("missing credential name"))?,
    )
    .map_err(ApiError::invalid_source)?;
    directory
        .read_bounded(&name, MAX_STORE_BYTES)
        .map_err(ApiError::invalid_source)
}

fn require_available(blocked: &AtomicBool) -> Result<(), ApiError> {
    if blocked.load(Ordering::SeqCst) {
        Err(ApiError::new(
            ::http::StatusCode::SERVICE_UNAVAILABLE,
            storage_contract::ErrorCode::RecoveryRequired,
            "authentication publication must be recovered before serving requests",
        ))
    } else {
        Ok(())
    }
}

impl AuthStore {
    pub(crate) fn reader(&self) -> AuthReader {
        AuthReader {
            committed: Arc::downgrade(&self.committed),
            blocked: Arc::downgrade(&self.blocked),
        }
    }

    fn publish_committed(&self) {
        let database = self.db.as_ref().map(|db| Arc::new(db.clone()));
        *self.committed.write().expect("service state lock poisoned") = database;
    }

    /// Explicit setup/recovery replaces authority without orphaning live readers.
    #[cfg(feature = "core")]
    pub(crate) fn replace_authority(&mut self, mut replacement: Self) {
        let database = replacement.db.as_ref().map(|db| Arc::new(db.clone()));
        let mut committed = self.committed.write().expect("service state lock poisoned");
        self.blocked
            .store(replacement.blocked.load(Ordering::SeqCst), Ordering::SeqCst);
        *committed = database;
        drop(committed);
        // The replaced store must drop only its retired channel, not block the
        // live channel now owned by the replacement.
        std::mem::swap(&mut self.committed, &mut replacement.committed);
        std::mem::swap(&mut self.blocked, &mut replacement.blocked);
        *self = replacement;
    }

    fn require_available(&self) -> Result<(), ApiError> {
        require_available(&self.blocked)
    }
    fn database(&self) -> Result<&Database, ApiError> {
        self.require_available()?;
        self.db.as_ref().ok_or_else(Self::missing_authority)
    }
    fn database_mut(&mut self) -> Result<&mut Database, ApiError> {
        self.require_available()?;
        self.db.as_mut().ok_or_else(Self::missing_authority)
    }
    fn missing_authority() -> ApiError {
        ApiError::new(
            ::http::StatusCode::SERVICE_UNAVAILABLE,
            storage_contract::ErrorCode::AuthRecoveryRequired,
            "administrator credentials are unavailable",
        )
    }
    pub fn snapshot(&self) -> Result<json_codec::Value, ApiError> {
        // A blocked publication may still be inspected for recovery, but an
        // uninitialized control plane cannot invent an administrator snapshot.
        Ok(
            json_codec::to_value(self.db.as_ref().ok_or_else(Self::missing_authority)?)
                .expect("invariant: credential snapshot serialization succeeds"),
        )
    }
    pub fn attach_backup(&mut self, backup: BackupHook) -> Result<(), ApiError> {
        backup(self.snapshot()?)?;
        self.backup = Some(backup);
        Ok(())
    }
    /// Resolve the sole configured principal after a trusted protocol handshake.
    /// This does not authenticate an HTTP request or an anonymous SMB peer.
    pub fn sole_identity(&self) -> Result<Identity, ApiError> {
        let database = self.database()?;
        database.identity(&database.administrator.username)
    }
    pub fn identity_for_principal(&self, username: &str) -> Result<Identity, ApiError> {
        self.database()?.identity(username)
    }
    /// Revalidate queued HTTP sessions. Protocol mappings carry no session and
    /// must still name the one configured administrator.
    pub fn revalidate_identity(&self, caller: &Identity) -> Result<Identity, ApiError> {
        let database = self.database()?;
        refresh_identity(
            &database.administrator.username,
            database.administrator.generation,
            &database.sessions,
            caller,
        )
    }
    fn acquire_lock(root: &Path) -> Result<AdvisoryLock, ApiError> {
        let directory = private_directory(root)?;
        AdvisoryLock::acquire(&directory, &RelativePath::new("auth.lock").expect("invariant: static credential lock filename is valid"))
            .map_err(|error| match error {
                source @ platform_io::fs_safety::Error::AlreadyLocked(_) => ApiError::conflict(
                    storage_contract::ErrorCode::AuthStoreBusy,
                    "another process owns the credential store; use its authenticated administration API",
                ).with_source(source),
                other => ApiError::invalid_source(other),
            })
    }
    /// Create only through the explicit administrative setup command.
    pub fn initialize(root: &Path, username: &str, password: &str) -> Result<Self, ApiError> {
        Self::initialize_at(root, username, password, false)
    }
    #[cfg(feature = "core")]
    pub(crate) fn initialize_management(
        root: &Path,
        username: &str,
        password: &str,
    ) -> Result<Self, ApiError> {
        Self::initialize_at(root, username, password, true)
    }
    fn initialize_at(
        root: &Path,
        username: &str,
        password: &str,
        management: bool,
    ) -> Result<Self, ApiError> {
        let root = absolute_root(root)?;
        let root = root.as_path();
        let path = root.join(if management {
            "management-auth.json"
        } else {
            "auth.json"
        });
        if path.symlink_metadata().is_ok() {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::AuthAlreadyInitialized,
                "credentials already exist",
            ));
        }
        #[cfg(unix)]
        PrivateDirectory::create_trusted(absolute_root(root)?).map_err(ApiError::invalid_source)?;
        #[cfg(not(unix))]
        {
            fs::create_dir_all(root).map_err(ApiError::io)?;
            PrivateDirectory::open_existing(absolute_root(root)?)
                .map_err(ApiError::invalid_source)?;
        }
        let auth_lock = Self::acquire_lock(root)?;
        let username = primitives::normalize_administrator_username(username)
            .map_err(ApiError::invalid_source)?;
        let hash = primitives::hash_password(password).map_err(ApiError::invalid_source)?;
        let db = Database {
            administrator: Administrator {
                username,
                password_hash: hash,
                generation: 1,
            },
            sessions: BTreeMap::new(),
        };
        let store = Self {
            path,
            db: Some(db),
            _lock: Some(auth_lock),
            local_authority: management,
            backup: None,
            blocked: Arc::new(AtomicBool::new(false)),
            committed: Arc::new(RwLock::new(None)),
            #[cfg(test)]
            password_work_hook: None,
        };
        store.persist(true)?;
        Ok(store)
    }

    pub fn open(root: &Path) -> Result<Self, ApiError> {
        Self::open_with_snapshot(root, None)
    }
    /// Restore only from the engine's already recovered authoritative control state.
    /// Missing local credentials never cause implicit account creation.
    pub fn open_with_snapshot(
        root: &Path,
        authoritative: Option<json_codec::Value>,
    ) -> Result<Self, ApiError> {
        Self::open_at(root, authoritative, false)
    }
    #[cfg(feature = "core")]
    pub(crate) fn open_management(root: &Path) -> Result<Self, ApiError> {
        Self::open_at(root, None, true)
    }
    #[cfg(feature = "core")]
    pub(crate) fn restore_management(
        root: &Path,
        snapshot: json_codec::Value,
    ) -> Result<Self, ApiError> {
        Self::open_at(root, Some(snapshot), true)
    }
    fn open_at(
        root: &Path,
        authoritative: Option<json_codec::Value>,
        management: bool,
    ) -> Result<Self, ApiError> {
        let root = absolute_root(root)?;
        let root = root.as_path();
        let auth_lock = Self::acquire_lock(root)?;
        let path = root.join(if management {
            "management-auth.json"
        } else {
            "auth.json"
        });
        let local = match fs::symlink_metadata(&path) {
            Ok(metadata) => {
                if !metadata.is_file() || metadata.file_type().is_symlink() {
                    return Err(ApiError::invalid("invalid credential store"));
                }
                #[cfg(unix)]
                {
                    use std::os::unix::fs::MetadataExt;
                    if metadata.mode() & 0o077 != 0 || metadata.nlink() != 1 {
                        return Err(ApiError::invalid(
                            "credential store must have mode 0600 and a single link",
                        ));
                    }
                }
                if authoritative.is_some() {
                    // Safety checks still apply, but a recovered authoritative
                    // snapshot replaces local contents without reading them.
                    // A stale or oversized cache cannot veto valid recovery.
                    None
                } else {
                    if metadata.len() > MAX_STORE_BYTES as u64 {
                        return Err(ApiError::invalid("credential store exceeds limit"));
                    }
                    Some(read_credentials(&path)?)
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
            Err(error) => return Err(ApiError::io(error)),
        };
        let restore = authoritative.is_some();
        let snapshot = match authoritative {
            Some(snapshot) => snapshot,
            None => storage_contract::strict_json(&local.ok_or_else(|| {
                ApiError::new(
                    ::http::StatusCode::SERVICE_UNAVAILABLE,
                    storage_contract::ErrorCode::AuthRecoveryRequired,
                    "credential store is missing and no authoritative backup is available",
                )
            })?)
            .map_err(ApiError::invalid_source)?,
        };
        let db = Database::from_snapshot(&snapshot)?;
        let store = Self {
            path,
            db: Some(db),
            _lock: Some(auth_lock),
            local_authority: management,
            backup: None,
            blocked: Arc::new(AtomicBool::new(false)),
            committed: Arc::new(RwLock::new(None)),
            #[cfg(test)]
            password_work_hook: None,
        };
        if restore {
            store.persist(false)?;
        } else {
            store.publish_committed();
        }
        Ok(store)
    }

    fn persist(&self, initial: bool) -> Result<(), ApiError> {
        // Reject an oversized candidate before creating even a temporary file
        // or publishing its protected backup. This is deterministic admission,
        // not an uncertain I/O outcome that should block existing credentials.
        let database = self.db.as_ref().ok_or_else(Self::missing_authority)?;
        let bytes =
            json_codec::to_vec(database).expect("invariant: internal JSON serialization succeeds");
        if bytes.len() > MAX_STORE_BYTES {
            return Err(ApiError::limit(
                "credential store exceeds serialized byte limit",
            ));
        }
        let temporary = self.path.with_extension(format!(
            "tmp-{}",
            primitives::random_token().map_err(ApiError::invalid_source)?
        ));
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary).map_err(ApiError::io)?;
        let mut local_published = false;
        let result = (|| {
            file.write_all(&bytes).map_err(ApiError::io)?;
            file.sync_all().map_err(ApiError::io)?;
            if !self.local_authority
                && let Some(backup) = &self.backup
                && let Err(error) = backup(self.snapshot()?)
            {
                // Admission rejected before any decision or auth publication.
                if error.code != storage_contract::ErrorCode::CommitPreparationBusy {
                    self.blocked.store(true, Ordering::SeqCst);
                }
                return Err(error);
            }
            if initial {
                fs::hard_link(&temporary, &self.path).map_err(|source| {
                    if source.kind() == std::io::ErrorKind::AlreadyExists {
                        ApiError::conflict(
                            storage_contract::ErrorCode::AuthAlreadyInitialized,
                            "credentials already exist",
                        )
                        .with_source(source)
                    } else {
                        ApiError::io(source)
                    }
                })?;
                local_published = true;
                fs::remove_file(&temporary).map_err(ApiError::io)?;
            } else {
                fs::rename(&temporary, &self.path).map_err(ApiError::io)?;
                local_published = true;
            }
            fs::File::open(self.path.parent().unwrap())
                .and_then(|dir| dir.sync_all())
                .map_err(ApiError::io)?;
            Ok(())
        })();
        if result.is_err() {
            if local_published
                || self.local_authority
                || self.backup.is_some()
                    && result.as_ref().err().is_none_or(|error| {
                        error.code != storage_contract::ErrorCode::CommitPreparationBusy
                    })
            {
                self.blocked.store(true, Ordering::SeqCst);
            }
            match fs::remove_file(temporary) {
                Ok(()) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => tracing::warn!(error = ?error, "credential staging cleanup failed"),
            }
        }
        if result.is_ok() {
            self.publish_committed();
        }
        if result.is_ok() && self.local_authority {
            // The durable system store is primary. Replica failure must never
            // roll back a password change or resurrect revoked sessions.
            if let Some(backup) = &self.backup
                && let Err(error) = backup(self.snapshot()?)
            {
                tracing::warn!(error = ?error, "credentials committed; disaster-recovery replica unavailable");
            }
        }
        result
    }

    pub fn is_available(&self) -> bool {
        self.db.is_some() && !self.blocked.load(Ordering::SeqCst)
    }
    #[cfg(feature = "core")]
    pub(crate) fn unavailable(root: &Path) -> Self {
        Self {
            path: root.join("management-auth.json"),
            db: None,
            _lock: None,
            local_authority: true,
            backup: None,
            blocked: Arc::new(AtomicBool::new(true)),
            committed: Arc::new(RwLock::new(None)),
            #[cfg(test)]
            password_work_hook: None,
        }
    }
    /// The callback maintains a disaster-recovery copy, not admission authority.
    #[cfg(feature = "core")]
    pub(crate) fn attach_replica(&mut self, replica: BackupHook) {
        self.backup = Some(replica);
    }

    pub fn login(&mut self, username: &str, password: &str) -> Result<LoginResult, ApiError> {
        let verified = self.login_challenge(username)?.verify(password)?;
        self.finish_login(verified)
    }

    pub(crate) fn login_challenge(&self, username: &str) -> Result<LoginChallenge, ApiError> {
        let administrator = &self.database()?.administrator;
        let username = match primitives::normalize_administrator_username(username) {
            Ok(username) => Some(username),
            Err(primitives::Error::InvalidAdministratorUsername) => None,
            Err(error) => return Err(ApiError::invalid_source(error)),
        };
        Ok(LoginChallenge::new(administrator, username))
    }

    pub(crate) fn finish_login(
        &mut self,
        verified: VerifiedLogin,
    ) -> Result<LoginResult, ApiError> {
        // Password changes and uncertain publication may race verification.
        // Recheck the exact credential proof before publishing any session.
        let administrator = &self.database()?.administrator;
        if administrator.username != verified.username
            || administrator.generation != verified.generation
            || administrator.password_hash != verified.password_hash
        {
            return Err(ApiError::unauthorized());
        }
        let token = primitives::random_token().map_err(ApiError::invalid_source)?;
        let csrf_token = primitives::derive_csrf_token(&token).map_err(ApiError::invalid_source)?;
        let expires_at = now()? + SESSION_TTL_SECONDS;
        let result = LoginResult {
            token: token.clone(),
            csrf_token,
            principal: administrator.username.clone(),
            expires_at,
        };
        let session = Session {
            generation: administrator.generation,
            expires_at,
        };
        let removed = self.prune_invalid_sessions()?;
        if self.database()?.sessions.len() >= MAX_SESSIONS {
            self.database_mut()?.sessions.extend(removed);
            return Err(ApiError::limit("session limit reached"));
        }
        let key = primitives::token_hash_hex(&token);
        self.database_mut()?.sessions.insert(key.clone(), session);
        if let Err(error) = self.persist(false) {
            // Publication may have blocked admission. Restore memory without
            // going through admission, so the exact pre-write state survives.
            let database = self.db.as_mut().expect("initialized administrator");
            database.sessions.remove(&key);
            database.sessions.extend(removed);
            return Err(error);
        }
        Ok(result)
    }

    /// Return removed records so a failed publication can restore exact state.
    fn prune_invalid_sessions(&mut self) -> Result<BTreeMap<String, Session>, ApiError> {
        let current_time = now()?;
        let database = self.database_mut()?;
        let generation = database.administrator.generation;
        let mut removed = BTreeMap::new();
        database.sessions.retain(|key, session| {
            let valid = session.expires_at > current_time && session.generation == generation;
            if !valid {
                removed.insert(key.clone(), session.clone());
            }
            valid
        });
        Ok(removed)
    }

    pub fn authenticate(&self, token: &str) -> Result<Identity, ApiError> {
        authenticate_database(self.database()?, token)
    }

    pub fn logout(&mut self, token: &str) -> Result<(), ApiError> {
        let key = primitives::token_hash_hex(token);
        let previous = self.database_mut()?.sessions.remove(&key);
        if let Err(error) = self.persist(false) {
            if let Some(session) = previous {
                self.db
                    .as_mut()
                    .expect("initialized administrator")
                    .sessions
                    .insert(key, session);
            }
            return Err(error);
        }
        Ok(())
    }

    /// Rotate the sole administrator's password and revoke every session.
    /// Shared HTTP callers use challenge/prepare/finish to hash outside the lock.
    pub fn change_password(
        &mut self,
        token: &str,
        current_password: &str,
        new_password: &str,
    ) -> Result<(), ApiError> {
        let prepared = self
            .password_change_challenge(token)?
            .prepare(current_password, new_password)?;
        self.finish_password_change(prepared)
    }

    pub(crate) fn password_change_challenge(
        &self,
        token: &str,
    ) -> Result<PasswordChangeChallenge, ApiError> {
        // The caller cannot select an identity, role, scope or generation.
        Ok(PasswordChangeChallenge {
            identity: self.authenticate(token)?,
            previous: self.database()?.administrator.clone(),
            #[cfg(test)]
            password_work_hook: self.password_work_hook.clone(),
        })
    }

    pub(crate) fn finish_password_change(
        &mut self,
        prepared: PreparedPasswordChange,
    ) -> Result<(), ApiError> {
        // Logout, session expiry, another rotation or uncertain publication may
        // have raced either Argon2 operation. Recheck before changing any state.
        self.revalidate_identity(&prepared.identity)?;
        if self.database()?.administrator != prepared.previous {
            return Err(ApiError::unauthorized());
        }
        let database = self.database_mut()?;
        database.administrator.password_hash = prepared.password_hash;
        database.administrator.generation = prepared.generation;
        // Include sessions created while hashing, not just the original snapshot.
        let sessions = std::mem::take(&mut database.sessions);
        if let Err(error) = self.persist(false) {
            let database = self.db.as_mut().expect("initialized administrator");
            database.administrator = prepared.previous;
            database.sessions = sessions;
            return Err(error);
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../test/server/unit/auth/budget_tests.rs"]
mod budget_tests;

#[cfg(test)]
#[path = "../../../test/server/unit/auth/tests.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../test/server/unit/auth/capacity_tests.rs"]
mod capacity_tests;

#[cfg(test)]
#[path = "../../../test/server/unit/auth/password_change_tests.rs"]
mod password_change_tests;

#[cfg(test)]
#[path = "../../../test/server/unit/auth/snapshot_tests.rs"]
mod snapshot_tests;
