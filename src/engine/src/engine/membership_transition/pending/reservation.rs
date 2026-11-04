//! An atomic directory name is the first durable target binding. It contains the
//! hash of the exact preparation, not a partially written JSON marker. Therefore
//! a crash before the writer lock or pending seal never leaves anonymous debris.
use super::*;
use std::collections::BTreeSet;

const PREFIX: &str = ".membership-reservation-";

#[derive(Serialize)]
struct Preparation<'a> {
    origin: &'a Origin,
    target: &'a ValidatedConfig,
    principal: &'a str,
    operation_id: OperationId,
    identity: &'a MemberIdentity,
}

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct SourcePreparation {
    origin: Origin,
    target: ValidatedConfig,
    principal: String,
    operation_id: OperationId,
    added_roots: BTreeMap<String, MemberIdentity>,
}
impl SourcePreparation {
    fn path(&self, disk: &DiskConfig) -> PathBuf {
        source_path(disk, self.operation_id)
    }
}
fn source_path(disk: &DiskConfig, operation: OperationId) -> PathBuf {
    disk.root
        .join("metadata/membership-preparations")
        .join(format!("{operation}.json"))
}

/// Initializers use the same directory-before-writer lock order as preparation.
/// The caller retains these locks through every initialization write, including
/// journal/index creation, rather than releasing them after the first scan.
pub(in crate::engine) fn lock_initialization_directories(
    disks: &DiskResources,
) -> Result<Vec<File>> {
    let mut locks = Vec::new();
    for disk in disks.handles() {
        let directory = disk.root.directory(disk.root.path())?;
        lock(&directory)?;
        admission::verify_named_root(&disk.root)?;
        locks.push(directory);
    }
    Ok(locks)
}

pub(in crate::engine) fn reject_reserved_initialization(root: &DiskRoot) -> Result<()> {
    for entry in root.entries(root.path())? {
        if entry
            .map_err(fs_error)?
            .as_os_str()
            .as_encoded_bytes()
            .starts_with(PREFIX.as_bytes())
        {
            return Err(Error::new(
                ErrorCode::DiskAlreadyEnrolled,
                "storage directory is reserved for a membership addition",
            ));
        }
    }
    Ok(())
}

pub(super) fn name(
    origin: &Origin,
    target: &ValidatedConfig,
    principal: &str,
    operation_id: OperationId,
    identity: &MemberIdentity,
) -> Result<String> {
    Ok(format!(
        "{PREFIX}{}",
        object_hash(
            "membership-root-preparation",
            &Preparation {
                origin,
                target,
                principal,
                operation_id,
                identity
            }
        )?
    ))
}

pub(super) fn verify_marker(root: &DiskRoot, name: &str) -> Result<()> {
    let path = root.path().join(name);
    let marker = root.private_child(&path)?;
    if marker
        .entries(&path)?
        .next()
        .transpose()
        .map_err(fs_error)?
        .is_some()
    {
        return Err(Error::new(
            ErrorCode::UnsafePath,
            "membership reservation contains unrelated material",
        ));
    }
    Ok(())
}

pub(super) fn lock(file: &File) -> Result<()> {
    rustix::fs::flock(file, rustix::fs::FlockOperation::NonBlockingLockExclusive).map_err(
        |error| {
            Error::from(std::io::Error::from(error)).context(
                ErrorCode::WriterAlreadyActive,
                "another initializer or membership preparation owns this directory",
            )
        },
    )?;
    Ok(())
}

pub(super) fn verify_writer(root: &DiskRoot, held: &crate::disk::FenceOwner) -> Result<()> {
    let named = root.open_regular(&root.path().join(".writer.lock"))?;
    if FileIdentity::from_fd(&named)? != held.writer_identity()? || named.metadata()?.len() != 0 {
        return Err(Error::new(
            ErrorCode::DiskIdentityMismatch,
            "membership writer lock was replaced or contains unrelated bytes",
        ));
    }
    Ok(())
}

struct InspectedRoot {
    disk_id: String,
    root: DiskRoot,
    directory_lock: File,
    identity: MemberIdentity,
    name: String,
}

