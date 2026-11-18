//! A completion seen during the space check is a read-only recovery tail on
//! that original, even if its name goes missing before the install loop.
use super::{COMPLETE, MAX_COMPLETE, invalid};
use crate::{Result, disk::{DiskRoot, FileStamp, HeldRegular}};
use std::collections::BTreeMap;

pub(super) struct Receipt(HeldRegular);

impl Receipt {
    pub(super) fn read(root: &DiskRoot) -> Result<Option<Self>> {
        Ok(HeldRegular::capture(root, COMPLETE, MAX_COMPLETE)?.map(Self))
    }

    pub(super) fn require(&self, root: &DiskRoot, material: &[u8]) -> Result<()> {
        if self.0.bytes() != material
            || Self::read(root)?.as_ref().map(|now| now.0.stamp()) != Some(self.0.stamp())
        {
            return Err(invalid("original FILE completion or its physical material changed"));
        }
        self.0.check_running()
    }
}

#[derive(Default)]
pub(in super::super) struct Seen(BTreeMap<(u64, u64), Option<FileStamp>>);

impl Seen {
    fn key(root: &DiskRoot) -> Result<(u64, u64)> {
        let actual = root.directory_identity(root.path())?;
        Ok((actual.device, actual.inode))
    }

    pub(super) fn remember(&mut self, root: &DiskRoot, record: Option<&Receipt>) -> Result<()> {
        let stamp = record.map(|record| record.0.stamp().clone());
        if self.0.insert(Self::key(root)?, stamp).is_some() {
            return Err(invalid("original FILE root occurs twice in the capacity set"));
        }
        Ok(())
    }

    pub(super) fn read(&self, root: &DiskRoot) -> Result<Option<Receipt>> {
        let previous = self.0.get(&Self::key(root)?)
            .ok_or_else(|| invalid("original FILE root changed after capacity observation"))?;
        let record = Receipt::read(root)?;
        if previous.as_ref().is_some_and(|seen| {
            record.as_ref().map(|record| record.0.stamp()) != Some(seen)
        }) {
            return Err(invalid(
                "previously observed original FILE completion changed or disappeared",
            ));
        }
        Ok(record)
    }
}

#[cfg(test)]
#[path = "../../../../../../../../../../../../test/engine/unit/backup_original_receipt_continuity_tests.rs"]
mod tests;
