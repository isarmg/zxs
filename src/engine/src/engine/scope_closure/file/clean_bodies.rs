//! Narrow positive case: one clean, born FILE, all its native stored versions
//! available and selected on this exact registration. No dirty/Unborn body,
//! missing replica, migration or terminal occurrence is classified here.
use super::*;
use crate::{ContentState, FileVersion, disk::DiskRoot, file_stamp::FileStamp, persist::layout};
use crate::engine::scope_closure::{OrdinalBuilder, OrdinalPages};
use data_serialization::Serialize;
use platform_io::fs_safety::{EntryName, linux::FileIdentity};
use std::{collections::{BTreeMap, BTreeSet}, os::unix::fs::MetadataExt};
use storage_contract::{ContentHash, VersionId, file_draft::FileWorkingState};

#[derive(Serialize)]
#[serialization(tag = "kind", rename_all = "snake_case")]
enum Body<'a> {
    Full {
        source: VersionId,
        relative_name: &'a str,
        #[serialization(with = "storage_contract::u64_string")]
        bytes: u64,
        content_hash: ContentHash,
    },
}
struct Leaf { name: EntryName, stamp: FileStamp, held: crate::ReadFile }
pub(in crate::engine) struct CleanFileBodies {
    pages: OrdinalPages,
    version_dir: DiskRoot,
    dir_entity: FileIdentity,
    names: BTreeMap<VersionId, Leaf>,
    image: Representation,
    registration: DiskRegistrationIdentity,
    file: FileId,
    occurrence: ContainerReplica,
    _permit: crate::engine::PayloadProofReservation,
}
fn rejected() -> crate::Error { Failure::ImageMismatch.into() }
fn check_entries(dir: &DiskRoot, names: &BTreeMap<VersionId, Leaf>) -> Result<()> {
    let mut expected: BTreeSet<&EntryName> = names.values().map(|leaf| &leaf.name).collect();
    if expected.len() != names.len() { return Err(rejected()); }
    for name in dir.entries(dir.path())? {
        let name = name?;
        if !expected.remove(&name) { return Err(rejected()); }
    }
    if !expected.is_empty() { return Err(rejected()); }
    Ok(())
}
fn clean(working: &FileWorkingState) -> bool {
    matches!(working, FileWorkingState::Automatic { .. } | FileWorkingState::ManualClean { .. })
}
impl ResidentFileRows {
    /// This is not a default no-body path: unsupported cases return an error.
    pub(in crate::engine) fn clean_full_bodies(&self, context: &PlanningContext<'_>,
        projection: crate::state_projection::StateProjection<'_>) -> Result<CleanFileBodies> {
        // One shared owner-indexed projection can be reused across all FILEs.
        if !std::ptr::eq(context.state.as_ref(), projection.state()) { return Err(rejected()); }
        self.image.require_rows(projection)?;
        let image = &self.image;
        let state = context.state;
        let file = state.file_containers.get(&image.file).ok_or_else(rejected)?;
        let disk: DiskId = self.registration.disk_id.parse()?;
        if context.config.storage_roots.get(&disk).is_none_or(|r| r.registration != self.registration)
            || file.replicas.get(&disk) != Some(&self.replica) || file.history_tree_id != image.history
            || file.working != image.working || !clean(&file.working) || image.versions.is_empty()
        { return Err(rejected()); }
        // Keep every original FD until the consumer finishes the decision or
        // abandons it. Never substitute rapid inode reuse for an original.
        let permits = image.versions.len().checked_add(1).ok_or(Failure::Budget)?;
        let permit = crate::engine::payload_proof_budget().reserve(permits).ok_or(Failure::Budget)?;
        let container = self.holder.container()?;
        let path = container.path().join(layout::VERSIONS.as_path());
        let directory = container.private_child(&path)?;
        let dir_entity = directory.directory_identity(directory.path())?;
        let mut values = OrdinalBuilder::new(image.versions.len());
        let mut names = BTreeMap::new();
        let mut physical = BTreeSet::new();
        for (version, status) in &image.versions {
            let placement = state.placements.get(&version.version_id).ok_or_else(rejected)?;
            if *status != ContentState::Available || state.versions.get(&version.version_id) != Some(version)
                || state.content_states.get(&version.version_id).is_some_and(|s| *s != ContentState::Available)
                || version.logical_size > context.config.max_file_bytes
                || !placement.route_slot_ids.contains(&self.registration.slot_id)
                || placement.binding_generations.get(&self.registration.slot_id) != Some(&self.registration.binding_generation)
            { return Err(rejected()); }
            let id: VersionId = version.version_id.parse()?;
            let p = self.holder.body_path(version)?;
            // Do not jump to a rename's staging location or follow a suffix.
            if p.parent() != Some(path.as_path()) { return Err(rejected()); }
            let name = EntryName::new(p.file_name().ok_or_else(rejected)?)?;
            let (held, stamp) = read_body(&directory, &name, version, &mut physical)?;
            let relative = p.strip_prefix(container.path()).map_err(|_| rejected())?.to_str().ok_or_else(rejected)?;
            values.push(&Body::Full { source: id, relative_name: relative, bytes: version.logical_size,
                content_hash: version.content_hash })?;
            if names.insert(id, Leaf { name, stamp, held }).is_some() { return Err(rejected()); }
        }
        let result = CleanFileBodies { pages: values.finish()?, version_dir: directory, dir_entity,
            names, image: self.representation, registration: self.registration.clone(), file: image.file,
            occurrence: self.replica.clone(), _permit: permit };
        result.recheck(self)?;
        Ok(result)
    }
}

