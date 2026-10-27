//! Typed public service observations. These snapshots never grant admission.
use data_serialization::Serialize;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageState {
    Ready,
    ReadOnly,
    Unavailable,
    Unconfigured,
    RecoveryRequired,
}
impl StorageState {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "ready",
            Self::ReadOnly => "read_only",
            Self::Unavailable => "unavailable",
            Self::Unconfigured => "unconfigured",
            Self::RecoveryRequired => "recovery_required",
        }
    }
}

/// Embedded engines do not run the management recovery worker. Managed status
/// is an observation, not an authorization or a promise about a later request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StorageStatus {
    EmbeddedReady,
    Managed {
        state: StorageState,
        recovering: bool,
    },
}
impl StorageStatus {
    pub fn state(self) -> StorageState {
        match self {
            Self::EmbeddedReady => StorageState::Ready,
            Self::Managed { state, .. } => state,
        }
    }
    pub fn recovering(self) -> bool {
        match self {
            Self::EmbeddedReady => false,
            Self::Managed { recovering, .. } => recovering,
        }
    }
}
#[derive(Serialize)]
pub(crate) struct StorageStatusResponse {
    state: &'static str,
    retryable: bool,
    #[serialization(skip_serializing_if = "Option::is_none")]
    recovering: Option<bool>,
}
impl From<StorageStatus> for StorageStatusResponse {
    fn from(status: StorageStatus) -> Self {
        Self {
            state: status.state().as_str(),
            retryable: status.state() != StorageState::Ready,
            recovering: match status {
                StorageStatus::EmbeddedReady => None,
                StorageStatus::Managed { recovering, .. } => Some(recovering),
            },
        }
    }
}
#[derive(Clone, Copy, Debug)]
enum CapabilityProfile {
    Unavailable,
    Engine,
}
/// A known capability profile and its numeric upload ceiling. A cached engine
/// profile survives temporary storage unavailability; `storage` reports that
/// independently. No profile or numeric field can be omitted by a backend.
#[derive(Clone, Debug)]
pub struct Capabilities {
    profile: CapabilityProfile,
    max_upload_bytes: u64,
    storage: Option<StorageStatus>,
}
impl Capabilities {
    /// The current engine feature set with the backend's upload ceiling.
    pub fn for_engine(max_upload_bytes: u64) -> Self {
        Self {
            profile: CapabilityProfile::Engine,
            max_upload_bytes,
            storage: None,
        }
    }
    /// Before any engine configuration has been observed, uploads are disabled.
    pub fn unavailable() -> Self {
        Self {
            profile: CapabilityProfile::Unavailable,
            max_upload_bytes: 0,
            storage: None,
        }
    }
    pub fn max_upload_bytes(&self) -> u64 {
        self.max_upload_bytes
    }
    /// Attach a management observation without granting any authorization.
    pub fn with_storage(mut self, storage: StorageStatus) -> Self {
        self.storage = Some(storage);
        self
    }
    pub(crate) fn with_upload_limit(mut self, limit: u64) -> Self {
        self.max_upload_bytes = self.max_upload_bytes.min(limit);
        self
    }
}
#[derive(Serialize)]
#[serialization(untagged)]
pub(crate) enum CapabilitiesResponse {
    Engine(EngineCapabilitiesResponse),
    Unavailable(UnavailableCapabilitiesResponse),
}
#[derive(Serialize)]
pub(crate) struct UnavailableCapabilitiesResponse {
    features: [&'static str; 0],
    #[serialization(with = "storage_contract::u64_string")]
    max_upload_bytes: u64,
    #[serialization(skip_serializing_if = "Option::is_none")]
    storage: Option<StorageStatusResponse>,
}
#[derive(Serialize)]
struct CapabilityPlatforms {
    web: bool,
    smb: bool,
}
#[derive(Serialize)]
struct EngineCapabilityFeatures {
    name_policy: &'static str,
    path_policy: &'static str,
    unicode_data_version: &'static str,
    supported_save_modes: &'static [&'static str],
    full_only: bool,
    full_branches: bool,
    branch_creation: &'static str,
    branch_content: &'static str,
    file_branches: bool,
    branch_scope: &'static str,
    directory_branches: bool,
    directory_branch_creation: &'static str,
    directory_subtrees: bool,
    namespace_batch: bool,
    smb_default_branch: &'static str,
    file_manager: bool,
    historical_save_creates_branch: bool,
    merge_behavior: &'static str,
    file_branch_merge: bool,
    directory_branch_merge: bool,
    directory_merge_status: &'static str,
    file_node_tags: bool,
    backup_configuration: bool,
    range_upload: bool,
    complete_upload: bool,
    authentication: &'static str,
    platforms: CapabilityPlatforms,
    operations: &'static [&'static str],
}

