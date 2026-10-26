//! Offline management operations. HTTP adapters must retain their normal origin,
//! CSRF, request-size and admission limits and hold the runtime lifecycle lock.
//! Disk paths and bootstrap authorization are local deployment inputs, never
//! request-provided paths. Missing configuration is not proof of an empty pool.
use crate::{ApiError, auth::Identity};
use data_serialization::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
};
use storage_engine::{Config, Engine, Mode, RetainedWorkspaceLimits, ValidatedConfig};

mod http;
pub(crate) use http::{protected_routes, public_routes};

pub const MAX_SETUP_BODY_BYTES: usize = 16 * 1024;
const MAX_LOCAL_CONFIG_BYTES: usize = 64 * 1024 * 1024;

/// Opaque local disk/preset IDs, not display labels or filesystem paths.
/// Shared with the daemon's CLI so accepted setup options remain selectable.
pub fn valid_local_setup_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serialization(rename_all = "snake_case")]
pub enum InstallationState {
    Empty,
    Existing,
    RecoveryRequired,
    Unavailable,
}

#[derive(Clone, Debug, Serialize)]
pub struct SetupStatus {
    pub state: InstallationState,
    pub disk_ids: Vec<String>,
    pub local_authorization_required: bool,
    pub allow_development: bool,
    pub completion_available: bool,
    pub config_presets: Vec<String>,
}

// Deliberately no Debug or Serialize: credentials must not enter diagnostics.
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct SetupRequest {
    pub disk_ids: Vec<String>,
    pub mode: Mode,
    pub username: String,
    pub password: String,
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct RestoreRequest {
    pub disk_ids: Vec<String>,
    /// Required only for bootstrap restore after management state loss.
    #[serialization(default)]
    pub accept_stale_credentials: bool,
}

pub struct RecoveryPreviewRequest {
    pub operation_id: storage_contract::OperationId,
    pub reason: String,
}

impl RecoveryPreviewRequest {
    pub(crate) fn parse(input: json_codec::Value) -> Result<Self, ApiError> {
        #[derive(Deserialize)]
        #[serialization(deny_unknown_fields)]
        struct Raw {
            operation_id: String,
            reason: String,
        }
        let raw: Raw = json_codec::from_value(input).map_err(ApiError::invalid_source)?;
        if raw.operation_id.is_empty() {
            return Err(ApiError::invalid("operation_id must be a nonempty string"));
        }
        Ok(Self {
            operation_id: raw
                .operation_id
                .parse::<storage_contract::OperationId>()
                .map_err(|source| engine_error(source.into()))?,
            reason: raw.reason,
        })
    }
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct RecoveryCommitRequest {
    pub recovery_id: String,
    pub digest: String,
    pub accept_permanent_loss: bool,
}

// No Debug/Serialize: these are credentials, not diagnostic data.
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct CompleteSetupRequest {
    pub username: String,
    pub password: String,
}

#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct PresetPreviewRequest {
    pub preset_id: String,
}
#[derive(Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct PresetSetupRequest {
    pub preset_id: String,
    pub config_digest: String,
    pub username: String,
    pub password: String,
}

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct SetupIntent {
    version: u32,
    config: ValidatedConfig,
    disk_ids: Vec<String>,
    bootstrap_binding: String,
}
#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct SetupReady {
    intent_digest: String,
    disk_digests: BTreeMap<String, String>,
}

/// Unforgeable capability, bound to one service and one locally provisioned key.
/// Only verification of bootstrap.key creates this value; loopback alone never does.
pub struct LocalBootstrapPermit {
    binding: blake3::Hash,
    issued: std::time::Instant,
}

pub struct SetupRecovery {
    data_root: PathBuf,
    management_root: PathBuf,
    disks: BTreeMap<String, PathBuf>,
    identities: BTreeMap<String, (u64, u64)>,
    allow_development: bool,
    presets: BTreeMap<String, PathBuf>,
    retained_workspace_limits: Option<RetainedWorkspaceLimits>,
}

