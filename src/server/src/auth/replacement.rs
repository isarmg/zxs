//! Typed replacement of the one system-management credential authority. The
//! caller must already own its protected writer and authorize the operation;
//! these storage primitives are not an HTTP authentication or root policy.
use super::*;
use std::io::Read;

/// Public correlation data, not a password hash, session, or bearer capability.
#[derive(Clone, Eq, PartialEq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct CredentialRevision {
    principal: String,
    #[serialization(with = "storage_contract::u64_string")]
    generation: u64,
    fingerprint: String,
}
impl CredentialRevision {
    pub fn principal(&self) -> &str {
        &self.principal
    }
    pub fn generation(&self) -> u64 {
        self.generation
    }
}
#[derive(Serialize, Deserialize)]
enum ReplacementFormat {
    #[serialization(rename = "system-management-password-replacement")]
    Password,
}
/// Protected recovery material. Contains the new password verifier, never the
/// submitted password. Persist only in an admitted private control directory;
/// never send this value in an API response or diagnostics.
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct PreparedPasswordReplacement {
    format: ReplacementFormat,
    previous: CredentialRevision,
    replacement: Database,
}
impl PreparedPasswordReplacement {
    pub fn previous(&self) -> &CredentialRevision {
        &self.previous
    }
    pub fn replacement_revision(&self) -> Result<CredentialRevision, ApiError> {
        self.validate()?;
        revision(&self.replacement)
    }
    /// A repeat request must supply the same password without exposing or
    /// returning its retained verifier. This does not authenticate a user.
    pub fn verifies_repeated_password(&self, password: &str) -> Result<bool, ApiError> {
        self.validate()?;
        Ok(primitives::verify_password(
            password,
            &self.replacement.administrator.password_hash,
        ))
    }
    fn validate(&self) -> Result<(), ApiError> {
        self.replacement.validate()?;
        primitives::require_canonical_administrator_username(&self.previous.principal)
            .map_err(ApiError::invalid_source)?;
        if self.previous.generation == 0
            || self.previous.fingerprint.len() != 64
            || !self
                .previous
                .fingerprint
                .bytes()
                .all(|value| value.is_ascii_digit() || (b'a'..=b'f').contains(&value))
            || self.previous.generation.checked_add(1)
                != Some(self.replacement.administrator.generation)
            || self.previous.principal != self.replacement.administrator.username
            || !self.replacement.sessions.is_empty()
        {
            return Err(ApiError::invalid(
                "invalid credential replacement baseline, generation or session policy",
            ));
        }
        Ok(())
    }
}
#[derive(Clone, Copy, Eq, PartialEq)]
pub enum PasswordReplacementOutcome {
    Published,
    ExistingDurablyVerified,
}
fn revision(database: &Database) -> Result<CredentialRevision, ApiError> {
    database.validate()?;
    let bytes = json_codec::to_vec(database)
        .expect("invariant: credential database serialization succeeds");
    let mut hasher = blake3::Hasher::new();
    hasher.update(b"zxs-system-management-credential-revision\0");
    hasher.update(&bytes);
    Ok(CredentialRevision {
        principal: database.administrator.username.clone(),
        generation: database.administrator.generation,
        fingerprint: hasher.finalize().to_hex().to_string(),
    })
}
impl AuthStore {
    pub fn credential_revision(&self) -> Result<CredentialRevision, ApiError> {
        revision(self.database()?)
    }

