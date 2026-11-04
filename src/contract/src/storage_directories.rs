//! Read-only host directory discovery. Identities are observations, not authority
//! to mount, format, erase, or later initialize a path without revalidation.
use data_serialization::{Deserialize, Serialize};

pub const DEFAULT_DISCOVERY_ROOT: &str = "/zxs/data/disks";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DiscoveryRootSettings {
    pub discovery_root: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct SetDiscoveryRootRequest {
    pub expected_discovery_root: String,
    pub discovery_root: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DirectoryIdentity {
    #[serialization(with = "crate::u64_string")]
    pub device: u64,
    #[serialization(with = "crate::u64_string")]
    pub inode: u64,
    /// Signed Unix seconds, encoded as text to preserve the full kernel value.
    /// Change time detects stale previews, not identity across initialization.
    pub ctime_seconds: String,
    pub ctime_nanoseconds: u32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct MountIdentity {
    /// Linux mount namespace observation; not stable across a reboot/remount.
    #[serialization(with = "crate::u64_string")]
    pub mount_id: u64,
    pub device_major: u32,
    pub device_minor: u32,
    #[serialization(with = "crate::u64_string")]
    pub root_inode: u64,
    pub filesystem: String,
    /// Root within this filesystem; distinguishes bind-mount aliases.
    pub filesystem_root: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct InspectRequest {
    pub mount_path: String,
    pub root_path: String,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct CandidateSelection {
    /// A content digest for consistency, not a secret or authorization token.
    pub selection_id: String,
    /// Server-configured discovery scope at inspection time, not client authority.
    pub discovery_root: String,
    pub discovery_identity: DirectoryIdentity,
    pub mount_path: String,
    pub root_path: String,
    pub mount_identity: MountIdentity,
    pub root_identity: DirectoryIdentity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(rename_all = "snake_case")]
pub enum DirectoryProblem {
    SymbolicLink,
    PermissionDenied,
    Unavailable,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DirectoryEntry {
    pub name: String,
    pub path: String,
    /// Only navigation, not admission as a storage root.
    pub can_browse: bool,
    pub problem: Option<DirectoryProblem>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct DirectoryListing {
    pub path: String,
    pub mount_path: String,
    pub mount_identity: MountIdentity,
    pub identity: DirectoryIdentity,
    pub entries: Vec<DirectoryEntry>,
    /// Bounded non-snapshot inventory. A false value does not promise the
    /// directory will remain unchanged after this read.
    pub truncated: bool,
    pub skipped_non_utf8: u32,
}