impl SetupRecovery {
    /// All roots must be supplied by trusted local deployment configuration.
    /// Unavailable disk descriptors are retained for status, but cannot be selected.
    /// A missing mount is never created and does not block other members.
    pub fn new(
        data_root: PathBuf,
        management_root: PathBuf,
        disks: BTreeMap<String, PathBuf>,
    ) -> Result<Self, ApiError> {
        for root in [&data_root, &management_root] {
            if !root.is_absolute()
                || root
                    .components()
                    .any(|p| p == std::path::Component::ParentDir)
            {
                return Err(ApiError::invalid(
                    "management roots must be absolute without traversal",
                ));
            }
        }
        let data_root = checked_directory(&data_root)?;
        let management_root = checked_directory(&management_root)?;
        let mut canonical = BTreeMap::new();
        let mut identities = BTreeMap::new();
        let mut unique = BTreeSet::new();
        for (id, path) in disks {
            if !valid_local_setup_id(&id) {
                return Err(ApiError::invalid("invalid local disk identifier"));
            }
            if !path.is_absolute()
                || path
                    .components()
                    .any(|p| p == std::path::Component::ParentDir)
            {
                return Err(ApiError::invalid(
                    "disk roots must be absolute without traversal",
                ));
            }
            // Do not require unrelated members to be online to inspect or restore
            // selected survivors. Failure retains an unavailable descriptor; it
            // never grants permission to create a missing directory or mount.
            let available = checked_directory(&path).and_then(|resolved| {
                directory_identity(&resolved).map(|identity| (resolved, identity))
            });
            let resolved = match available {
                Ok((resolved, identity)) => {
                    identities.insert(id.clone(), identity);
                    resolved
                }
                Err(error) => {
                    tracing::debug!(error = ?error, "local setup disk descriptor is unavailable");
                    path
                }
            };
            if !unique.insert(resolved.clone()) {
                return Err(ApiError::invalid("duplicate disk directory"));
            }
            canonical.insert(id, resolved);
        }
        Ok(Self {
            data_root,
            management_root,
            disks: canonical,
            identities,
            allow_development: false,
            presets: BTreeMap::new(),
            retained_workspace_limits: None,
        })
    }

    /// Immutable trusted-local host policy; never derive these limits from a request.
    pub fn with_workspace_limits(
        mut self,
        limits: Option<RetainedWorkspaceLimits>,
    ) -> Result<Self, ApiError> {
        if let Some(limits) = limits {
            limits.validate().map_err(engine_error)?;
        }
        self.retained_workspace_limits = limits;
        Ok(self)
    }

    /// Explicit trusted-local opt-in only; never derive this flag from a request.
    pub fn allow_development_mode(mut self) -> Self {
        self.allow_development = true;
        self
    }

    /// Trusted deployment IDs and paths only. No I/O occurs before a selected,
    /// authenticated preview/application, so a bad preset cannot block Web boot.
    pub fn with_presets(mut self, presets: BTreeMap<String, PathBuf>) -> Self {
        self.presets = presets
            .into_iter()
            .filter(|(id, _)| valid_local_setup_id(id))
            .collect();
        self
    }

    /// This is deliberately conservative. Files, unreadable mounts and partial
    /// setup intent block initialization instead of being treated as an empty pool.
    pub fn status(&self) -> SetupStatus {
        let state = if self
            .data_root
            .join("config.json")
            .symlink_metadata()
            .is_ok()
        {
            InstallationState::Existing
        } else if self
            .data_root
            .join("setup-intent.json")
            .symlink_metadata()
            .is_ok()
            || self
                .data_root
                .join("management-authority.json")
                .symlink_metadata()
                .is_ok()
            || self
                .management_root
                .join("management-auth.json")
                .symlink_metadata()
                .is_ok()
            || self.data_root.join("auth.json").symlink_metadata().is_ok()
        {
            InstallationState::RecoveryRequired
        } else {
            match self.empty_installation() {
                Ok(true) => InstallationState::Empty,
                Ok(false) => InstallationState::RecoveryRequired,
                Err(_) => InstallationState::Unavailable,
            }
        };
        SetupStatus {
            state,
            disk_ids: self.disks.keys().cloned().collect(),
            local_authorization_required: true,
            allow_development: self.allow_development,
            completion_available: self
                .data_root
                .join("setup-ready.json")
                .symlink_metadata()
                .is_ok()
                && self.completion_proof().is_ok(),
            config_presets: self.presets.keys().cloned().collect(),
        }
    }