#[derive(Clone)]
pub(in crate::engine::membership_transition) struct ReservedRoot {
    pub(super) root: Arc<DiskRoot>,
    pub(in crate::engine::membership_transition) identity: MemberIdentity,
    pub(super) name: String,
    pub(super) writer: Arc<crate::disk::FenceOwner>,
}
impl ReservedRoot {
    pub(in crate::engine::membership_transition) fn verify_unsealed(&self) -> Result<()> {
        admission::verify_named_root(&self.root)?;
        verify_writer(&self.root, &self.writer)?;
        inspect_unsealed(&self.root, &self.name, false)
    }
}

fn inspect_unsealed(root: &DiskRoot, name: &str, empty_allowed: bool) -> Result<()> {
    let entries = root
        .entries(root.path())?
        .collect::<std::result::Result<Vec<_>, _>>()
        .map_err(fs_error)?;
    if entries.is_empty() && empty_allowed {
        return Ok(());
    }
    verify_marker(root, name)?;
    for entry in entries {
        match entry.as_os_str().to_str() {
            Some(value) if value == name => (),
            Some(".writer.lock") => {
                if root
                    .open_regular(&root.path().join(".writer.lock"))?
                    .metadata()?
                    .len()
                    != 0
                {
                    return Err(Error::new(
                        ErrorCode::DiskAlreadyEnrolled,
                        "reserved root contains unrelated lock bytes",
                    ));
                }
            }
            _ => {
                return Err(Error::new(
                    ErrorCode::DiskAlreadyEnrolled,
                    "new directory is not empty or reserved by this exact operation",
                ));
            }
        }
    }
    Ok(())
}

impl Engine {
    pub(in crate::engine::membership_transition) fn admit_added_directories(
        &self,
        target: &ValidatedConfig,
        directories: Vec<PendingMemberDirectory>,
    ) -> Result<BTreeMap<String, DiskRoot>> {
        let added = target
            .disks
            .iter()
            .filter(|disk| {
                !self
                    .config
                    .disks
                    .iter()
                    .any(|old| old.disk_id == disk.disk_id)
            })
            .map(|disk| (disk.disk_id.as_str(), disk))
            .collect::<BTreeMap<_, _>>();
        if directories.len() != added.len() {
            return Err(Error::new(
                ErrorCode::ConfigConflict,
                "caller must hold every added directory exactly once",
            ));
        }
        let mut held = BTreeMap::new();
        for selected in directories {
            let disk = added.get(selected.disk_id.as_str()).ok_or_else(|| {
                Error::new(
                    ErrorCode::ConfigConflict,
                    "held directory is not an added member",
                )
            })?;
            let root = DiskRoot::from_directory(disk.root.clone(), selected.directory)?;
            if held.insert(selected.disk_id, root).is_some() {
                return Err(Error::new(
                    ErrorCode::ConfigConflict,
                    "duplicate held added directory",
                ));
            }
        }
        Ok(held)
    }