static ENGINE_FEATURES: EngineCapabilityFeatures = EngineCapabilityFeatures {
    name_policy: name_policy::NAME_POLICY,
    path_policy: "PortablePath",
    unicode_data_version: name_policy::UNICODE_VERSION,
    supported_save_modes: &["full"],
    full_only: true,
    full_branches: false,
    branch_creation: "record-until-write",
    branch_content: "inherit-parent-until-first-write",
    file_branches: true,
    branch_scope: "file",
    directory_branches: true,
    directory_branch_creation: "inherit-directory-tree",
    directory_subtrees: true,
    namespace_batch: true,
    smb_default_branch: "selected-root-directory",
    file_manager: true,
    historical_save_creates_branch: true,
    merge_behavior: "explicit-file-node-adoption",
    file_branch_merge: true,
    directory_branch_merge: false,
    directory_merge_status: "design-pending",
    file_node_tags: true,
    backup_configuration: true,
    range_upload: false,
    complete_upload: true,
    authentication: "session-bearer-or-cookie",
    platforms: CapabilityPlatforms {
        web: true,
        smb: true,
    },
    operations: &[
        "repositories",
        "tree",
        "workspace",
        "commit",
        "history",
        "graph",
        "branch",
        "merge-link",
        "file-branch-merge",
        "file-node-tags",
        "backup-configuration",
        "purge-plan",
        "split-plan",
        "version-range-read",
        "file-manager",
        "file-branches",
        "file-branch-rename",
        "file-path-nodes",
        "directory-branches",
        "directory-subtrees",
        "namespace-batch",
        "default-branch",
        "file-notes",
        "system-status",
    ],
};
#[derive(Serialize)]
pub(crate) struct EngineCapabilitiesResponse {
    #[serialization(flatten)]
    features: &'static EngineCapabilityFeatures,
    #[serialization(with = "storage_contract::u64_string")]
    max_upload_bytes: u64,
    #[serialization(skip_serializing_if = "Option::is_none")]
    storage: Option<StorageStatusResponse>,
}
impl From<Capabilities> for CapabilitiesResponse {
    fn from(capabilities: Capabilities) -> Self {
        let Capabilities {
            profile,
            max_upload_bytes,
            storage,
        } = capabilities;
        let storage = storage.map(StorageStatusResponse::from);
        match profile {
            CapabilityProfile::Unavailable => Self::Unavailable(UnavailableCapabilitiesResponse {
                features: [],
                max_upload_bytes,
                storage,
            }),
            CapabilityProfile::Engine => Self::Engine(EngineCapabilitiesResponse {
                features: &ENGINE_FEATURES,
                max_upload_bytes,
                storage,
            }),
        }
    }
}
#[cfg(test)]
#[path = "../../../test/server/unit/backend_status.rs"]
mod tests;

/// A successful readiness observation, not a lease for future writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Readiness {
    Managed { history_index: HistoryIndexStatus },
    Engine(EngineReadiness),
}

/// Safe projection of the optional index's availability; no internal error text.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serialization(tag = "state", rename_all = "snake_case")]
pub enum HistoryIndexStatus {
    Unbuilt,
    Ready,
    Unavailable { code: storage_contract::ErrorCode },
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EngineReadiness {
    pub mode: storage_contract::Mode,
    pub sequence: u64,
    pub storage_disk_count: usize,
    pub metadata_member_count: usize,
    pub history_index: HistoryIndexStatus,
}
#[derive(Serialize)]
#[serialization(untagged)]
pub(crate) enum ReadinessResponse {
    Managed(ManagedReadinessResponse),
    Engine(EngineReadinessResponse),
}
#[derive(Serialize)]
pub(crate) struct ManagedReadinessResponse {
    ready: bool,
    history_index: HistoryIndexStatus,
}
#[derive(Serialize)]
pub(crate) struct EngineReadinessResponse {
    ready: bool,
    mode: storage_contract::Mode,
    #[serialization(with = "storage_contract::u64_string")]
    sequence: u64,
    storage_disk_count: usize,
    metadata_member_count: usize,
    metadata_redundancy: &'static str,
    history_index: HistoryIndexStatus,
}
impl From<Readiness> for ReadinessResponse {
    fn from(readiness: Readiness) -> Self {
        match readiness {
            Readiness::Managed { history_index } => Self::Managed(ManagedReadinessResponse {
                ready: true,
                history_index,
            }),
            Readiness::Engine(EngineReadiness {
                mode,
                sequence,
                storage_disk_count,
                metadata_member_count,
                history_index,
            }) => Self::Engine(EngineReadinessResponse {
                ready: true,
                mode,
                sequence,
                storage_disk_count,
                metadata_member_count,
                history_index,
                metadata_redundancy: if metadata_member_count == 1 {
                    "none"
                } else {
                    "multiple-copies"
                },
            }),
        }
    }
}
