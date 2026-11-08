//! Shared initial credential candidate and no-overwrite installation. The caller
//! owns the empty-system proof and setup authorization, not this format writer.
use super::replacement::revision;
use super::*;

#[derive(Serialize, Deserialize)]
enum InitialFormat {
    #[serialization(rename = "system-management-initial-administrator")]
    Administrator,
}
/// Protected initialization material. Contains a verifier, not a plaintext
/// password, and must never be serialized into a public HTTP response or log.
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct PreparedInitialAdministrator {
    format: InitialFormat,
    pub(super) database: Database,
}
impl PreparedInitialAdministrator {
    /// Apply the same current username and password policy as normal setup.
    pub fn new(username: &str, password: &str) -> Result<Self, ApiError> {
        let username = primitives::normalize_administrator_username(username)
            .map_err(ApiError::invalid_source)?;
        let password_hash =
            primitives::hash_password(password).map_err(ApiError::authentication_operation)?;
        Ok(Self {
            format: InitialFormat::Administrator,
            database: Database {
                administrator: Administrator {
                    username,
                    password_hash,
                    generation: 1,
                },
                sessions: BTreeMap::new(),
            },
        })
    }
    pub fn credential_revision(&self) -> Result<CredentialRevision, ApiError> {
        self.validate()?;
        revision(&self.database)
    }
    pub fn verifies_repeated_password(&self, password: &str) -> Result<bool, ApiError> {
        self.validate()?;
        Ok(primitives::verify_password(
            password,
            &self.database.administrator.password_hash,
        ))
    }
    fn validate(&self) -> Result<(), ApiError> {
        self.database.validate()?;
        if self.database.administrator.generation != 1 || !self.database.sessions.is_empty() {
            return Err(ApiError::invalid(
                "initial administrator must have generation one and no sessions",
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum InitialManagementOutcome {
    Created,
    ExistingDurablyVerified,
}
impl AuthStore {
    /// Install the exact retained initial administrator in an already-admitted
    /// private directory. Existing different credentials or later sessions are
    /// never overwritten. A returned store retains the real auth.lock owner.
    ///
    /// # Errors
    /// Preserves lock, file safety, format, conflict and publication failures.
    /// Uncertain publication is not proof that account creation did not occur.
    pub fn create_initial_system_management(
        directory: PrivateDirectory,
        candidate: &PreparedInitialAdministrator,
    ) -> Result<(Self, InitialManagementOutcome), ApiError> {
        candidate.validate()?;
        directory
            .verify_identity()
            .map_err(ApiError::invalid_source)?;
        let auth_lock = Self::acquire_lock(&directory)?;
        let name = credential_name(true);
        let existing = match directory.read_bounded(&name, MAX_STORE_BYTES) {
            Ok(bytes) => {
                let value =
                    storage_contract::strict_json(&bytes).map_err(ApiError::invalid_source)?;
                let database = Database::from_snapshot(&value)?;
                if revision(&database)? != candidate.credential_revision()? {
                    return Err(ApiError::conflict(
                        storage_contract::ErrorCode::AuthAlreadyInitialized,
                        "existing credentials differ from the retained initial administrator or include later sessions",
                    ));
                }
                true
            }
            Err(platform_io::fs_safety::Error::Io(error))
                if error.kind() == std::io::ErrorKind::NotFound =>
            {
                false
            }
            Err(error) => return Err(ApiError::invalid_source(error)),
        };
        let store = Self {
            directory: Some(directory),
            name,
            db: Some(candidate.database.clone()),
            _lock: Some(auth_lock),
            local_authority: true,
            backup: None,
            blocked: Arc::new(AtomicBool::new(false)),
            committed: Arc::new(RwLock::new(None)),
        };
        let outcome = if existing {
            store.publish_committed();
            InitialManagementOutcome::ExistingDurablyVerified
        } else {
            store.persist(true)?;
            InitialManagementOutcome::Created
        };
        store.synchronize_current_credentials()?;
        Ok((store, outcome))
    }
}
#[cfg(test)]
#[path = "../../../../test/server/unit/auth/initial_tests.rs"]
mod tests;