    pub(in crate::engine::membership_transition) fn prepare_added_roots(
        &mut self,
        origin: &Origin,
        target: &ValidatedConfig,
        principal: &str,
        operation_id: OperationId,
        mut held: Option<BTreeMap<String, DiskRoot>>,
    ) -> Result<BTreeMap<String, ReservedRoot>> {
        if !target.disks.iter().any(|disk| {
            !origin
                .config()
                .disks
                .iter()
                .any(|old| old.disk_id == disk.disk_id)
        }) {
            return Ok(BTreeMap::new());
        }
        let preparations = self.read_source_preparations()?;
        let saved = preparations.get(&operation_id);
        // Compare the complete request independently of the later frozen prefix.
        if let Some(saved) = saved
            && (saved.operation_id != operation_id
                || saved.principal != principal
                || object_hash("membership-preparation-origin", &saved.origin)?
                    != object_hash("membership-preparation-origin", origin)?
                || object_hash("membership-target", &saved.target)?
                    != object_hash("membership-target", target)?)
        {
            return Err(Error::new(
                ErrorCode::IdempotencyKeyReused,
                "membership operation already reserves another source, actor or target",
            ));
        }
        let mut identities = BTreeSet::new();
        for handle in self.disk_resources.handles() {
            let identity = admission::verify_named_root(&handle.root)?;
            identities.insert((identity.device, identity.inode));
        }
        let system = DiskRoot::open(&self.root)?;
        let system_id = FileIdentity::from_fd(system.directory(system.path())?)?;
        identities.insert((system_id.device, system_id.inode));
        let mut roots = Vec::new();
        for disk in target.disks.iter().filter(|disk| {
            !origin
                .config()
                .disks
                .iter()
                .any(|old| old.disk_id == disk.disk_id)
        }) {
            let root = match &mut held {
                Some(held) => held
                    .remove(&disk.disk_id)
                    .expect("invariant: caller admission covered every added directory"),
                None => DiskRoot::open(&disk.root)?,
            };
            let directory_lock = root.directory(root.path())?;
            lock(&directory_lock)?;
            let physical = admission::verify_named_root(&root)?;
            if !identities.insert((physical.device, physical.inode)) {
                return Err(Error::new(
                    ErrorCode::DiskIdentityMismatch,
                    "added directory aliases another held root",
                ));
            }
            let identity = admission::member_identity(target, disk, physical);
            if saved.is_some_and(|saved| saved.added_roots.get(&disk.disk_id) != Some(&identity)) {
                return Err(Error::new(
                    ErrorCode::DiskIdentityMismatch,
                    "prepared operation cannot replace its original held directory",
                ));
            }
            if preparations.values().any(|other| {
                other.operation_id != operation_id
                    && other.added_roots.values().any(|reserved| {
                        reserved.device == identity.device && reserved.inode == identity.inode
                    })
            }) {
                return Err(Error::new(
                    ErrorCode::IdempotencyKeyReused,
                    "another source operation already reserved this directory",
                ));
            }
            let name = name(origin, target, principal, operation_id, &identity)?;
            inspect_unsealed(&root, &name, true)?;
            roots.push(InspectedRoot {
                disk_id: disk.disk_id.clone(),
                root,
                directory_lock,
                identity,
                name,
            });
        }
        let source = SourcePreparation {
            origin: origin.clone(),
            target: target.clone(),
            principal: principal.to_owned(),
            operation_id,
            added_roots: roots
                .iter()
                .map(|root| (root.disk_id.clone(), root.identity.clone()))
                .collect(),
        };
        if let Some(saved) = saved
            && object_hash("membership-source-preparation", saved)?
                != object_hash("membership-source-preparation", &source)?
        {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "source preparation identity set differs",
            ));
        }
        self.persist_source_preparation(&source)?;
        self.reserve_target_roots(roots)
    }

    fn reserve_target_roots(
        &mut self,
        roots: Vec<InspectedRoot>,
    ) -> Result<BTreeMap<String, ReservedRoot>> {
        // All source preparations are durable before the first target mutation.
        // Directory locks serialize preparation peers; the ordinary writer lock
        // below also arbitrates initialization, which rechecks after locking.
        let mut prepared = BTreeMap::new();
        for (
            index,
            InspectedRoot {
                disk_id,
                root,
                directory_lock,
                identity,
                name,
            },
        ) in roots.into_iter().enumerate()
        {
            #[cfg(feature = "fault-injection")]
            self.failpoint(&format!("before_membership_reservation_{}", index + 1))?;
            admission::verify_named_root(&root)?;
            inspect_unsealed(&root, &name, true)?;
            match rustix::fs::mkdirat(
                &directory_lock,
                name.as_str(),
                rustix::fs::Mode::from_raw_mode(0o700),
            ) {
                Ok(()) | Err(rustix::io::Errno::EXIST) => (),
                Err(error) => return Err(std::io::Error::from(error).into()),
            }
            #[cfg(feature = "fault-injection")]
            self.failpoint(&format!(
                "after_membership_reservation_create_{}",
                index + 1
            ))?;
            verify_marker(&root, &name)?;
            root.sync_dir(&root.path().join(&name))?;
            #[cfg(feature = "fault-injection")]
            self.failpoint(&format!(
                "after_membership_reservation_directory_sync_{}",
                index + 1
            ))?;
            directory_lock.sync_all()?;
            #[cfg(feature = "fault-injection")]
            self.failpoint(&format!("after_membership_reservation_sync_{}", index + 1))?;
            let writer = root.lock_file()?;
            platform_io::fs_safety::linux::SingleLinkRequirement::check(&writer)
                .map_err(fs_error)?;
            #[cfg(feature = "fault-injection")]
            self.failpoint(&format!("after_membership_writer_create_{}", index + 1))?;
            lock(&writer)?;
            #[cfg(feature = "fault-injection")]
            self.failpoint(&format!("after_membership_writer_lock_{}", index + 1))?;
            writer.sync_all()?;
            #[cfg(feature = "fault-injection")]
            self.failpoint(&format!("after_membership_writer_file_sync_{}", index + 1))?;
            directory_lock.sync_all()?;
            let reserved = ReservedRoot {
                root: Arc::new(root),
                identity,
                name,
                writer: crate::disk::FenceOwner::new(writer, Some(directory_lock)),
            };
            reserved.verify_unsealed()?;
            prepared.insert(disk_id, reserved);
            #[cfg(feature = "fault-injection")]
            self.failpoint(&format!("after_membership_writer_sync_{}", index + 1))?;
            #[cfg(not(feature = "fault-injection"))]
            let _ = index;
        }
        Ok(prepared)
    }
    fn read_source_preparations(&self) -> Result<BTreeMap<OperationId, SourcePreparation>> {
        let mut saved: BTreeMap<OperationId, SourcePreparation> = BTreeMap::new();
        for disk in self.config.metadata_members() {
            let root = &self.disk_resources.get(disk)?.root;
            let directory = disk.root.join("metadata/membership-preparations");
            if root.optional_metadata(&directory)?.is_none() {
                continue;
            }
            for entry in root.entries(&directory)? {
                let entry = entry.map_err(fs_error)?;
                let path = directory.join(entry.as_path());
                if platform_io::fs_safety::AtomicFile::is_temporary_name(&entry) {
                    // No target mutation precedes complete synchronized source
                    // records. Interrupted staging is preserved, never authority.
                    root.open_regular(&path)?;
                    continue;
                }
                let current: SourcePreparation =
                    decode_strict(&root.read_bytes(&path, MAX_METADATA)?)?;
                if source_path(disk, current.operation_id) != path {
                    return Err(Error::new(
                        ErrorCode::ObjectCorrupt,
                        "source preparation name differs from its operation",
                    ));
                }
                match saved.get(&current.operation_id) {
                    Some(saved)
                        if object_hash("membership-source-preparation", saved)?
                            != object_hash("membership-source-preparation", &current)? =>
                    {
                        return Err(Error::new(
                            ErrorCode::LogFork,
                            "old members have conflicting source preparations",
                        ));
                    }
                    Some(_) => (),
                    None => {
                        saved.insert(current.operation_id, current);
                    }
                }
            }
        }
        Ok(saved)
    }

    fn persist_source_preparation(&mut self, source: &SourcePreparation) -> Result<()> {
        for (index, disk) in self.members().iter().enumerate() {
            let root = &self.disk_resources.get(disk)?.root;
            root.write_json(&source.path(disk), source, false)?;
            #[cfg(feature = "fault-injection")]
            self.failpoint(&format!(
                "after_membership_source_preparation_{}",
                index + 1
            ))?;
            #[cfg(not(feature = "fault-injection"))]
            let _ = index;
        }
        Ok(())
    }
}

