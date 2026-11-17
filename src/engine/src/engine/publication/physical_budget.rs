//! One aggregate requirement calculation, before any physical preparation.
//! This is a live free-space check, not a cross-session allocation reservation.
use crate::engine::fresh_layout::FreshLayoutError as Failure;
use crate::engine::PlanningContext;
use crate::{DiskConfig, Result};
use std::collections::BTreeMap;
use storage_contract::DiskId;

pub(in crate::engine) struct PhysicalRequirements<'context, 'source> {
    context: &'context PlanningContext<'source>,
    bytes: BTreeMap<DiskId, (&'source DiskConfig, u64)>,
}

impl<'context, 'source> PhysicalRequirements<'context, 'source> {
    pub(in crate::engine) fn new(context: &'context PlanningContext<'source>) -> Self {
        Self { context, bytes: BTreeMap::new() }
    }

    /// A route contributes only to its registered target. Repeated FILEs and
    /// the directory's independent full copies accumulate, never overwrite.
    pub(in crate::engine) fn add(&mut self, disk: &DiskConfig, bytes: u64) -> Result<()> {
        let actual = self.context.disk(&disk.slot_id)?;
        if actual != disk {
            return Err(Failure::ContainerScope.into());
        }
        let id = disk.disk_id.parse::<DiskId>()?;
        let (_, total) = self.bytes.entry(id).or_insert((actual, 0));
        *total = total.checked_add(bytes).ok_or(Failure::PhysicalBytesOverflow)?;
        Ok(())
    }

    pub(in crate::engine) fn check(&self) -> Result<()> {
        // Preserve the ordinary gate's original slot ordering, including which
        // unavailable target wins when more than one route is damaged.
        let mut targets: Vec<_> = self.bytes.iter().collect();
        targets.sort_by(|(_, (left, _)), (_, (right, _))| left.slot_id.cmp(&right.slot_id));
        for (id, (disk, bytes)) in targets {
            let free = self.context.disk_resources.get(disk)?.root.available()?;
            let budget = disk.capacity_bytes.map(|limit| {
                limit.saturating_sub(self.context.used_bytes(&disk.slot_id))
            }).unwrap_or(u64::MAX);
            if bytes.saturating_add(self.context.config.metadata_reserve_bytes) > free
                || *bytes > budget
            {
                return Err(Failure::PhysicalCapacity { disk: *id }.into());
            }
        }
        Ok(())
    }
}
