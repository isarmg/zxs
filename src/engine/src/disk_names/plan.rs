//! Complete registered-topology projection; no physical discovery or mutation.
use crate::disk_names::{
    DiskNameObservation, DiskNamePreviewBlocker, DiskNamePreviewPhase, DiskSourceNameError,
    DiskSourceNamePreview, DiskSourcePath, DiskSourceRenameRoot, DiskSourceSlice,
    MAX_CHAIN_IDENTITIES, MAX_DISKS, MAX_SLICES, Result, layout, limit,
};
use name_policy::{Name, SlotId};
use std::collections::{BTreeMap, BTreeSet};
use storage_contract::DiskId;

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct SlotKey(String);
impl From<&SlotId> for SlotKey {
    fn from(slot: &SlotId) -> Self {
        Self(slot.as_str().to_ascii_lowercase())
    }
}

#[derive(PartialEq, Eq, PartialOrd, Ord)]
struct NameKey(String);

pub(super) fn build(preview: &mut DiskSourceNamePreview) -> Result<()> {
    if preview.disks.len() > MAX_DISKS {
        return Err(limit());
    }
    let requested = preview.requested_disk_id;
    let source = preview
        .disks
        .iter()
        .find(|disk| disk.identity.disk_id == requested)
        .ok_or(DiskSourceNameError::UnregisteredDisk {
            disk_id: requested,
        })?;
    let changed = source.observed_name.as_ref() != Some(&preview.proposed_name);
    conflicts(preview, DiskNamePreviewPhase::Observed);
    conflicts(preview, DiskNamePreviewPhase::Proposed);
    let disks = &preview.disks;
    let slots: BTreeMap<_, _> = disks
        .iter()
        .enumerate()
        .map(|(index, disk)| (SlotKey::from(&disk.identity.slot_id), index))
        .collect();
    let mut chain_identities = 0usize;
    for target in disks {
        let mut cursor = target;
        let mut chain = Vec::new();
        let mut visited = BTreeSet::from([target.identity.disk_id]);
        loop {
            let Some(upstream) = &cursor.upstream_slot_id else {
                if cursor.level != 1 {
                    preview.blockers.push(DiskNamePreviewBlocker::InvalidTopology {
                        target: target.identity.disk_id,
                    });
                }
                break;
            };
            let Some(&index) = slots.get(&SlotKey::from(upstream)) else {
                preview.blockers.push(DiskNamePreviewBlocker::MissingUpstream {
                    target: target.identity.disk_id,
                    slot: upstream.clone(),
                });
                break;
            };
            let parent = &disks[index];
            if parent.level.checked_add(1) != Some(cursor.level)
                || !visited.insert(parent.identity.disk_id)
            {
                preview.blockers.push(DiskNamePreviewBlocker::InvalidTopology {
                    target: target.identity.disk_id,
                });
                break;
            }
            chain.push(index);
            chain_identities = chain_identities.checked_add(chain.len()).ok_or_else(limit)?;
            if preview.slices.len() == MAX_SLICES || chain_identities > MAX_CHAIN_IDENTITIES {
                return Err(limit());
            }
            let before = project(
                disks,
                target,
                &chain,
                requested,
                &preview.proposed_name,
                DiskNamePreviewPhase::Observed,
                &mut preview.blockers,
            );
            let after = project(
                disks,
                target,
                &chain,
                requested,
                &preview.proposed_name,
                DiskNamePreviewPhase::Proposed,
                &mut preview.blockers,
            );
            // The first slice ending at the renamed source is the outermost
            // changed root on this target. Deeper slices are affected, not extra moves.
            if changed && parent.identity.disk_id == requested {
                preview.rename_roots.push(DiskSourceRenameRoot {
                    target: target.identity.disk_id,
                    source: requested,
                    before: before.clone(),
                    after: after.clone(),
                });
            }
            preview.slices.push(DiskSourceSlice {
                target: target.identity.disk_id,
                source: parent.identity.disk_id,
                chain: chain
                    .iter()
                    .map(|&index| disks[index].identity.disk_id)
                    .collect(),
                before,
                after,
                affected: changed
                    && chain
                        .iter()
                        .any(|&index| disks[index].identity.disk_id == requested),
            });
            cursor = parent;
        }
    }
    Ok(())
}

fn conflicts(preview: &mut DiskSourceNamePreview, phase: DiskNamePreviewPhase) {
    let mut names = BTreeMap::new();
    for disk in &preview.disks {
        let name = selected_name(disk, preview.requested_disk_id, &preview.proposed_name, phase);
        let Some(name) = name else {
            continue;
        };
        if let Some(other_disk_id) = names.insert(NameKey(name.key()), disk.identity.disk_id) {
            preview.blockers.push(DiskNamePreviewBlocker::NameConflict {
                disk_id: disk.identity.disk_id,
                other_disk_id,
                phase,
            });
        }
    }
}

fn selected_name<'a>(
    disk: &'a DiskNameObservation,
    requested: DiskId,
    proposed: &'a Name,
    phase: DiskNamePreviewPhase,
) -> Option<&'a Name> {
    match phase {
        DiskNamePreviewPhase::Proposed if disk.identity.disk_id == requested => Some(proposed),
        DiskNamePreviewPhase::Observed | DiskNamePreviewPhase::Proposed => disk.observed_name.as_ref(),
    }
}

fn project(
    disks: &[DiskNameObservation],
    target: &DiskNameObservation,
    chain: &[usize],
    requested: DiskId,
    proposed: &Name,
    phase: DiskNamePreviewPhase,
    blockers: &mut Vec<DiskNamePreviewBlocker>,
) -> Option<DiskSourcePath> {
    let maximum = target.component_maximum?;
    let mut path = String::new();
    for &index in chain {
        let disk = &disks[index];
        let name = selected_name(disk, requested, proposed, phase)?;
        let bytes = name.as_str().len() + layout::UUID_COMPONENT_OVERHEAD;
        if maximum
            .checked_sub(layout::RESERVE_BYTES)
            .is_none_or(|allowed| bytes as u64 > allowed)
        {
            blockers.push(DiskNamePreviewBlocker::ComponentBudget {
                target: target.identity.disk_id,
                source: disk.identity.disk_id,
                phase,
                bytes,
                maximum,
            });
            return None;
        }
        if !path.is_empty() {
            path.push('/');
        }
        path.push_str(name.as_str());
        path.push_str("--");
        // DiskId's canonical Display is disk_ followed by all 32 ASCII hex digits.
        // This renders the typed identity; it never infers identity from a path.
        let identity = disk.identity.disk_id.to_string();
        path.push_str(&identity["disk_".len()..]);
    }
    for root in layout::SLICE_ROOTS {
        let bytes = path.len() + 1 + root.len();
        let components = chain.len() + root.split('/').count();
        if bytes > layout::MAX_PATH_BYTES || components > layout::MAX_PATH_COMPONENTS {
            blockers.push(DiskNamePreviewBlocker::ProjectedPathBudget {
                target: target.identity.disk_id,
                source: disks[*chain.last()?].identity.disk_id,
                phase,
                bytes,
                components,
            });
            return None;
        }
    }
    Some(DiskSourcePath(path))
}