fn read_body(dir: &DiskRoot, name: &EntryName, version: &FileVersion,
             physical: &mut BTreeSet<(u64, u64)>) -> Result<(crate::ReadFile, FileStamp)> {
    let mut file = dir.open_read_entry(name)?;
    let m = file.metadata()?;
    if !m.is_file() || m.nlink() != 1 || m.len() != version.logical_size
        || !physical.insert((m.dev(), m.ino()))
    { return Err(rejected()); }
    let stamp = FileStamp::from_metadata(&m);
    // Unlike an installation verifier this only reads and does not fsync.
    file.verify_full(platform_io::payload::Expectation {
        bytes: version.logical_size, blake3: Some(version.content_hash.digest()),
    })?;
    let named = dir.open_read_entry(name)?;
    if FileStamp::from_metadata(&file.metadata()?) != stamp || FileStamp::from_metadata(&named.metadata()?) != stamp {
        return Err(rejected());
    }
    file.check_running()?;
    Ok((file, stamp))
}

impl CleanFileBodies {
    pub(in crate::engine) fn pages(&self) -> &OrdinalPages { &self.pages }
    /// A named replacement, changed body, extra version or moved original
    /// parent invalidates this measurement even if the digest is unchanged.
    pub(in crate::engine) fn recheck(&self, file: &ResidentFileRows) -> Result<()> {
        if self.registration != file.registration || self.file != file.image.file
            || self.image != file.representation || self.occurrence != file.replica {
            return Err(rejected());
        }
        file.image.require_named(&file.holder)?;
        let container = file.holder.container()?;
        let named = container.private_child_matching(&container.path().join(layout::VERSIONS.as_path()), self.dir_entity)?;
        check_entries(&named, &self.names)?;
        for leaf in self.names.values() {
            let f = named.open_read_entry(&leaf.name)?;
            leaf.held.check_running()?;
            if FileStamp::from_metadata(&f.metadata()?) != leaf.stamp
                || FileStamp::from_metadata(&leaf.held.metadata()?) != leaf.stamp { return Err(rejected()); }
        }
        if self.version_dir.directory_identity(self.version_dir.path())? != self.dir_entity {
            return Err(rejected());
        }
        file.image.require_named(&file.holder)
    }
}

#[cfg(test)]
#[path = "../../../../../../test/engine/unit/engine/scope_clean_file_bodies.rs"]
mod tests;
