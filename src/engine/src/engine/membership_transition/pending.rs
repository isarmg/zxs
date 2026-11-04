//! Pending control metadata is deliberately outside normal disk resources and
//! active journals. These receipts grant neither joint authority nor readiness.
use super::*;
use archive::PendingControlArchive;
use platform_io::fs_safety::linux::FileIdentity;
use std::{
    fs::File,
    path::{Path, PathBuf},
    sync::Arc,
};

pub(crate) mod activation;
mod admission;
mod data;
mod joint;
pub use joint::{MembershipJointDecisionStatus, MembershipJointVote};
pub(super) const CONTROL_ARCHIVE_DIRECTORY: &str = "control-archive";
const ENROLLMENT_KIND: &str = "membership-pending-member";
const BOOTSTRAP_KIND: &str = "member-control-bootstrap";
const PENDING_SEAL_KIND: &str = "membership-pending-seal";

/// Caller-opened, existing private root for one newly added device. The Engine
/// verifies both this descriptor and its exact configured name before writing.
pub struct PendingMemberDirectory {
    pub disk_id: String,
    pub directory: File,
}

/// A synchronized, member-specific control receipt, not an ordinary publication
/// receipt and not permission to write business data or retire an old member.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingMemberControlReceipt {
    pub disk_id: String,
    pub slot_id: String,
    pub binding_generation: u64,
    pub bootstrap_hash: String,
    pub frame_hash: String,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PendingMembershipStatus {
    pub transition_hash: String,
    pub control_archive_root: String,
    pub target_metadata_members: usize,
    pub added_data_disks: usize,
    pub ready_data_disks: Vec<String>,
    pub control_receipts: Vec<PendingMemberControlReceipt>,
}
impl PendingMembershipStatus {
    /// All control-metadata copies are synchronized. This is not full readiness.
    pub fn all_control_metadata_synchronized(&self) -> bool {
        self.control_receipts.len() == self.target_metadata_members
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct MemberIdentity {
    pool_id: String,
    disk_id: String,
    slot_id: String,
    #[serialization(with = "storage_contract::u64_string")]
    binding_generation: u64,
    #[serialization(with = "storage_contract::u64_string")]
    device: u64,
    #[serialization(with = "storage_contract::u64_string")]
    inode: u64,
    simulation: bool,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum PendingFormat {
    #[serialization(rename = "membership-pending")]
    MembershipPending,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct PendingDiskSeal {
    format: PendingFormat,
    identity: MemberIdentity,
    transition_hash: String,
    target_config_hash: String,
    control_archive_root: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(tag = "kind", deny_unknown_fields)]
enum EnrollmentOrigin {
    Retained { original_seal_hash: String },
    Added { pending_seal_hash: String },
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
enum ControlScope {
    #[serialization(rename = "control-metadata-only")]
    ControlMetadataOnly,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct MemberEnrollment {
    scope: ControlScope,
    identity: MemberIdentity,
    origin: EnrollmentOrigin,
    transition_hash: String,
    target_config_hash: String,
    control_archive_root: String,
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(tag = "kind", deny_unknown_fields)]
enum FrozenBoundary {
    EmptyGenesis,
    Published { boundary: PublishedBoundary },
}
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct ControlBootstrap {
    scope: ControlScope,
    identity: MemberIdentity,
    enrollment_hash: String,
    transition_hash: String,
    target_config_hash: String,
    control_archive_root: String,
    frozen_boundary: FrozenBoundary,
    #[serialization(with = "storage_contract::u64_string")]
    fence_sequence: u64,
}

struct PendingDisk {
    root: DiskRoot,
    seal: PendingDiskSeal,
    // Same .writer.lock as ordinary Engine initialization. Never reacquire an
    // old member lock, nor add this resource to normal Engine::disk_resources.
    _lock: File,
}
enum MemberResource {
    Retained(Arc<DiskHandle>),
    Added(Box<PendingDisk>),
}
impl MemberResource {
    fn root(&self) -> &DiskRoot {
        match self {
            Self::Retained(handle) => &handle.root,
            Self::Added(handle) => &handle.root,
        }
    }
}
struct PendingMember {
    disk: DiskConfig,
    resource: MemberResource,
    enrollment: MemberEnrollment,
    bootstrap: PreparedMetadataObject,
}
pub(in crate::engine) struct PendingMembership {
    source: PendingControlArchive,
    transition_hash: String,
    archive_root: String,
    members: Vec<PendingMember>,
    data_disks: Vec<data::PendingDataDisk>,
    ready_data_disks: Vec<String>,
    receipts: BTreeMap<String, Receipt>,
    joint: Option<joint::JointProgress>,
    #[cfg(feature = "fault-injection")]
    journal_fault: Option<JournalFault>,
    #[cfg(feature = "fault-injection")]
    joint_fault: Option<joint::JointFault>,
}
impl PendingMembership {
    fn added_resources(&self) -> impl Iterator<Item = (&DiskConfig, &PendingDisk)> {
        self.members
            .iter()
            .filter_map(|member| match &member.resource {
                MemberResource::Added(resource) => Some((&member.disk, resource.as_ref())),
                MemberResource::Retained(_) => None,
            })
            .chain(
                self.data_disks
                    .iter()
                    .map(|disk| (&disk.disk, disk.resource.as_ref())),
            )
    }
    fn verify_data_readiness(&self) -> Result<()> {
        for disk in &self.data_disks {
            disk.verify_ready()?;
        }
        Ok(())
    }
    fn status(&self) -> PendingMembershipStatus {
        PendingMembershipStatus {
            transition_hash: self.transition_hash.clone(),
            control_archive_root: self.archive_root.clone(),
            target_metadata_members: self.members.len(),
            added_data_disks: self.data_disks.len(),
            ready_data_disks: self.ready_data_disks.clone(),
            control_receipts: self
                .members
                .iter()
                .filter_map(|member| {
                    self.receipts.get(&member.disk.disk_id).map(|receipt| {
                        PendingMemberControlReceipt {
                            disk_id: receipt.disk_id.clone(),
                            slot_id: receipt.slot_id.clone(),
                            binding_generation: member.disk.binding_generation,
                            bootstrap_hash: receipt.record.object_hash.clone(),
                            frame_hash: receipt.frame_hash.clone(),
                        }
                    })
                })
                .collect(),
        }
    }
}

impl Engine {
    pub fn pending_membership_status(&self) -> Option<PendingMembershipStatus> {
        self.pending_membership
            .as_ref()
            .map(PendingMembership::status)
    }

    /// Explicitly enroll/resume only pure device additions under the current original
    /// fence. All supplied roots must already exist and be private. Retained
    /// resources/locks are reused; new resources remain pending until dropped.
    ///
    /// Errors preserve all installed materials and keep business writes blocked.
    /// Once admission succeeds, use `resume_pending_members` on this Engine.
    /// After reopening from old members, call this method again with the same
    /// externally trusted archive root and newly held directories. A source
    /// archive or partial success alone never grants target write authority.
    pub fn enroll_pending_members(
        &mut self,
        source_archive: &Path,
        expected_archive_root: &str,
        directories: Vec<PendingMemberDirectory>,
    ) -> Result<PendingMembershipStatus> {
        if self.pending_membership.is_some() {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "pending resources are already held; explicitly resume the same enrollment",
            ));
        }
        self.require_no_commit_preparation()?;
        let added = self.validate_pending_target()?;
        let source = PendingControlArchive::open(self, source_archive, expected_archive_root)?;
        let pending =
            self.admit_pending_members(source, expected_archive_root, added, directories)?;
        self.pending_membership = Some(pending);
        self.resume_pending_members()
    }

    /// Reopen and verify all pending materials before reusing or appending their
    /// own control receipts. The original fence and expected archive root remain
    /// fixed. Failed or uncertain appends are never retried through an old writer.
    pub fn resume_pending_members(&mut self) -> Result<PendingMembershipStatus> {
        let mut pending = self.pending_membership.take().ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "no explicitly admitted pending resources",
            )
        })?;
        // An older observation is not evidence that this invocation reverified
        // a target. Preserve only actual synchronized progress of this attempt.
        pending.receipts.clear();
        pending.ready_data_disks.clear();
        pending.joint = None;
        let result = self.resume_pending_control(&mut pending);
        let status = pending.status();
        self.pending_membership = Some(pending);
        result?;
        Ok(status)
    }

    fn resume_pending_control(&mut self, pending: &mut PendingMembership) -> Result<()> {
        self.validate_pending_target()?;
        pending.source.verify_current_fence(self)?;
        for member in &pending.members {
            self.validate_pending_resource(member)?;
        }
        for disk in &pending.data_disks {
            disk.validate()?;
        }
        #[cfg(feature = "fault-injection")]
        let mut sealed = 0;
        for (disk, resource) in pending.added_resources() {
            resource
                .root
                .write_json(&disk.root.join("disk.json"), &resource.seal, false)?;
            #[cfg(feature = "fault-injection")]
            {
                sealed += 1;
                self.failpoint(&format!("after_membership_pending_seal_{sealed}"))?;
            }
        }
        for disk in &pending.data_disks {
            disk.synchronize()?;
            pending.ready_data_disks.push(disk.disk.disk_id.clone());
            #[cfg(feature = "fault-injection")]
            self.failpoint(&format!(
                "after_membership_data_ready_{}",
                disk.disk.disk_id
            ))?;
        }
        for member in &pending.members {
            let root = member.resource.root();
            let directory = member_directory(&member.disk, &pending.transition_hash);
            let held_member = admit_member_directory(root, &directory, &member.enrollment)?;
            let root = &held_member;
            #[cfg(feature = "fault-injection")]
            self.failpoint("after_membership_pending_member")?;
            let archive = directory.join(CONTROL_ARCHIVE_DIRECTORY);
            root.private_dir(&archive)?;
            let held_archive = pending.source.install(root.private_child(&archive)?)?;
            #[cfg(feature = "fault-injection")]
            self.failpoint("after_membership_pending_archive")?;
            install_bootstrap(root, &directory, &member.bootstrap)?;
            #[cfg(feature = "fault-injection")]
            self.failpoint("after_membership_pending_object")?;
            let sequence = self
                .membership_fence
                .as_ref()
                .expect("invariant: pending enrollment retains original fence")
                .intent
                .fence_sequence;
            verify_receipt_directories(member, &held_member, &held_archive)?;
            let mut journal =
                PendingJournal::open_at(root, &directory, sequence, &member.bootstrap.hash)?;
            #[cfg(feature = "fault-injection")]
            {
                journal.fault = pending.journal_fault.take();
            }
            let receipt = journal.synchronize(&member.disk)?;
            verify_receipt_directories(member, &held_member, &held_archive)?;
            pending
                .receipts
                .insert(member.disk.disk_id.clone(), receipt);
            #[cfg(feature = "fault-injection")]
            self.failpoint(&format!(
                "after_membership_pending_receipt_{}",
                pending.receipts.len()
            ))?;
        }
        Ok(())
    }
}

pub(super) fn member_directory(disk: &DiskConfig, transition: &str) -> PathBuf {
    disk.root
        .join("metadata/membership-pending")
        .join(transition)
}
fn admit_member_directory(
    root: &DiskRoot,
    path: &Path,
    expected: &MemberEnrollment,
) -> Result<DiskRoot> {
    root.private_dir(path)?;
    // Admit the child before reading or publishing its marker. Every later
    // operation, including the receipt, retains this exact directory identity.
    let held = root.private_child(path)?;
    let root = &held;
    let names = root
        .entries(path)?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(fs_error)?;
    if !names.is_empty() {
        let saved: MemberEnrollment =
            decode_strict(&root.read_bytes(&path.join("member.json"), MAX_METADATA)?)?;
        if &saved != expected {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "pending member enrollment differs",
            ));
        }
        for name in names {
            if platform_io::fs_safety::AtomicFile::is_temporary_name(&name) {
                root.open_regular(&path.join(name.as_path()))?;
            } else if !matches!(
                name.as_os_str().to_str(),
                Some("member.json" | "control-archive" | "objects" | "journal.bin" | "joint")
            ) {
                return Err(Error::new(
                    ErrorCode::UnsafePath,
                    "pending member contains unrelated entries",
                ));
            }
        }
    }
    root.write_json(&path.join("member.json"), expected, false)?;
    Ok(held)
}