    /// Prepare a single generation increment and revoke every session. The
    /// caller owns authorization and a bound on concurrent password hashing.
    /// No file or in-memory authority is changed by preparation.
    pub fn prepare_system_password_replacement(
        &self,
        password: &str,
    ) -> Result<PreparedPasswordReplacement, ApiError> {
        if !self.local_authority {
            return Err(ApiError::invalid(
                "password replacement requires the system-management authority, not an engine cache",
            ));
        }
        let database = self.database()?;
        let generation = database
            .administrator
            .generation
            .checked_add(1)
            .ok_or_else(|| {
                ApiError::conflict(
                    storage_contract::ErrorCode::AuthStateInvalid,
                    "administrator generation has been exhausted",
                )
            })?;
        let password_hash =
            primitives::hash_password(password).map_err(ApiError::authentication_operation)?;
        Ok(PreparedPasswordReplacement {
            format: ReplacementFormat::Password,
            previous: revision(database)?,
            replacement: Database {
                administrator: Administrator {
                    username: database.administrator.username.clone(),
                    password_hash,
                    generation,
                },
                sessions: BTreeMap::new(),
            },
        })
    }

    /// Publish only against the exact original protected snapshot, or resolve
    /// the exact already-published replacement without another generation bump.
    /// Uncertain I/O leaves the normal store admission fence closed; reopen the
    /// real authority and inspect the retained operation before retrying.
    pub fn apply_system_password_replacement(
        &mut self,
        change: &PreparedPasswordReplacement,
    ) -> Result<PasswordReplacementOutcome, ApiError> {
        if !self.local_authority {
            return Err(ApiError::invalid(
                "password replacement requires the system-management authority",
            ));
        }
        change.validate()?;
        let current = self.credential_revision()?;
        if current == revision(&change.replacement)? {
            self.synchronize_current_credentials()?;
            return Ok(PasswordReplacementOutcome::ExistingDurablyVerified);
        }
        if current != change.previous {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::AuthStateInvalid,
                "credential authority differs from the original replacement; no generation was overwritten",
            ));
        }
        self.synchronize_current_credentials()?;
        let previous = self.db.replace(change.replacement.clone());
        if let Err(error) = self.persist(false) {
            self.db = previous;
            return Err(error);
        }
        if let Err(error) = self.synchronize_current_credentials() {
            self.blocked.store(true, Ordering::SeqCst);
            return Err(error);
        }
        Ok(PasswordReplacementOutcome::Published)
    }

    fn synchronize_current_credentials(&self) -> Result<(), ApiError> {
        let result = (|| {
            let directory = self
                .directory
                .as_ref()
                .expect("invariant: available credentials retain their admitted directory");
            directory
                .verify_identity()
                .map_err(credential_publication_error)?;
            let file = platform_io::fs_safety::linux::open_private_regular(directory, &self.name)
                .map_err(credential_publication_error)?;
            if file.metadata().map_err(ApiError::io)?.len() > MAX_STORE_BYTES as u64 {
                return Err(ApiError::limit(
                    "credential store exceeds serialized byte limit",
                ));
            }
            let mut bytes = Vec::new();
            (&file)
                .take(MAX_STORE_BYTES as u64 + 1)
                .read_to_end(&mut bytes)
                .map_err(ApiError::io)?;
            if bytes.len() > MAX_STORE_BYTES {
                return Err(ApiError::limit(
                    "credential store exceeds serialized byte limit",
                ));
            }
            let actual = storage_contract::strict_json(&bytes).map_err(ApiError::invalid_source)?;
            if revision(&Database::from_snapshot(&actual)?)? != self.credential_revision()? {
                return Err(ApiError::conflict(
                    storage_contract::ErrorCode::AuthStateInvalid,
                    "on-disk credentials changed outside the held writer",
                ));
            }
            directory
                .verify_payload(&self.name, &file)
                .map_err(credential_publication_error)?;
            file.sync_all().map_err(ApiError::io)?;
            directory.sync().map_err(credential_publication_error)?;
            directory
                .verify_payload(&self.name, &file)
                .map_err(credential_publication_error)?;
            Ok(())
        })();
        if result.is_err() {
            self.blocked.store(true, Ordering::SeqCst);
        }
        result
    }
}

#[cfg(test)]
#[path = "../../../../test/server/unit/auth/replacement_tests.rs"]
mod tests;