    fn empty_installation(&self) -> Result<bool, ApiError> {
        if self.disks.is_empty() {
            return Ok(false);
        }
        match fs::read_dir(&self.data_root) {
            Ok(entries) => {
                for entry in entries {
                    let entry = entry.map_err(ApiError::io)?;
                    let name = entry.file_name();
                    if entry.path() == self.management_root
                        && checked_directory(&entry.path())? == self.management_root
                    {
                        continue;
                    }
                    if name != "setup.lock" {
                        return Ok(false);
                    }
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(ApiError::io(e)),
        }
        for (id, path) in &self.disks {
            let expected = self.identities.get(id).ok_or_else(|| {
                ApiError::new(
                    ::http::StatusCode::SERVICE_UNAVAILABLE,
                    storage_contract::ErrorCode::DiskUnavailable,
                    "a configured disk is unavailable",
                )
            })?;
            if checked_directory(path)? != *path || directory_identity(path)? != *expected {
                return Ok(false);
            }
            if fs::read_dir(path)
                .map_err(ApiError::io)?
                .next()
                .transpose()
                .map_err(ApiError::io)?
                .is_some()
            {
                return Ok(false);
            }
        }
        Ok(true)
    }

    /// The token must arrive in a request header, never a URL. Provision exactly
    /// 64 ASCII hexadecimal characters locally in management_root/bootstrap.key
    /// with mode 0600. This function never creates, prints or returns the secret.
    pub fn authorize_bootstrap(&self, token: &str) -> Result<LocalBootstrapPermit, ApiError> {
        if token.len() != 64 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Err(ApiError::unauthorized());
        }
        let expected = self.bootstrap_binding()?;
        let candidate = self.token_binding(token.as_bytes());
        let mismatch = expected
            .as_bytes()
            .iter()
            .zip(candidate.as_bytes())
            .fold(0u8, |n, (a, b)| n | (a ^ b));
        if mismatch != 0 {
            return Err(ApiError::unauthorized());
        }
        Ok(LocalBootstrapPermit {
            binding: expected,
            issued: std::time::Instant::now(),
        })
    }

    fn token_binding(&self, token: &[u8]) -> blake3::Hash {
        let mut h = blake3::Hasher::new();
        h.update(b"zxs-local-bootstrap-v1\0");
        h.update(self.management_root.as_os_str().as_encoded_bytes());
        h.update(b"\0");
        h.update(self.data_root.as_os_str().as_encoded_bytes());
        h.update(b"\0");
        h.update(token);
        h.finalize()
    }

    fn bootstrap_binding(&self) -> Result<blake3::Hash, ApiError> {
        if checked_directory(&self.management_root)? != self.management_root {
            return Err(ApiError::unauthorized());
        }
        let mut file = private_file(&self.management_root.join("bootstrap.key"), false)?;
        let metadata = file.metadata().map_err(ApiError::io)?;
        let modified = metadata.modified().map_err(ApiError::io)?;
        let age = std::time::SystemTime::now()
            .duration_since(modified)
            .map_err(|source| {
                ApiError::new(
                    ::http::StatusCode::UNAUTHORIZED,
                    storage_contract::ErrorCode::BootstrapExpired,
                    "local bootstrap authorization has expired",
                )
                .with_source(source)
            })?;
        if age > std::time::Duration::from_secs(30 * 60) {
            return Err(ApiError::new(
                ::http::StatusCode::UNAUTHORIZED,
                storage_contract::ErrorCode::BootstrapExpired,
                "local bootstrap authorization has expired",
            ));
        }
        if metadata.len() != 64 {
            return Err(ApiError::unauthorized());
        }
        let mut token = [0; 64];
        file.read_exact(&mut token).map_err(ApiError::io)?;
        if !token.iter().all(u8::is_ascii_hexdigit) {
            return Err(ApiError::unauthorized());
        }
        Ok(self.token_binding(&token))
    }

    fn verify_permit(&self, permit: &LocalBootstrapPermit) -> Result<(), ApiError> {
        if permit.issued.elapsed() > std::time::Duration::from_secs(10 * 60)
            || permit.binding != self.bootstrap_binding()?
        {
            return Err(ApiError::unauthorized());
        }
        Ok(())
    }

    fn lock(&self) -> Result<File, ApiError> {
        // Deployment provisions the system directory; HTTP never chooses it.
        if checked_directory(&self.data_root)? != self.data_root {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::SystemRootChanged,
                "system directory changed",
            ));
        }
        let file = private_file(&self.data_root.join("setup.lock"), true)?;
        file.try_lock().map_err(|source| {
            ApiError::conflict(
                storage_contract::ErrorCode::SetupBusy,
                "another setup or recovery operation is active",
            )
            .with_source(source)
        })?;
        Ok(file)
    }

    fn selected(&self, ids: &[String]) -> Result<Vec<PathBuf>, ApiError> {
        let mut unique = BTreeSet::new();
        ids.iter()
            .map(|id| {
                if !unique.insert(id) {
                    return Err(ApiError::invalid("duplicate disk identifier"));
                }
                let path = self
                    .disks
                    .get(id)
                    .ok_or_else(|| ApiError::invalid("disk is not locally authorized"))?;
                let expected = self.identities.get(id).ok_or_else(|| {
                    ApiError::new(
                        ::http::StatusCode::SERVICE_UNAVAILABLE,
                        storage_contract::ErrorCode::DiskUnavailable,
                        "selected disk is unavailable",
                    )
                })?;
                // Check only selected usable roots: unavailable unrelated descriptors
                // cannot block management or recovery from healthy survivors.
                if [&self.data_root, &self.management_root]
                    .into_iter()
                    .any(|root| path.starts_with(root) || root.starts_with(path))
                {
                    return Err(ApiError::invalid(
                        "storage disk overlaps system or management directory",
                    ));
                }
                if checked_directory(path)? != *path || directory_identity(path)? != *expected {
                    return Err(ApiError::conflict(
                        storage_contract::ErrorCode::DiskChanged,
                        "disk identity changed",
                    ));
                }
                Ok(path.clone())
            })
            .collect()
    }

    /// Serialized, fail-closed setup. The intent is synced BEFORE disk writes.
    /// A failure retains it and must be explicitly recovered, never auto-retried
    /// as a new installation. The callback installs protected management auth
    /// and its authority marker, then drops the engine for a serialized runtime reopen.
    pub fn initialize<F>(
        &self,
        permit: &LocalBootstrapPermit,
        request: SetupRequest,
        install: F,
    ) -> Result<(), ApiError>
    where
        F: FnOnce(Engine, &str, &str) -> Result<(), ApiError>,
    {
        self.verify_permit(permit)?;
        authentication::normalize_administrator_username(&request.username)
            .map_err(ApiError::invalid_source)?;
        authentication::validate_password(&request.password).map_err(ApiError::invalid_source)?;
        let _lock = self.lock()?;
        self.verify_permit(permit)?;
        if self.status().state != InstallationState::Empty {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::RecoveryRequired,
                "only a verified empty installation can be initialized",
            ));
        }
        if request.disk_ids.is_empty() || request.disk_ids.len() > 32 {
            return Err(ApiError::invalid(
                "select between one and 32 authorized storage disks",
            ));
        }
        let paths = self.selected(&request.disk_ids)?;
        if request.mode == Mode::Development && !self.allow_development {
            return Err(ApiError::forbidden());
        }
        if request.mode == Mode::Production {
            for path in &paths {
                require_mountpoint(path)?;
            }
        }
        let mut config = Config::development(paths);
        config.mode = request.mode;
        let config = ValidatedConfig::try_from(config).map_err(engine_error)?;
        self.initialize_config(
            permit,
            config,
            request.disk_ids.to_vec(),
            &request.username,
            &request.password,
            install,
        )
    }

    fn initialize_config<F>(
        &self,
        permit: &LocalBootstrapPermit,
        config: ValidatedConfig,
        disk_ids: Vec<String>,
        username: &str,
        password: &str,
        install: F,
    ) -> Result<(), ApiError>
    where
        F: FnOnce(Engine, &str, &str) -> Result<(), ApiError>,
    {
        self.verify_permit(permit)?;
        // A locally provisioned limit below the selected (including preset)
        // baseline must not publish an intent that strands a pristine setup.
        let config = config
            .with_workspace_limits(self.retained_workspace_limits)
            .map_err(engine_error)?;
        let intent = SetupIntent {
            version: 2,
            config: config.clone(),
            disk_ids,
            bootstrap_binding: permit.binding.to_hex().to_string(),
        };
        self.write_control("setup-intent.json", &intent)?;
        let engine = Engine::initialize_validated(&self.data_root, config).map_err(engine_error)?;
        self.write_ready(&intent)?;
        // Disk I/O may outlive or revoke the original local authorization.
        // Never publish account authority after that authorization is gone.
        self.verify_permit(permit)?;
        install(engine, username, password)?;
        // Keep a durable record even after success. Missing config later cannot
        // make this previously initialized installation look new.
        Ok(())
    }

    fn preset(&self, id: &str) -> Result<(ValidatedConfig, Vec<String>), ApiError> {
        let path = self
            .presets
            .get(id)
            .ok_or_else(|| ApiError::invalid("setup preset is not locally configured"))?;
        let config = read_preset(path)?;
        let mut ids = Vec::new();
        let mut roots = Vec::with_capacity(config.disks.len());
        for disk in &config.disks {
            let canonical = checked_directory(&disk.root)?;
            let id = self
                .disks
                .iter()
                .find_map(|(id, allowed)| (allowed == &canonical).then_some(id.clone()))
                .ok_or_else(ApiError::forbidden)?;
            roots.push(self.selected(std::slice::from_ref(&id))?.remove(0));
            ids.push(id);
        }
        let config = config.with_disk_roots(roots).map_err(engine_error)?;
        if config.mode == Mode::Development && !self.allow_development {
            return Err(ApiError::forbidden());
        }
        if config.mode == Mode::Production {
            for disk in &config.disks {
                require_mountpoint(&disk.root)?;
            }
        }
        Ok((config, ids))
    }

    pub fn preview_preset(
        &self,
        permit: &LocalBootstrapPermit,
        request: PresetPreviewRequest,
    ) -> Result<json_codec::Value, ApiError> {
        self.verify_permit(permit)?;
        let (config, ids) = self.preset(&request.preset_id)?;
        let config_digest = value_digest(&config)?;
        let mut safe =
            json_codec::to_value(&config).expect("invariant: internal JSON serialization succeeds");
        for (disk, id) in safe["disks"]
            .as_array_mut()
            .ok_or_else(|| ApiError::invalid("invalid preset projection"))?
            .iter_mut()
            .zip(ids)
        {
            let object = disk
                .as_object_mut()
                .ok_or_else(|| ApiError::invalid("invalid preset disk projection"))?;
            object.remove("root");
            object.insert("candidate_id".into(), json_codec::Value::String(id));
        }
        Ok(
            json_codec::json!({"preset_id":request.preset_id,"config_digest":config_digest,"configuration":safe}),
        )
    }

    pub fn initialize_preset<F>(
        &self,
        permit: &LocalBootstrapPermit,
        request: PresetSetupRequest,
        install: F,
    ) -> Result<(), ApiError>
    where
        F: FnOnce(Engine, &str, &str) -> Result<(), ApiError>,
    {
        self.verify_permit(permit)?;
        authentication::normalize_administrator_username(&request.username)
            .map_err(ApiError::invalid_source)?;
        authentication::validate_password(&request.password).map_err(ApiError::invalid_source)?;
        let _lock = self.lock()?;
        self.verify_permit(permit)?;
        if self.status().state != InstallationState::Empty {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::RecoveryRequired,
                "only a verified empty installation can be initialized",
            ));
        }
        let (config, ids) = self.preset(&request.preset_id)?;
        if request.config_digest != value_digest(&config)? {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::SetupPresetChanged,
                "review the current setup preset before applying it",
            ));
        }
        self.initialize_config(
            permit,
            config,
            ids,
            &request.username,
            &request.password,
            install,
        )
    }

    fn write_control<T: Serialize>(&self, name: &str, value: &T) -> Result<(), ApiError> {
        let bytes =
            json_codec::to_vec(value).expect("invariant: internal JSON serialization succeeds");
        if bytes.len() > MAX_LOCAL_CONFIG_BYTES {
            return Err(ApiError::invalid("setup control record exceeds limit"));
        }
        let path = self.data_root.join(name);
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(path).map_err(ApiError::io)?;
        file.write_all(&bytes).map_err(ApiError::io)?;
        file.sync_all().map_err(ApiError::io)?;
        File::open(&self.data_root)
            .and_then(|f| f.sync_all())
            .map_err(ApiError::io)
    }

    fn write_ready(&self, intent: &SetupIntent) -> Result<(), ApiError> {
        let mut disk_digests = BTreeMap::new();
        for id in &intent.disk_ids {
            disk_digests.insert(
                id.clone(),
                tree_digest(&self.selected(std::slice::from_ref(id))?[0])?,
            );
        }
        self.write_control(
            "setup-ready.json",
            &SetupReady {
                intent_digest: value_digest(intent)?,
                disk_digests,
            },
        )
    }

    fn completion_proof(&self) -> Result<SetupIntent, ApiError> {
        for path in [
            self.data_root.join("management-authority.json"),
            self.management_root.join("management-auth.json"),
            self.data_root.join("auth.json"),
        ] {
            match path.symlink_metadata() {
                Ok(_) => {
                    return Err(ApiError::conflict(
                        storage_contract::ErrorCode::AuthorityRecoveryRequired,
                        "existing authority or credentials prohibit initialization completion",
                    ));
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                Err(e) => return Err(ApiError::io(e)),
            }
        }
        let intent: SetupIntent = read_control(&self.data_root.join("setup-intent.json"))?;
        let ready: SetupReady = read_control(&self.data_root.join("setup-ready.json"))?;
        if intent.version != 2
            || intent.bootstrap_binding != self.bootstrap_binding()?.to_hex().as_str()
            || ready.intent_digest != value_digest(&intent)?
        {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::SetupProofInvalid,
                "no matching protected initialization intent",
            ));
        }
        let config: Config = read_control(&self.data_root.join("config.json"))?;
        if value_digest(&config)? != value_digest(&intent.config)?
            || ready.disk_digests.len() != intent.disk_ids.len()
        {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::SetupProofInvalid,
                "initialized configuration differs from its bound intent",
            ));
        }
        let paths = self.selected(&intent.disk_ids)?;
        if intent.config.mode == Mode::Development && !self.allow_development {
            return Err(ApiError::forbidden());
        }
        if intent.config.mode == Mode::Production {
            for path in &paths {
                require_mountpoint(path)?;
            }
        }
        if intent.config.disks.len() != paths.len()
            || intent
                .config
                .disks
                .iter()
                .zip(&paths)
                .any(|(disk, path)| disk.root != *path)
        {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::SetupProofInvalid,
                "initialized disks differ from their bound intent",
            ));
        }
        for (id, path) in intent.disk_ids.iter().zip(paths) {
            if ready.disk_digests.get(id) != Some(&tree_digest(&path)?) {
                return Err(ApiError::conflict(
                    storage_contract::ErrorCode::SetupContentChanged,
                    "disk contents changed since interrupted initialization",
                ));
            }
        }
        let workspaces = self.data_root.join("workspaces");
        checked_directory(&workspaces)?;
        if fs::read_dir(&workspaces)
            .map_err(ApiError::io)?
            .next()
            .transpose()
            .map_err(ApiError::io)?
            .is_some()
        {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::SetupContentChanged,
                "existing workspaces prohibit initialization completion",
            ));
        }
        Ok(intent)
    }

    /// Complete only our proven post-storage, pre-authority initialization stage.
    /// Existing authority, credentials, user data or old/unbound intents never reset.
    pub fn complete_initialization<F>(
        &self,
        permit: &LocalBootstrapPermit,
        request: CompleteSetupRequest,
        install: F,
    ) -> Result<(), ApiError>
    where
        F: FnOnce(Engine, &str, &str) -> Result<(), ApiError>,
    {
        self.verify_permit(permit)?;
        authentication::normalize_administrator_username(&request.username)
            .map_err(ApiError::invalid_source)?;
        authentication::validate_password(&request.password).map_err(ApiError::invalid_source)?;
        let _lock = self.lock()?;
        self.verify_permit(permit)?;
        self.completion_proof()?;
        let mut engine = Engine::open_for_recovery_with_workspace_limits(
            &self.data_root,
            self.retained_workspace_limits,
        )
        .map_err(engine_error)?;
        crate::engine_backend::report_recovery_warnings(&mut engine);
        let pristine = storage_engine::State {
            topology_generation: 1,
            ..Default::default()
        };
        if engine.is_read_only()
            || engine.auth_state().is_some()
            || value_digest(engine.state())? != value_digest(&pristine)?
        {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::SetupContentChanged,
                "only a pristine unpublished installation can be completed",
            ));
        }
        // Recheck after core open, before any authority publication.
        self.completion_proof()?;
        self.verify_permit(permit)?;
        install(engine, &request.username, &request.password)
    }

    // Internal management operations receive identities revalidated by
    // ManagementBackend::setup_operation against current system authority.
    /// Isolated proof verification, not the healthy /api/admin/check endpoint.
    /// No client-provided export destination and no file payload transfer here.
    pub(crate) fn offline_check(
        &self,
        _identity: &Identity,
        disk_id: &str,
    ) -> Result<storage_engine::CheckReport, ApiError> {
        let _lock = self.lock()?;
        let paths = self.selected(&[disk_id.to_owned()])?;
        storage_engine::offline_check(&paths[0]).map_err(engine_error)
    }

    /// This never weakens ordinary write quorum. The core accepts only an
    /// already-decided unpublished operation with all original disks accessible.
    pub(crate) fn recovery_preview(
        &self,
        identity: &Identity,
        request: RecoveryPreviewRequest,
        revalidate: impl FnOnce(&Identity) -> Result<(), ApiError>,
    ) -> Result<storage_engine::RecoveryPlan, ApiError> {
        let _lock = self.lock()?;
        let mut engine = Engine::open_for_recovery_with_workspace_limits(
            &self.data_root,
            self.retained_workspace_limits,
        )
        .map_err(engine_error)?;
        crate::engine_backend::report_recovery_warnings(&mut engine);
        #[cfg(test)]
        tests::after_recovery_open();
        // Opening/replaying storage can outlive the entrance session. Admit
        // this new recovery action only after the engine and its locks exist.
        revalidate(identity)?;
        engine
            .prepare_degraded_recovery(request.operation_id, &identity.principal, &request.reason)
            .map_err(engine_error)
    }

    pub(crate) fn recovery_commit(
        &self,
        identity: &Identity,
        request: RecoveryCommitRequest,
        revalidate: impl FnOnce(&Identity) -> Result<(), ApiError>,
    ) -> Result<storage_engine::OperationResult, ApiError> {
        if !request.accept_permanent_loss {
            return Err(ApiError::invalid(
                "explicit permanent-loss acknowledgement is required",
            ));
        }
        let _lock = self.lock()?;
        let mut engine = Engine::open_for_recovery_with_workspace_limits(
            &self.data_root,
            self.retained_workspace_limits,
        )
        .map_err(engine_error)?;
        crate::engine_backend::report_recovery_warnings(&mut engine);
        #[cfg(test)]
        tests::after_recovery_open();
        // Do not hold the authority lock across disk I/O. Once this fresh
        // admission succeeds, the core keeps its normal completion semantics.
        revalidate(identity)?;
        engine
            .commit_degraded_recovery(
                &request.recovery_id,
                &request.digest,
                &identity.principal,
                true,
            )
            .map_err(engine_error)
    }

    /// Requires explicit local authorization when the system state is lost.
    /// Recovery itself enforces the quorum fixed by the original metadata membership.
    /// The callback restores EXISTING authoritative credentials only; it must
    /// never initialize credentials if the recovered pool has no auth snapshot.
    pub fn restore_system<F>(
        &self,
        permit: &LocalBootstrapPermit,
        request: RestoreRequest,
        restore: F,
    ) -> Result<(), ApiError>
    where
        F: FnOnce(Engine) -> Result<(), ApiError>,
    {
        self.verify_permit(permit)?;
        let _lock = self.lock()?;
        self.verify_permit(permit)?;
        if self
            .management_root
            .join("management-auth.json")
            .symlink_metadata()
            .is_ok()
        {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::SystemAuthRequired,
                "use existing system administrator credentials",
            ));
        }
        if !request.accept_stale_credentials {
            return Err(ApiError::invalid(
                "acknowledge that restored administrator credentials may be stale; restored sessions are always revoked",
            ));
        }
        self.restore_locked(request, |engine| {
            // Recovery can outlive local authorization. Recheck at the authority
            // publication boundary, not just before slow disk recovery begins.
            self.verify_permit(permit)?;
            restore(engine)
        })
    }

    /// Normal restore uses a freshly authenticated system administrator.
    pub(crate) fn restore_authenticated<F>(
        &self,
        _identity: &Identity,
        request: RestoreRequest,
        restore: F,
    ) -> Result<(), ApiError>
    where
        F: FnOnce(Engine) -> Result<(), ApiError>,
    {
        let _lock = self.lock()?;
        self.restore_locked(request, restore)
    }

    fn restore_locked<F>(&self, request: RestoreRequest, restore: F) -> Result<(), ApiError>
    where
        F: FnOnce(Engine) -> Result<(), ApiError>,
    {
        if request.disk_ids.is_empty() || request.disk_ids.len() > 32 {
            return Err(ApiError::invalid(
                "restore requires authorized metadata members; the original configuration determines quorum",
            ));
        }
        if self
            .data_root
            .join("config.json")
            .symlink_metadata()
            .is_ok()
        {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::AlreadyInitialized,
                "configuration exists; use diagnostic recovery",
            ));
        }
        let disks = self.selected(&request.disk_ids)?;
        let mut engine = Engine::restore_from_disks_with_workspace_limits(
            &self.data_root,
            &disks,
            self.retained_workspace_limits,
        )
        .map_err(engine_error)?;
        crate::engine_backend::report_recovery_warnings(&mut engine);
        if engine.auth_state().is_none() {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::AuthRecoveryRequired,
                "storage configuration was restored but authoritative credentials are absent; initialization remains blocked and explicit local recovery is required",
            ));
        }
        #[cfg(test)]
        tests::after_restore();
        restore(engine)
    }
}

