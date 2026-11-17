//! One pool-wide retained-name registration projection. Every caller consumes
//! the same permanent original decisions, including pending container births;
//! no worktree, slot, mount or live-container inventory owns a separate counter.
use crate::ErrorCode;
use std::collections::BTreeMap;
use storage_contract::file_container::retained::{
    RETAINED_DISK_COUNT, RetainedAllocation, RetainedDiskNumber, RetainedDiskRegistration,
    RetainedKind,
};
use storage_contract::{DiskId, OperationId, PoolId, Timestamp};

mod allocation;
pub(crate) use allocation::allocate_file;

#[derive(Debug, error_definition::Error)]
pub enum RetainedNameError {
    #[error("retained disk registration differs from its pool or original assignment")]
    Registration,
    #[error("all 1296 retained disk numbers are permanently registered")]
    NumbersExhausted,
    #[error("retained name differs from its original disk, timestamp, kind or user name")]
    Allocation,
    #[error("retained target names exceed the complete scan budget")]
    InventoryBudget,
    #[error("retained target contains a non-UTF-8 component")]
    NonUtf8,
    #[error("retained name collisions exhausted the bounded allocation attempts")]
    NamesExhausted,
    #[error("reading the held retained target: {source}")]
    Target {
        #[source]
        source: Box<crate::Error>,
    },
    #[error("reading a retained target entry: {source}")]
    Entry {
        #[source]
        source: platform_io::fs_safety::Error,
    },
    #[error("retained target cannot admit the complete original name: {source}")]
    Budget {
        #[source]
        source: crate::name_budget::NameBudgetError,
    },
    #[error("constructing the original retained name: {source}")]
    Name {
        #[source]
        source: storage_contract::file_container::retained::RetainedNameError,
    },
    #[error("retained allocation timestamp cannot be represented by the UTC clock: {source}")]
    ClockValue {
        #[source]
        source: std::num::TryFromIntError,
    },
    #[error("retained allocation timestamp exceeds the supported UTC range: {source}")]
    ClockRange {
        #[source]
        source: date_time::error::ComponentRange,
    },
}

impl RetainedNameError {
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Registration | Self::Allocation | Self::NonUtf8 => {
                ErrorCode::ContainerLayoutInvalid
            }
            Self::NumbersExhausted | Self::InventoryBudget => ErrorCode::ResourceLimitExceeded,
            Self::NamesExhausted => ErrorCode::NameConflict,
            Self::Target { source } => source.code(),
            Self::Entry { .. } | Self::ClockValue { .. } | Self::ClockRange { .. } => {
                ErrorCode::IoError
            }
            Self::Budget { .. } => ErrorCode::NameTooLong,
            Self::Name { source } => match source {
                storage_contract::file_container::retained::RetainedNameError::Entropy {
                    ..
                }
                | storage_contract::file_container::retained::RetainedNameError::RandomDraws => {
                    ErrorCode::IoError
                }
                storage_contract::file_container::retained::RetainedNameError::DiskNumber
                | storage_contract::file_container::retained::RetainedNameError::Prefix
                | storage_contract::file_container::retained::RetainedNameError::OriginalName {
                    ..
                } => ErrorCode::ContainerLayoutInvalid,
            },
        }
    }
}

type Result<T> = std::result::Result<T, RetainedNameError>;

pub(crate) struct RetainedDiskRegistry {
    pool: PoolId,
    disks: BTreeMap<DiskId, RetainedDiskRegistration>,
    numbers: BTreeMap<RetainedDiskNumber, DiskId>,
    originals: BTreeMap<OperationId, DiskId>,
}

impl RetainedDiskRegistry {
    pub(crate) fn new(pool: PoolId) -> Self {
        Self {
            pool,
            disks: BTreeMap::new(),
            numbers: BTreeMap::new(),
            originals: BTreeMap::new(),
        }
    }

    /// Feed admitted original decisions in publication order, not current
    /// visible outcomes. The first decision must itself own its registration;
    /// later ones can only repeat that already-admitted exact fact.
    pub(crate) fn observe(
        &mut self,
        original: OperationId,
        registration: &RetainedDiskRegistration,
    ) -> Result<()> {
        if registration.pool != self.pool {
            return Err(RetainedNameError::Registration);
        }
        if let Some(existing) = self.disks.get(&registration.disk) {
            return if existing == registration {
                Ok(())
            } else {
                Err(RetainedNameError::Registration)
            };
        }
        if registration.operation != original
            || self.originals.contains_key(&original)
            || self.numbers.contains_key(&registration.number)
            || self.next_number()? != registration.number
        {
            return Err(RetainedNameError::Registration);
        }
        self.disks.insert(registration.disk, registration.clone());
        self.numbers.insert(registration.number, registration.disk);
        self.originals.insert(original, registration.disk);
        Ok(())
    }

    fn next_number(&self) -> Result<RetainedDiskNumber> {
        (0..RETAINED_DISK_COUNT)
            .filter_map(RetainedDiskNumber::new)
            .find(|number| !self.numbers.contains_key(number))
            .ok_or(RetainedNameError::NumbersExhausted)
    }

    /// A new assignment is only prospective until the caller publishes it in
    /// the same real native decision as the first retained container. No local
    /// map mutation, separate reserve operation or configuration rewrite occurs.
    pub(crate) fn registration(
        &self,
        disk: DiskId,
        original: OperationId,
    ) -> Result<RetainedDiskRegistration> {
        match self.disks.get(&disk) {
            Some(existing) => Ok(existing.clone()),
            None if self.originals.contains_key(&original) => Err(RetainedNameError::Registration),
            None => Ok(RetainedDiskRegistration {
                pool: self.pool,
                disk,
                number: self.next_number()?,
                operation: original,
            }),
        }
    }

    /// Repeated allocations are checked against the admitted global mapping;
    /// source/held-root authority remains the actual container issuer's work.
    pub(crate) fn require_allocation(
        &self,
        allocation: &RetainedAllocation,
        original_name: &name_policy::Name,
        kind: RetainedKind,
    ) -> Result<()> {
        if self.disks.get(&allocation.registration.disk) != Some(&allocation.registration)
            || allocation.name.number() != allocation.registration.number
            || allocation.name.kind() != kind
            || allocation.name.original_name() != original_name.as_str()
            || allocation.name.utc_stamp() != utc_stamp(allocation.created_at)?
        {
            return Err(RetainedNameError::Allocation);
        }
        Ok(())
    }
}

fn utc_stamp(timestamp: Timestamp) -> Result<String> {
    let seconds = i64::try_from(timestamp.unix_seconds())
        .map_err(|source| RetainedNameError::ClockValue { source })?;
    let date = date_time::OffsetDateTime::from_unix_timestamp(seconds)
        .map_err(|source| RetainedNameError::ClockRange { source })?;
    const BASE36: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    Ok(format!(
        "{:02}{:x}{}{}{:02}",
        date.year() % 100,
        u8::from(date.month()),
        char::from(BASE36[usize::from(date.day())]),
        char::from(BASE36[usize::from(date.hour())]),
        date.minute()
    ))
}

#[cfg(test)]
#[path = "../../../test/engine/unit/retained_names.rs"]
mod tests;