pub(in crate::engine::membership_transition) fn verify_intent_roots(
    intent: &TransitionIntent,
) -> Result<()> {
    intent.verify_fence_scope()?;
    let additions = intent
        .target
        .disks
        .iter()
        .filter(|disk| {
            !intent
                .origin
                .config()
                .disks
                .iter()
                .any(|old| old.disk_id == disk.disk_id)
        })
        .collect::<Vec<_>>();
    if additions.len() != intent.added_roots.len() {
        return Err(Error::new(
            ErrorCode::DiskIdentityMismatch,
            "transition lacks the exact prepared root set",
        ));
    }
    let mut physical = BTreeSet::new();
    for disk in additions {
        let identity = intent.added_roots.get(&disk.disk_id).ok_or_else(|| {
            Error::new(
                ErrorCode::DiskIdentityMismatch,
                "transition omits an added root identity",
            )
        })?;
        if identity.pool_id != intent.target.pool_id
            || identity.disk_id != disk.disk_id
            || identity.slot_id != disk.slot_id
            || identity.binding_generation != disk.binding_generation
            || identity.simulation != (intent.target.mode == Mode::Development)
            || !physical.insert((identity.device, identity.inode))
        {
            return Err(Error::new(
                ErrorCode::DiskIdentityMismatch,
                "transition root identity differs from its target",
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/engine/membership_reservation_tests.rs"]
mod tests;
