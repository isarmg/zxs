//! Decided, inode-bound version leaf renames. A retained intent locates bytes;
//! it does not authorize publication or replace the caller's writer fence.
use crate::container_storage::version_names::{VersionNameError, check_target_name};
use crate::container_storage::{installed_version_leaf, verify_body};
use crate::disk::DiskRoot;
use crate::{Error, FileVersion, ReadFile, Result, State};
use data_serialization::{Deserialize, Serialize};
use name_policy::Name;
use platform_io::fs_safety::linux::FileIdentity;
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::os::unix::fs::MetadataExt;
use storage_contract::{FileId, HistoryTreeId, Sequence, VersionId, VersionStorageId};

const MAX_BYTES: usize = 4 * 1024 * 1024;
const MAX_VERSIONS: usize = 65_536;

fn invalid() -> Error {
    Error::VersionNames(VersionNameError::Intent)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Body {
    version: VersionId,
    storage: VersionStorageId,
    #[serialization(with = "storage_contract::u64_string")]
    device: u64,
    #[serialization(with = "storage_contract::u64_string")]
    inode: u64,
    #[serialization(with = "storage_contract::u64_string")]
    size: u64,
    modified_seconds: i64,
    modified_nanoseconds: i64,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Intent {
    file: FileId,
    history: HistoryTreeId,
    sequence: Sequence,
    source_name: Name,
    target_name: Name,
    #[serialization(with = "storage_contract::u64_string")]
    directory_device: u64,
    #[serialization(with = "storage_contract::u64_string")]
    directory_inode: u64,
    bodies: Vec<Body>,
}

fn intent_path(container: &Path) -> PathBuf {
    container.join(crate::persist::layout::VERSION_NAMES_INTENT.as_path())
}

fn read(root: &DiskRoot, container: &Path) -> Result<Option<(Intent, Vec<u8>)>> {
    let path = intent_path(container);
    if root.optional_metadata(&path)?.is_none() {
        return Ok(None);
    }
    let bytes = root.read_bytes(&path, MAX_BYTES)?;
    let intent: Intent = storage_contract::decode_strict(&bytes)?;
    if intent.bodies.len() > MAX_VERSIONS || intent.source_name == intent.target_name {
        return Err(invalid());
    }
    let mut identities = BTreeSet::new();
    let mut storage = BTreeSet::new();
    for body in &intent.bodies {
        if !identities.insert(body.version) || !storage.insert(body.storage) {
            return Err(invalid());
        }
    }
    check_directory(root, container, &intent)?;
    let basename = container.file_name().and_then(|name| name.to_str()).ok_or_else(invalid)?;
    if basename != intent.source_name.as_str() && basename != intent.target_name.as_str() {
        return Err(invalid());
    }
    Ok(Some((intent, bytes)))
}

fn check_directory(root: &DiskRoot, container: &Path, intent: &Intent) -> Result<()> {
    let identity = root.directory_identity(container)?;
    if identity.device != intent.directory_device || identity.inode != intent.directory_inode {
        return Err(invalid());
    }
    Ok(())
}

fn paths(container: &Path, intent: &Intent, body: &Body) -> Result<(PathBuf, PathBuf)> {
    let versions = container.join(crate::persist::layout::VERSIONS.as_path());
    Ok((
        versions.join(installed_version_leaf(body.storage, &intent.source_name)?.as_path()),
        versions.join(installed_version_leaf(body.storage, &intent.target_name)?.as_path()),
    ))
}

fn position(root: &DiskRoot, container: &Path, intent: &Intent, body: &Body) -> Result<(PathBuf, bool)> {
    check_directory(root, container, intent)?;
    let (source, target) = paths(container, intent, body)?;
    let source_present = root.optional_metadata(&source)?.is_some();
    let target_present = root.optional_metadata(&target)?.is_some();
    let (path, complete) = match (source_present, target_present) {
        (true, false) => (source, false),
        (false, true) => (target, true),
        (true, true) | (false, false) => return Err(invalid()),
    };
    let held = root.open_read_regular(&path)?;
    let metadata = held.metadata()?;
    let identity = FileIdentity::from_metadata(&metadata);
    if identity.device != body.device || identity.inode != body.inode
        || metadata.len() != body.size || metadata.mtime() != body.modified_seconds
        || metadata.mtime_nsec() != body.modified_nanoseconds
    {
        return Err(invalid());
    }
    Ok((path, complete))
}

struct HeldBody {
    file: ReadFile,
    stamp: crate::file_stamp::FileStamp,
    // Fields drop in order: close the descriptor before returning its lease.
    _reservation: crate::engine::PayloadProofReservation,
}

fn hold_body(root: &DiskRoot, path: &Path, version: &FileVersion) -> Result<HeldBody> {
    let reservation = crate::engine::payload_proof_budget().reserve(1)
        .ok_or(Error::VersionNames(VersionNameError::Descriptors))?;
    let file = verify_body(root, path, version)?;
    let stamp = crate::file_stamp::FileStamp::from_metadata(&file.metadata()?);
    Ok(HeldBody { file, stamp, _reservation: reservation })
}

/// In-process evidence from the actual rename syscall, never disk authority.
pub(super) struct AppliedVersionRename {
    before: crate::file_stamp::FileStamp,
    after: crate::file_stamp::FileStamp,
}
impl AppliedVersionRename {
    pub(super) fn matches(
        &self,
        before: &crate::file_stamp::FileStamp,
        after: &crate::file_stamp::FileStamp,
    ) -> bool {
        &self.before == before && &self.after == after
    }
}

pub(super) struct Prepared {
    intent: Intent,
    original: Option<Vec<u8>>,
    held: Vec<HeldBody>,
}

impl Prepared {
    /// Atomic replacement keeps the previous intent until the new inode is durable.
    pub(super) fn reservation(&self) -> Result<(u64, usize)> {
        let bytes = crate::disk::encode_metadata(&self.intent, MAX_BYTES)?;
        let retained = self.original.as_ref().map_or(0, |old| old.len() as u64);
        Ok((bytes.len() as u64 + retained, 2))
    }

    /// Preparation verifies and flushes existing bodies but does not change names.
    /// Intent persistence belongs only to the decided tail.
    pub(super) fn capture(
        root: &DiskRoot,
        container: &Path,
        state: &State,
        file: &str,
        history: &str,
        target_name: &Name,
    ) -> Result<Option<Self>> {
        let file_id: FileId = file.parse()?;
        let history_id: HistoryTreeId = history.parse()?;
        let source = container.file_name().and_then(|name| name.to_str()).ok_or_else(invalid)?;
        let source_name = Name::parse(source)?;
        let existing = read(root, container)?;
        if let Some((intent, _)) = &existing {
            if intent.file != file_id || intent.history != history_id
                || intent.sequence.get() != state.sequence
                || &intent.target_name != target_name
            {
                return Err(invalid());
            }
            // The disk intent is not an authority for the work set. Reconstruct
            // coverage from the admitted candidate and actual source/target leaves.
            let listed: BTreeSet<_> = intent.bodies.iter().map(|body| body.storage).collect();
            let versions = container.join(crate::persist::layout::VERSIONS.as_path());
            for version in state.versions.values().filter(|version| {
                version.file_id == file && version.history_tree_id == history
            }) {
                let source = installed_version_leaf(version.storage_id, &intent.source_name)?;
                let target = installed_version_leaf(version.storage_id, &intent.target_name)?;
                let present = root.optional_metadata(&versions.join(source.as_path()))?.is_some()
                    || root.optional_metadata(&versions.join(target.as_path()))?.is_some();
                if present && !listed.contains(&version.storage_id) {
                    return Err(invalid());
                }
            }
            let mut complete = true;
            let mut held = Vec::new();
            for body in &intent.bodies {
                let version = state.versions.get(&body.version.to_string()).ok_or_else(invalid)?;
                if version.storage_id != body.storage || version.file_id != file
                    || version.history_tree_id != history
                {
                    return Err(invalid());
                }
                let (path, done) = position(root, container, intent, body)?;
                complete &= done;
                held.push(hold_body(root, &path, version)?);
            }
            if !complete || source_name != intent.target_name {
                return Ok(Some(Self {
                    intent: intent.clone(),
                    original: existing.map(|(_, bytes)| bytes),
                    held,
                }));
            }
        }
        if &source_name == target_name {
            return Ok(None);
        }
        let identity = root.directory_identity(container)?;
        let mut intent = Intent {
            file: file_id,
            history: history_id,
            sequence: Sequence::new(state.sequence),
            source_name,
            target_name: target_name.clone(),
            directory_device: identity.device,
            directory_inode: identity.inode,
            bodies: Vec::new(),
        };
        let mut held = Vec::new();
        let mut ids = BTreeSet::new();
        for version in state.versions.values().filter(|version| {
            version.file_id == file && version.history_tree_id == history
        }) {
            if !ids.insert(version.storage_id) || intent.bodies.len() == MAX_VERSIONS {
                return Err(invalid());
            }
            let source = installed_version_leaf(version.storage_id, &intent.source_name)?;
            let target = installed_version_leaf(version.storage_id, target_name)?;
            check_target_name(root, &target)?;
            let versions = container.join(crate::persist::layout::VERSIONS.as_path());
            let source = versions.join(source.as_path());
            let target = versions.join(target.as_path());
            if root.optional_metadata(&target)?.is_some() {
                return Err(invalid());
            }
            if root.optional_metadata(&source)?.is_none() {
                // A missing copy or a cleared body is not manufactured by a name change.
                continue;
            }
            let original = hold_body(root, &source, version)?;
            let metadata = original.file.metadata()?;
            let identity = FileIdentity::from_metadata(&metadata);
            intent.bodies.push(Body {
                version: version.version_id.parse()?,
                storage: version.storage_id,
                device: identity.device,
                inode: identity.inode,
                size: metadata.len(),
                modified_seconds: metadata.mtime(),
                modified_nanoseconds: metadata.mtime_nsec(),
            });
            held.push(original);
        }
        crate::disk::encode_metadata(&intent, MAX_BYTES)?;
        Ok(Some(Self { intent, original: existing.map(|(_, bytes)| bytes), held }))
    }

    /// Caller has durably selected the complete publication before invoking this.
    pub(super) fn apply(
        self,
        root: &DiskRoot,
        container: &Path,
        observe: &mut impl FnMut(&'static str) -> Result<()>,
    ) -> Result<Vec<(VersionId, AppliedVersionRename)>> {
        check_directory(root, container, &self.intent)?;
        let current = read(root, container)?.map(|(_, bytes)| bytes);
        if current != self.original {
            return Err(invalid());
        }
        let bytes = crate::disk::encode_metadata(&self.intent, MAX_BYTES)?;
        root.write_atomic(&intent_path(container), &bytes, self.original.is_some())?;
        root.sync_dir(container)?;
        observe("version-names-intent")?;
        let mut renamed = Vec::new();
        for (body, held) in self.intent.bodies.iter().zip(self.held) {
            let identity = FileIdentity::from_metadata(&held.file.metadata()?);
            if identity.device != body.device || identity.inode != body.inode {
                return Err(invalid());
            }
            let (path, complete) = position(root, container, &self.intent, body)?;
            let (_, target) = paths(container, &self.intent, body)?;
            // Completed recovery leaves retain a freshly captured stamp too;
            // a prior rename does not authorize changes after that capture.
            let before = crate::file_stamp::FileStamp::from_metadata(&held.file.metadata()?);
            if before != held.stamp {
                return Err(invalid());
            }
            if !complete {
                root.rename_noreplace_synced(&path, &target)?;
                let after = crate::file_stamp::FileStamp::from_metadata(&held.file.metadata()?);
                if !after.differs_only_by_rename(&before) {
                    return Err(invalid());
                }
                renamed.push((body.version, AppliedVersionRename { before, after }));
            }
            position(root, container, &self.intent, body)?;
            root.sync_dir(&container.join(crate::persist::layout::VERSIONS.as_path()))?;
            observe("version-name-renamed")?;
        }
        root.sync_dir(container)?;
        Ok(renamed)
    }
}

/// Locate the one retained inode during a decided rename interrupted before
/// the outer container move. Byte verification and publication admission remain
/// the responsibility of the existing caller.
pub(super) fn interrupted_path(
    root: &DiskRoot,
    container: &Path,
    version: &FileVersion,
) -> Result<Option<PathBuf>> {
    let Some((intent, _)) = read(root, container)? else { return Ok(None); };
    if intent.file.to_string() != version.file_id
        || intent.history.to_string() != version.history_tree_id
    {
        return Err(invalid());
    }
    let Some(body) = intent.bodies.iter().find(|body| body.version.to_string() == version.version_id)
    else { return Ok(None); };
    if body.storage != version.storage_id {
        return Err(invalid());
    }
    position(root, container, &intent, body).map(|(path, _)| Some(path))
}

/// Expected publication facts retained by the caller's prepared candidate,
/// never reconstructed from the mutable locator being cleaned up.
pub(super) struct CompletionBinding {
    pub(super) file: FileId,
    pub(super) history: HistoryTreeId,
    pub(super) sequence: Sequence,
    pub(super) target_name: Name,
}

/// Remove only the exact completed intent after the outer container and FILE
/// metadata have reached their target. Interrupted cleanup is harmless.
pub(super) fn finish(
    root: &DiskRoot,
    container: &Path,
    expected: &CompletionBinding,
) -> Result<()> {
    let Some((intent, bytes)) = read(root, container)? else { return Ok(()); };
    if intent.file != expected.file || intent.history != expected.history
        || intent.sequence != expected.sequence || intent.target_name != expected.target_name
        || container.file_name().and_then(|name| name.to_str())
            != Some(expected.target_name.as_str())
    {
        return Err(invalid());
    }
    for body in &intent.bodies {
        if !position(root, container, &intent, body)?.1 {
            return Err(invalid());
        }
    }
    if root.read_bytes(&intent_path(container), MAX_BYTES)? != bytes {
        return Err(invalid());
    }
    root.remove_file_synced(&intent_path(container))?;
    root.sync_dir(container)
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/container_storage/version_rename.rs"]
mod tests;
