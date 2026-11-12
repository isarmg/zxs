//! Read-only, pre-initialization capture of the current SMB namespace.
//!
//! Capture and finish run under the caller's currently authorized Engine guard.
//! Only verification runs outside it. No type here grants repository write access,
//! accepts exclusions, or opens an initialized `.zxs` repository.
use crate::engine::{ContentReadPlan, Engine, RangeContent, ReadLease, ReadRegistry};
use crate::{
    BranchId, ContentHash, ContentState, Entry, EntryId, Error, FileId, NamespaceEntryKind,
    NamespaceSnapshot, RepositoryId, VersionId,
};
use name_policy::{LookupPath, PortablePath};
use std::{
    collections::BTreeMap,
    io::Read,
    sync::{Arc, LazyLock, Mutex, Weak},
    time::{Duration, Instant},
};
use storage_contract::history_window::HistoryWindowCancellation;

/// Hard ceilings apply even when a trusted caller requests smaller limits.
#[derive(Clone, Copy, Debug)]
pub struct DirectorySnapshotLimits {
    pub entries: usize,
    pub path_bytes: usize,
    pub manifest_bytes: usize,
    pub unique_versions: usize,
    pub route_references: usize,
    pub routes_per_version: usize,
    pub logical_bytes: u64,
    pub proof_bytes: usize,
    pub scanned_entries: usize,
    pub work_units: usize,
    pub duration: Duration,
}
impl Default for DirectorySnapshotLimits {
    fn default() -> Self {
        Self {
            entries: 2048,
            path_bytes: 1_048_576,
            manifest_bytes: 4_194_304,
            unique_versions: 512,
            route_references: 4096,
            routes_per_version: 8,
            logical_bytes: 67_108_864,
            proof_bytes: 4_194_304,
            scanned_entries: 32_768,
            work_units: 262_144,
            duration: Duration::from_secs(60),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirectorySnapshotBudget {
    Entries,
    PathBytes,
    ManifestBytes,
    UniqueVersions,
    RouteReferences,
    RoutesPerVersion,
    LogicalBytes,
    ProofBytes,
    ScannedEntries,
    WorkUnits,
    ConcurrentTasks,
}

#[derive(Debug, error_definition::Error)]
pub enum DirectorySnapshotSourceError {
    #[error("directory snapshot limits exceed the hard ceilings")]
    InvalidLimits,
    #[error("directory snapshot exceeds its {0:?} budget")]
    Budget(DirectorySnapshotBudget),
    #[error("directory snapshot source was cancelled")]
    Cancelled,
    #[error("directory snapshot source deadline expired")]
    Expired,
    #[error("directory snapshot source admission registry is unavailable")]
    AdmissionUnavailable,
    #[error("directory snapshot source directory is absent or is not a directory")]
    DirectoryUnavailable,
    #[error("directory snapshot source has conflicting management material at {0}")]
    ManagementConflict(PortablePath),
    #[error("directory snapshot source changed or is no longer accessible")]
    SourceChanged,
    #[error("directory snapshot source version {0} is unavailable or purged")]
    ContentUnavailable(VersionId),
    #[error("directory snapshot source content identity differs")]
    ContentIdentity,
    #[error("directory snapshot source payload failed verification")]
    PayloadMismatch,
    #[error("reading directory snapshot source payload: {0}")]
    Read(#[source] std::io::Error),
    #[error("admitting directory snapshot source identity: {0}")]
    Identity(#[source] storage_contract::Error),
    #[error("preparing directory snapshot source: {0}")]
    Source(#[source] Error),
}
type Result<T> = std::result::Result<T, DirectorySnapshotSourceError>;

#[derive(Debug)]
pub enum DirectorySnapshotVerificationError<E> {
    Source(DirectorySnapshotSourceError),
    Authorization(E),
}

#[derive(Clone, Copy, Debug)]
pub enum DirectorySnapshotContent {
    Directory,
    File {
        file_id: FileId,
        version_id: VersionId,
        size: u64,
        content_hash: ContentHash,
    },
}
#[derive(Clone, Debug)]
pub struct DirectorySnapshotEntry {
    pub path: PortablePath,
    pub entry_id: EntryId,
    pub content: DirectorySnapshotContent,
}
#[derive(Clone, Copy, Debug, Default)]
pub struct DirectorySnapshotUsage {
    pub entries: usize,
    pub path_bytes: usize,
    /// Upper bound for a JSON preview, including worst-case string escaping.
    /// This is not a persisted commit-manifest encoding or commit identity.
    pub manifest_bytes: usize,
    pub unique_versions: usize,
    pub route_references: usize,
    /// Counts every file occurrence, including aliases of the same version.
    pub logical_bytes: u64,
    pub proof_bytes: usize,
    pub scanned_entries: usize,
    pub work_units: usize,
}
#[derive(Debug)]
pub struct DirectorySnapshotPreview {
    pub repository_id: RepositoryId,
    pub branch_id: BranchId,
    pub root: Option<PortablePath>,
    pub entries: Vec<DirectorySnapshotEntry>,
    pub usage: DirectorySnapshotUsage,
}

struct Reservation {
    owner: Weak<Mutex<ReadRegistry>>,
}
// A single task per Engine is stricter than separate source/repository limits.
// Two tasks globally, each opening at most eight payload routes, bound this
// feature to sixteen simultaneous payload handles. Proof opens are sequential.
static ACTIVE: LazyLock<Mutex<Vec<Weak<Reservation>>>> = LazyLock::new(Mutex::default);

struct Progress {
    limits: DirectorySnapshotLimits,
    usage: DirectorySnapshotUsage,
    cancellation: HistoryWindowCancellation,
    deadline: Instant,
}
impl Progress {
    fn checkpoint(&self) -> Result<()> {
        if self.cancellation.is_cancelled() {
            return Err(DirectorySnapshotSourceError::Cancelled);
        }
        if Instant::now() >= self.deadline {
            return Err(DirectorySnapshotSourceError::Expired);
        }
        Ok(())
    }
    fn work(&mut self, amount: usize) -> Result<()> {
        self.checkpoint()?;
        charge(
            &mut self.usage.work_units,
            amount,
            self.limits.work_units,
            DirectorySnapshotBudget::WorkUnits,
        )
    }
}

struct SourceScope {
    repository: RepositoryId,
    branch: BranchId,
    root: Option<(EntryId, PortablePath)>,
    topology: u64,
}
struct FrozenVersion {
    size: u64,
    hash: ContentHash,
    plan: ContentReadPlan,
}

/// Owns the complete unique-version lease set, not a body-read authorization.
/// Dropping a plan releases all leases and the task slot. Cancellation and the
/// deadline are checked at bounded scan/read checkpoints. `cancel` consumes an
/// idle plan and immediately releases its entire lease set and task slot.
pub struct DirectorySnapshotSourcePlan {
    scope: SourceScope,
    entries: Vec<DirectorySnapshotEntry>,
    versions: BTreeMap<VersionId, FrozenVersion>,
    reservation: Arc<Reservation>,
    progress: Progress,
}

/// No public data access: current authorization and deletion checks must occur
/// after the lock-free I/O, before even a verified preview can be disclosed.
pub struct VerifiedDirectorySnapshotSource {
    scope: SourceScope,
    entries: Vec<DirectorySnapshotEntry>,
    leases: Vec<ReadLease>,
    reservation: Arc<Reservation>,
    progress: Progress,
}

impl Engine {
    /// The caller must authorize this branch under the same Engine guard.
    /// This never creates a repository and rejects every `.zxs` component.
    pub fn prepare_directory_snapshot_source(
        &self,
        branch: BranchId,
        root: Option<&LookupPath>,
        limits: DirectorySnapshotLimits,
        cancellation: HistoryWindowCancellation,
    ) -> Result<DirectorySnapshotSourcePlan> {
        validate_limits(limits)?;
        let mut progress = Progress {
            limits,
            usage: DirectorySnapshotUsage::default(),
            cancellation,
            deadline: Instant::now() + limits.duration,
        };
        progress.checkpoint()?;
        let reservation = reserve(self)?;
        let branch_key = branch.to_string();
        let snapshot = self
            .namespace_snapshot(&branch_key)
            .map_err(DirectorySnapshotSourceError::Source)?;
        let witness = snapshot.witness();
        let repository = self
            .repository_for_branch(&branch_key)
            .map_err(DirectorySnapshotSourceError::Source)?;
        if repository.main_branch_id != branch_key {
            return Err(DirectorySnapshotSourceError::SourceChanged);
        }
        let scope = SourceScope {
            repository: repository.repository_id.parse()
                .map_err(DirectorySnapshotSourceError::Identity)?,
            branch,
            root: capture_root(&snapshot, root)?,
            topology: self.state.topology_generation,
        };
        let entries = capture_entries(self, &snapshot, &scope, &mut progress)?;
        // Enumeration and the whole lease acquisition run in one immutable
        // Engine borrow. No publication may interleave these two phases.
        if !self.namespace_snapshot_matches(&witness) {
            return Err(DirectorySnapshotSourceError::SourceChanged);
        }
        let versions = retain_versions(self, &entries, &mut progress)?;
        progress.checkpoint()?;
        Ok(DirectorySnapshotSourcePlan {
            scope,
            entries,
            versions,
            reservation,
            progress,
        })
    }

    /// Reauthorize the current principal before this call, under this same guard.
    /// Normal saves/selection changes preserve the frozen version. Removed,
    /// moved, deleted, purged or withdrawn source occurrences fail as a whole.
    pub fn finish_directory_snapshot_preview(
        &self,
        mut verified: VerifiedDirectorySnapshotSource,
    ) -> Result<DirectorySnapshotPreview> {
        verified.progress.checkpoint()?;
        if !verified.reservation.owner.ptr_eq(&Arc::downgrade(&self.reads))
            || verified.scope.topology != self.state.topology_generation
        {
            return Err(DirectorySnapshotSourceError::SourceChanged);
        }
        let branch = verified.scope.branch.to_string();
        let repository = self.repository_for_branch(&branch)
            .map_err(DirectorySnapshotSourceError::Source)?;
        if repository.repository_id != verified.scope.repository.to_string()
            || repository.main_branch_id != branch
        {
            return Err(DirectorySnapshotSourceError::SourceChanged);
        }
        let snapshot = self.namespace_snapshot(&branch)
            .map_err(DirectorySnapshotSourceError::Source)?;
        recheck_occurrences(self, &snapshot, &mut verified)?;
        // Leases remain owned by verified until the full recheck has succeeded.
        drop(verified.leases);
        Ok(DirectorySnapshotPreview {
            repository_id: verified.scope.repository,
            branch_id: verified.scope.branch,
            root: verified.scope.root.map(|(_, path)| path),
            entries: verified.entries,
            usage: verified.progress.usage,
        })
    }
}

fn validate_limits(limits: DirectorySnapshotLimits) -> Result<()> {
    let max = DirectorySnapshotLimits::default();
    if limits.entries > max.entries
        || limits.path_bytes > max.path_bytes
        || limits.manifest_bytes > max.manifest_bytes
        || limits.unique_versions > max.unique_versions
        || limits.route_references > max.route_references
        || limits.routes_per_version > max.routes_per_version
        || limits.logical_bytes > max.logical_bytes
        || limits.proof_bytes > max.proof_bytes
        || limits.scanned_entries > max.scanned_entries
        || limits.work_units > max.work_units
        || limits.duration > max.duration
    {
        return Err(DirectorySnapshotSourceError::InvalidLimits);
    }
    Ok(())
}
fn reserve(engine: &Engine) -> Result<Arc<Reservation>> {
    let mut active = ACTIVE.lock()
        .map_err(|_| DirectorySnapshotSourceError::AdmissionUnavailable)?;
    active.retain(|reservation| reservation.strong_count() != 0);
    let owner = Arc::downgrade(&engine.reads);
    if active.len() >= 2
        || active.iter().filter_map(Weak::upgrade).any(|held| held.owner.ptr_eq(&owner))
    {
        return Err(DirectorySnapshotSourceError::Budget(
            DirectorySnapshotBudget::ConcurrentTasks,
        ));
    }
    let reservation = Arc::new(Reservation { owner });
    active.push(Arc::downgrade(&reservation));
    Ok(reservation)
}
fn capture_root(
    snapshot: &NamespaceSnapshot,
    root: Option<&LookupPath>,
) -> Result<Option<(EntryId, PortablePath)>> {
    let Some(root) = root else {
        return Ok(None);
    };
    let entry = snapshot.tree().get_key(root.key())
        .filter(|entry| entry.kind == NamespaceEntryKind::Directory)
        .ok_or(DirectorySnapshotSourceError::DirectoryUnavailable)?;
    reject_management(&entry.path)?;
    Ok(Some((
        entry.entry_id.parse().map_err(DirectorySnapshotSourceError::Identity)?,
        entry.path.clone(),
    )))
}
fn reject_management(path: &PortablePath) -> Result<()> {
    if path.key().split('/').any(|component| component == ".zxs") {
        return Err(DirectorySnapshotSourceError::ManagementConflict(path.clone()));
    }
    Ok(())
}
fn relative_path(entry: &Entry, scope: &SourceScope) -> Option<PortablePath> {
    match &scope.root {
        None => Some(entry.path.clone()),
        Some((_, root)) => {
            let suffix = entry.lookup_key().strip_prefix(root.key())?;
            if !suffix.starts_with('/') {
                return None;
            }
            // This keeps the frozen display suffix verbatim, not its lookup key.
            entry.path.strip_components(root.as_str().split('/').count())
        }
    }
}
fn capture_entries(
    engine: &Engine,
    snapshot: &NamespaceSnapshot,
    scope: &SourceScope,
    progress: &mut Progress,
) -> Result<Vec<DirectorySnapshotEntry>> {
    charge(
        &mut progress.usage.scanned_entries,
        snapshot.entries().len(),
        progress.limits.scanned_entries,
        DirectorySnapshotBudget::ScannedEntries,
    )?;
    charge(
        &mut progress.usage.manifest_bytes,
        1024 + scope.root.as_ref().map_or(0, |(_, path)| 6 * path.as_str().len()),
        progress.limits.manifest_bytes,
        DirectorySnapshotBudget::ManifestBytes,
    )?;
    let tree = snapshot.tree();
    let mut entries = Vec::new();
    for (index, entry) in snapshot.entries().iter().enumerate() {
        progress.work(1 + entry.path.as_str().split('/').count())?;
        let Some(path) = relative_path(entry, scope) else {
            continue;
        };
        if tree.visible_entry(index).is_none() {
            continue;
        }
        // This only inspects the visible namespace, not physical initialization
        // safety. Unknown visible management trees are never silently excluded.
        reject_management(&path)?;
        charge(&mut progress.usage.entries, 1, progress.limits.entries,
            DirectorySnapshotBudget::Entries)?;
        charge(&mut progress.usage.path_bytes, path.as_str().len(),
            progress.limits.path_bytes, DirectorySnapshotBudget::PathBytes)?;
        charge(&mut progress.usage.manifest_bytes, 512 + 6 * path.as_str().len(),
            progress.limits.manifest_bytes, DirectorySnapshotBudget::ManifestBytes)?;
        let content = match entry.kind {
            NamespaceEntryKind::Directory => DirectorySnapshotContent::Directory,
            NamespaceEntryKind::File => {
                let version_id = entry.version_id.parse()
                    .map_err(DirectorySnapshotSourceError::Identity)?;
                let file_id = entry.file_id.parse()
                    .map_err(DirectorySnapshotSourceError::Identity)?;
                let (version, disposition) = engine.version_metadata(version_id)
                    .ok_or(DirectorySnapshotSourceError::ContentUnavailable(version_id))?;
                if disposition.is_some_and(|state| state != ContentState::Available) {
                    return Err(DirectorySnapshotSourceError::ContentUnavailable(version_id));
                }
                engine.repository_for_history(&version.history_tree_id)
                    .map_err(DirectorySnapshotSourceError::Source)?;
                if engine.file_deletion(&version.file_id).is_some() {
                    return Err(DirectorySnapshotSourceError::SourceChanged);
                }
                if version.logical_size != entry.size {
                    return Err(DirectorySnapshotSourceError::ContentIdentity);
                }
                charge_body(progress, entry.size)?;
                DirectorySnapshotContent::File {
                    file_id,
                    version_id,
                    size: entry.size,
                    content_hash: version.content_hash,
                }
            }
        };
        entries.push(DirectorySnapshotEntry {
            path,
            entry_id: entry.entry_id.parse()
                .map_err(DirectorySnapshotSourceError::Identity)?,
            content,
        });
    }
    Ok(entries)
}
fn retain_versions(
    engine: &Engine,
    entries: &[DirectorySnapshotEntry],
    progress: &mut Progress,
) -> Result<BTreeMap<VersionId, FrozenVersion>> {
    let mut versions = BTreeMap::new();
    for entry in entries {
        progress.work(1)?;
        let DirectorySnapshotContent::File { version_id, size, content_hash, .. } =
            entry.content else {
                continue;
            };
        if versions.contains_key(&version_id) {
            continue;
        }
        charge(&mut progress.usage.unique_versions, 1, progress.limits.unique_versions,
            DirectorySnapshotBudget::UniqueVersions)?;
        let key = version_id.to_string();
        let placement = engine.state.placements.get(&key)
            .ok_or(DirectorySnapshotSourceError::SourceChanged)?;
        let routes = placement.route_slot_ids.len();
        if routes > progress.limits.routes_per_version {
            return Err(DirectorySnapshotSourceError::Budget(
                DirectorySnapshotBudget::RoutesPerVersion,
            ));
        }
        charge(&mut progress.usage.route_references, routes, progress.limits.route_references,
            DirectorySnapshotBudget::RouteReferences)?;
        progress.work(routes)?;
        let plan = engine.prepare_content(&key).map_err(DirectorySnapshotSourceError::Source)?;
        let proof_bytes = plan.checksum_manifest_bytes()
            .map_err(DirectorySnapshotSourceError::Source)?;
        charge(&mut progress.usage.proof_bytes, proof_bytes, progress.limits.proof_bytes,
            DirectorySnapshotBudget::ProofBytes)?;
        versions.insert(version_id, FrozenVersion {
            size,
            hash: content_hash,
            plan,
        });
    }
    Ok(versions)
}

impl DirectorySnapshotSourcePlan {
    /// Cancel an idle plan without retaining any version protection or task slot.
    pub fn cancel(self) {}

    /// Verify outside the Engine lock. The consumer reauthorizes each frozen
    /// version before opening it and before every bounded body read. Neither
    /// partial bytes nor a partial preview escape on cancellation or revocation.
    pub fn verify<E>(
        mut self,
        mut authorize: impl FnMut(VersionId) -> std::result::Result<(), E>,
    ) -> std::result::Result<VerifiedDirectorySnapshotSource,
        DirectorySnapshotVerificationError<E>>
    {
        use DirectorySnapshotVerificationError::{Authorization, Source};
        let mut leases = Vec::with_capacity(self.versions.len());
        for (identity, version) in self.versions {
            self.progress.work(1).map_err(Source)?;
            authorize(identity).map_err(Authorization)?;
            let content = version.plan.open()
                .map_err(|error| Source(DirectorySnapshotSourceError::Source(error)))?;
            if content.size != version.size || content.content_hash != version.hash {
                return Err(Source(DirectorySnapshotSourceError::ContentIdentity));
            }
            let lease = verify_content(content, &mut self.progress, || authorize(identity))?;
            leases.push(lease);
        }
        self.progress.checkpoint().map_err(Source)?;
        Ok(VerifiedDirectorySnapshotSource {
            scope: self.scope,
            entries: self.entries,
            leases,
            reservation: self.reservation,
            progress: self.progress,
        })
    }
}
fn verify_content<E>(
    mut content: RangeContent,
    progress: &mut Progress,
    mut authorize: impl FnMut() -> std::result::Result<(), E>,
) -> std::result::Result<ReadLease, DirectorySnapshotVerificationError<E>> {
    use DirectorySnapshotVerificationError::{Authorization, Source};
    let mut complete = blake3::Hasher::new();
    let mut remaining = content.size;
    let mut buffer = [0u8; 65_536];
    for expected in &content.manifest.chunks {
        let mut chunk = blake3::Hasher::new();
        let mut chunk_left = remaining.min(u64::from(content.manifest.chunk_bytes));
        while chunk_left != 0 {
            progress.work(1).map_err(Source)?;
            authorize().map_err(Authorization)?;
            content.file.check_running()
                .map_err(|error| Source(DirectorySnapshotSourceError::Source(error)))?;
            let length = chunk_left.min(buffer.len() as u64) as usize;
            content.file.read_exact(&mut buffer[..length])
                .map_err(|error| Source(DirectorySnapshotSourceError::Read(error)))?;
            chunk.update(&buffer[..length]);
            complete.update(&buffer[..length]);
            chunk_left -= length as u64;
            remaining -= length as u64;
        }
        if chunk.finalize().as_bytes() != expected {
            return Err(Source(DirectorySnapshotSourceError::PayloadMismatch));
        }
    }
    progress.checkpoint().map_err(Source)?;
    authorize().map_err(Authorization)?;
    let tail = content.file.read(&mut [0; 1])
        .map_err(|error| Source(DirectorySnapshotSourceError::Read(error)))?;
    let size = content.file.metadata()
        .map_err(|error| Source(DirectorySnapshotSourceError::Read(error)))?.len();
    if remaining != 0 || tail != 0 || size != content.size
        || ContentHash::from_digest(complete.finalize()) != content.content_hash
    {
        return Err(Source(DirectorySnapshotSourceError::PayloadMismatch));
    }
    Ok(content.lease)
}
fn recheck_occurrences(
    engine: &Engine,
    snapshot: &NamespaceSnapshot,
    verified: &mut VerifiedDirectorySnapshotSource,
) -> Result<()> {
    let tree = snapshot.tree();
    if let Some((identity, path)) = &verified.scope.root {
        let current = tree.get_key(path.key())
            .ok_or(DirectorySnapshotSourceError::SourceChanged)?;
        if current.kind != NamespaceEntryKind::Directory
            || current.entry_id != identity.to_string()
        {
            return Err(DirectorySnapshotSourceError::SourceChanged);
        }
    }
    for entry in &verified.entries {
        verified.progress.work(1)?;
        let original = match &verified.scope.root {
            None => entry.path.clone(),
            Some((_, root)) => entry.path.rebase(0, root)
                .map_err(|_| DirectorySnapshotSourceError::SourceChanged)?,
        };
        let current = tree.get_key(original.key())
            .ok_or(DirectorySnapshotSourceError::SourceChanged)?;
        if current.entry_id != entry.entry_id.to_string() {
            return Err(DirectorySnapshotSourceError::SourceChanged);
        }
        match entry.content {
            DirectorySnapshotContent::Directory => {
                if current.kind != NamespaceEntryKind::Directory {
                    return Err(DirectorySnapshotSourceError::SourceChanged);
                }
            }
            DirectorySnapshotContent::File { file_id, version_id, size, content_hash } => {
                let (version, disposition) = engine.version_metadata(version_id)
                    .ok_or(DirectorySnapshotSourceError::SourceChanged)?;
                if current.kind != NamespaceEntryKind::File
                    || current.file_id != file_id.to_string()
                    || engine.file_deletion(&current.file_id).is_some()
                    || disposition.is_some_and(|state| state != ContentState::Available)
                    || engine.file_deletion(&version.file_id).is_some()
                    || version.logical_size != size
                    || version.content_hash != content_hash
                {
                    return Err(DirectorySnapshotSourceError::SourceChanged);
                }
                engine.repository_for_history(&version.history_tree_id)
                    .map_err(DirectorySnapshotSourceError::Source)?;
            }
        }
    }
    verified.progress.checkpoint()
}
fn charge(value: &mut usize, amount: usize, maximum: usize, kind: DirectorySnapshotBudget)
    -> Result<()>
{
    if amount > maximum.saturating_sub(*value) {
        return Err(DirectorySnapshotSourceError::Budget(kind));
    }
    *value += amount;
    Ok(())
}
fn charge_body(progress: &mut Progress, amount: u64) -> Result<()> {
    if amount > progress.limits.logical_bytes.saturating_sub(progress.usage.logical_bytes) {
        return Err(DirectorySnapshotSourceError::Budget(DirectorySnapshotBudget::LogicalBytes));
    }
    progress.usage.logical_bytes += amount;
    Ok(())
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/engine/directory_snapshot_source_tests.rs"]
mod tests;