fn checked_directory(path: &Path) -> Result<PathBuf, ApiError> {
    if !path.is_absolute() {
        return Err(ApiError::invalid("disk directory must be absolute"));
    }
    let metadata = fs::symlink_metadata(path).map_err(ApiError::io)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(ApiError::invalid("directory must not be a symbolic link"));
    }
    path.canonicalize().map_err(ApiError::io)
}
fn private_file(path: &Path, create: bool) -> Result<File, ApiError> {
    let mut flags = rustix::fs::OFlags::RDONLY
        | rustix::fs::OFlags::NOFOLLOW
        | rustix::fs::OFlags::NONBLOCK
        | rustix::fs::OFlags::CLOEXEC;
    if create {
        flags = (flags & !rustix::fs::OFlags::RDONLY)
            | rustix::fs::OFlags::RDWR
            | rustix::fs::OFlags::CREATE;
    }
    let file = File::from(
        rustix::fs::open(path, flags, rustix::fs::Mode::from_raw_mode(0o600))
            .map_err(std::io::Error::from)
            .map_err(ApiError::io)?,
    );
    let metadata = file.metadata().map_err(ApiError::io)?;
    if !metadata.is_file() {
        return Err(ApiError::invalid("invalid private control file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if metadata.mode() & 0o777 != 0o600 || metadata.nlink() != 1 {
            return Err(ApiError::invalid(
                "private control file requires mode 0600 and one link",
            ));
        }
    }
    Ok(file)
}
fn engine_error(error: storage_engine::Error) -> ApiError {
    // Core errors can contain local paths. Keep HTTP diagnostics bounded and
    // private without presenting uncertain storage writes as definite conflicts.
    let mut mapped = crate::engine_backend::error(error);
    mapped.message = "offline storage operation could not complete safely".into();
    mapped
}

#[cfg(test)]
#[path = "../../../test/server/unit/setup_recovery_tests.rs"]
mod tests;

fn directory_identity(path: &Path) -> Result<(u64, u64), ApiError> {
    use std::os::unix::fs::MetadataExt;
    let metadata = fs::symlink_metadata(path).map_err(ApiError::io)?;
    Ok((metadata.dev(), metadata.ino()))
}
fn require_mountpoint(path: &Path) -> Result<(), ApiError> {
    let mounts = fs::read_to_string("/proc/self/mountinfo").map_err(ApiError::io)?;
    let mounted = mounts.lines().any(|line| {
        line.split_once(" - ")
            .and_then(|(left, _)| left.split_whitespace().nth(4))
            .map(|point| {
                PathBuf::from(
                    point
                        .replace("\\040", " ")
                        .replace("\\011", "\t")
                        .replace("\\134", "\\"),
                )
            })
            .is_some_and(|point| point == path)
    });
    if !mounted {
        return Err(ApiError::conflict(
            storage_contract::ErrorCode::DiskNotMounted,
            "production setup requires each configured disk root to be an active mountpoint",
        ));
    }
    Ok(())
}

fn value_digest<T: Serialize>(value: &T) -> Result<String, ApiError> {
    Ok(blake3::hash(
        &json_codec::to_vec(value).expect("invariant: internal JSON serialization succeeds"),
    )
    .to_hex()
    .to_string())
}
fn read_control<T: data_serialization::de::DeserializeOwned>(path: &Path) -> Result<T, ApiError> {
    let mut file = private_file(path, false)?;
    if file.metadata().map_err(ApiError::io)?.len() > MAX_LOCAL_CONFIG_BYTES as u64 {
        return Err(ApiError::invalid("setup control record exceeds limit"));
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(MAX_LOCAL_CONFIG_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(ApiError::io)?;
    if bytes.len() > MAX_LOCAL_CONFIG_BYTES {
        return Err(ApiError::invalid("setup control record exceeds limit"));
    }
    storage_contract::decode_strict(&bytes).map_err(ApiError::invalid_source)
}
fn tree_digest(root: &Path) -> Result<String, ApiError> {
    fn visit(
        root: &Path,
        relative: &Path,
        hash: &mut blake3::Hasher,
        count: &mut usize,
    ) -> Result<(), ApiError> {
        let mut entries = fs::read_dir(root.join(relative))
            .map_err(ApiError::io)?
            .take(129)
            .collect::<Result<Vec<_>, _>>()
            .map_err(ApiError::io)?;
        if entries.len().saturating_add(*count) > 128 {
            return Err(ApiError::conflict(
                storage_contract::ErrorCode::SetupContentChanged,
                "initialization tree no longer pristine",
            ));
        }
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            *count += 1;
            if *count > 128 {
                return Err(ApiError::conflict(
                    storage_contract::ErrorCode::SetupContentChanged,
                    "initialization tree no longer pristine",
                ));
            }
            let path = relative.join(entry.file_name());
            let metadata = entry.path().symlink_metadata().map_err(ApiError::io)?;
            hash.update(path.as_os_str().as_encoded_bytes());
            hash.update(b"\0");
            if metadata.file_type().is_symlink() {
                return Err(ApiError::invalid(
                    "symbolic links prohibit initialization completion",
                ));
            }
            if metadata.is_dir() {
                hash.update(b"dir\0");
                visit(root, &path, hash, count)?;
            } else if metadata.is_file() {
                if metadata.len() > MAX_LOCAL_CONFIG_BYTES as u64 {
                    return Err(ApiError::conflict(
                        storage_contract::ErrorCode::SetupContentChanged,
                        "initialization tree contains unexpected data",
                    ));
                }
                let mut file = private_file(&entry.path(), false)?;
                let mut bytes = Vec::new();
                Read::by_ref(&mut file)
                    .take(MAX_LOCAL_CONFIG_BYTES as u64 + 1)
                    .read_to_end(&mut bytes)
                    .map_err(ApiError::io)?;
                if bytes.len() > MAX_LOCAL_CONFIG_BYTES {
                    return Err(ApiError::conflict(
                        storage_contract::ErrorCode::SetupContentChanged,
                        "initialization file changed",
                    ));
                }
                hash.update(b"file\0");
                hash.update(&(bytes.len() as u64).to_le_bytes());
                hash.update(&bytes);
            } else {
                return Err(ApiError::invalid(
                    "special files prohibit initialization completion",
                ));
            }
        }
        Ok(())
    }
    let mut hash = blake3::Hasher::new();
    visit(root, Path::new(""), &mut hash, &mut 0)?;
    Ok(hash.finalize().to_hex().to_string())
}

fn read_preset(path: &Path) -> Result<ValidatedConfig, ApiError> {
    use std::os::unix::fs::MetadataExt;
    if !path.is_absolute()
        || path
            .components()
            .any(|p| p == std::path::Component::ParentDir)
    {
        return Err(ApiError::invalid(
            "setup preset path must be a locally configured absolute path",
        ));
    }
    let mut file = File::from(
        rustix::fs::open(
            path,
            rustix::fs::OFlags::RDONLY
                | rustix::fs::OFlags::NOFOLLOW
                | rustix::fs::OFlags::NONBLOCK
                | rustix::fs::OFlags::CLOEXEC,
            rustix::fs::Mode::empty(),
        )
        .map_err(std::io::Error::from)
        .map_err(ApiError::io)?,
    );
    let metadata = file.metadata().map_err(ApiError::io)?;
    if !metadata.is_file()
        || metadata.nlink() != 1
        || !matches!(metadata.mode() & 0o777, 0o400 | 0o600)
        || metadata.uid() != rustix::process::geteuid().as_raw()
        || metadata.len() > MAX_LOCAL_CONFIG_BYTES as u64
    {
        return Err(ApiError::invalid(
            "setup preset must be a bounded owned private regular single-link file",
        ));
    }
    let mut bytes = Vec::new();
    Read::by_ref(&mut file)
        .take(MAX_LOCAL_CONFIG_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(ApiError::io)?;
    if bytes.len() > MAX_LOCAL_CONFIG_BYTES {
        return Err(ApiError::invalid("setup preset exceeds limit"));
    }
    let config: Config = storage_contract::decode_strict(&bytes).map_err(|source| {
        ApiError::invalid("local setup preset is not a valid configuration").with_source(source)
    })?;
    ValidatedConfig::try_from(config).map_err(engine_error)
}

/// Deterministic test-only interruption after durable disk recovery.
#[cfg(test)]
pub(crate) fn after_next_restore(action: impl FnOnce() + 'static) {
    tests::after_next_restore(action);
}

/// Deterministic test-only interruption before a new recovery action is admitted.
#[cfg(test)]
pub(crate) fn after_next_recovery_open(action: impl FnOnce() + 'static) {
    tests::after_next_recovery_open(action);
}
