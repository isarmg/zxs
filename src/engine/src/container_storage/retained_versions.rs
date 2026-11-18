//! A namespace rename moves one outer directory. Its BODY leaves are not a
//! rename work list. Prove the same installed files before and after the move.
use super::{VersionNameError, installed_version_leaf, verify_body};
use crate::disk::DiskRoot;
use crate::file_stamp::FileStamp;
use crate::state_projection::StateProjection;
use crate::{ContentState, Error, ReadFile, Result};
use platform_io::fs_safety::{EntryName, linux::FileIdentity};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

fn changed() -> Error {
    Error::VersionNames(VersionNameError::Material)
}

fn reserve(count: usize) -> Result<crate::engine::PayloadProofReservation> {
    crate::engine::payload_proof_budget().reserve(count)
        .ok_or(Error::VersionNames(VersionNameError::Descriptors))
}

struct Original {
    file: ReadFile,
    stamp: FileStamp,
    _lease: crate::engine::PayloadProofReservation,
}

pub(super) struct HeldBodies {
    versions: DiskRoot,
    directory: FileIdentity,
    originals: BTreeMap<EntryName, Original>,
    // Holds the versions FD and the transient verification/iteration descriptors.
    _lease: crate::engine::PayloadProofReservation,
}

impl HeldBodies {
    pub(super) fn capture(
        root: &DiskRoot,
        container: &Path,
        view: StateProjection<'_>,
        file: &str,
        history: &str,
        slot: Option<&str>,
    ) -> Result<Self> {
        let lease = reserve(3)?;
        let versions = root.private_child(&container.join(crate::persist::layout::VERSIONS.as_path()))?;
        let directory = versions.directory_identity(versions.path())?;
        let mut originals = BTreeMap::new();
        let mut ids = BTreeSet::new();
        let state = view.state();
        for key in view.owners().versions(file) {
            let version = &state.versions[key];
            if version.history_tree_id != history {
                continue;
            }
            if ids.len() == 65_536 || !ids.insert(version.storage_id) {
                return Err(changed());
            }
            let leaf = installed_version_leaf(version)?;
            let path = versions.path().join(leaf.as_path());
            let Some(metadata) = versions.optional_metadata(&path)? else {
                let required = slot.is_some_and(|slot| {
                    state.content_states.get(key) == Some(&ContentState::Available)
                        && state.placements.get(key).is_some_and(|placement| {
                            placement.route_slot_ids.iter().any(|route| route == slot)
                        })
                });
                if required {
                    return Err(changed());
                }
                continue;
            };
            let stamp = FileStamp::from_metadata(&metadata);
            let lease = reserve(1)?;
            let file = verify_body(&versions, &path, version)?;
            if FileStamp::from_metadata(&file.metadata()?) != stamp {
                return Err(changed());
            }
            originals.insert(leaf, Original { file, stamp, _lease: lease });
        }
        let proof = Self { versions, directory, originals, _lease: lease };
        proof.verify_at(root, container)?;
        Ok(proof)
    }

    pub(super) fn verify_at(&self, root: &DiskRoot, container: &Path) -> Result<()> {
        let path = container.join(crate::persist::layout::VERSIONS.as_path());
        let current = root.private_child_matching(&path, self.directory)?;
        if self.versions.directory_identity(self.versions.path())? != self.directory {
            return Err(changed());
        }
        // Enumerate only this held versions parent to reject extra or replaced
        // material. No unknown spelling is ever an alternate source of a BODY.
        let mut count = 0;
        for leaf in current.entries(current.path())? {
            let leaf = leaf?;
            let original = self.originals.get(&leaf).ok_or_else(changed)?;
            let path = current.path().join(leaf.as_path());
            let named = current.open_read_regular(&path)?;
            if FileStamp::from_metadata(&named.metadata()?) != original.stamp
                || FileStamp::from_metadata(&original.file.metadata()?) != original.stamp
            {
                return Err(changed());
            }
            count += 1;
        }
        if count != self.originals.len() || root.directory_identity(&path)? != self.directory {
            return Err(changed());
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/container_storage/retained_versions.rs"]
mod tests;