fn verify_receipt_directories(
    member: &PendingMember,
    directory: &DiskRoot,
    archive: &DiskRoot,
) -> Result<()> {
    admission::verify_named_root(member.resource.root())?;
    for (parent, child) in [(member.resource.root(), directory), (directory, archive)] {
        let named = FileIdentity::from_fd(parent.directory(child.path())?)?;
        let held = FileIdentity::from_fd(child.directory(child.path())?)?;
        if named != held {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "pending control directory changed before its receipt",
            ));
        }
    }
    Ok(())
}
fn install_bootstrap(
    root: &DiskRoot,
    directory: &Path,
    object: &PreparedMetadataObject,
) -> Result<()> {
    let objects = directory.join("objects");
    root.private_dir(&objects)?;
    for name in root.entries(&objects)? {
        if name.map_err(fs_error)?.as_os_str() != BOOTSTRAP_KIND {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "unexpected pending object namespace",
            ));
        }
    }
    let path = objects.join(BOOTSTRAP_KIND);
    root.private_dir(&path)?;
    let filename = format!("{}.json", object.hash);
    for name in root.entries(&path)? {
        let name = name.map_err(fs_error)?;
        if platform_io::fs_safety::AtomicFile::is_temporary_name(&name) {
            root.open_regular(&path.join(name.as_path()))?;
        } else if name.as_os_str() != filename.as_str() {
            return Err(Error::new(
                ErrorCode::ObjectCorrupt,
                "pending bootstrap belongs to another member",
            ));
        }
    }
    root.write_atomic(&path.join(filename), object.bytes(), false)
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/engine/membership_pending_tests.rs"]
mod tests;
