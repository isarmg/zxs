//! Missing physical entries on one held target. This is a space observation, not
//! a reservation or integrity proof: every install and complete readback still runs.
use crate::disk::DiskRoot;
use crate::engine::backup_preparation::{RESERVE, inspection::ReportCounts};
use crate::history_package::PreparedHistoryPackage;
use crate::{Error, ErrorCode, Result};
use platform_io::fs_safety::{Error as FsError, RelativePath};
use std::num::NonZeroU64;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;

mod layout;
pub(crate) use layout::require_new_backup_directories;
pub(in crate::engine) use layout::require_new_feedback_capacity;

mod records;
pub(super) use records::Records;

#[derive(Debug, error_definition::Error)]
enum CapacityError {
    #[error("backup file addresses overlap or are not in address order")]
    Layout,
    #[error("backup target reports unusable allocation units")]
    Geometry,
    #[error("complete backup allocation exceeds the capacity counter")]
    Overflow,
    #[error("complete backup receipt exceeds its existing reader limit")]
    RecordLimit,
    #[error("backup target cannot hold the complete frozen unit and its reserve")]
    Space,
    #[error("backup target lacks available inodes for its complete frozen unit")]
    Inodes,
}

impl From<CapacityError> for Error {
    fn from(error: CapacityError) -> Self {
        let code = match error {
            CapacityError::Layout => ErrorCode::HistoryPackageInvalid,
            CapacityError::Geometry | CapacityError::Overflow | CapacityError::RecordLimit => {
                ErrorCode::ResourceLimit
            }
            CapacityError::Space | CapacityError::Inodes => ErrorCode::NoSpace,
        };
        storage_contract::Error::new(code, error.to_string()).into()
    }
}

struct Space {
    unit: NonZeroU64,
    bytes: u64,
    inodes: Option<u64>,
}

impl Space {
    fn observe(root: &DiskRoot) -> Result<Self> {
        let stat = root.filesystem_statistics()?;
        let fragment = NonZeroU64::new(stat.f_frsize).ok_or(CapacityError::Geometry)?;
        Ok(Self {
            // f_bavail counts fragments, not optimal I/O blocks. Charge the
            // larger unit for allocation; never enlarge the available bytes.
            unit: fragment.max(NonZeroU64::new(stat.f_bsize).unwrap_or(fragment)),
            bytes: stat.f_bavail.saturating_mul(fragment.get()),
            // Some filesystems allocate inodes dynamically and report no total.
            inodes: (stat.f_files != 0).then_some(stat.f_favail),
        })
    }
}

#[derive(Default)]
struct Needed {
    files: u64,
    directories: u64,
    bytes: u64,
}

impl Needed {
    fn charge(&mut self, count: EntryCost, unit: NonZeroU64) -> Result<()> {
        let entries = match count {
            EntryCost::Directory => &mut self.directories,
            EntryCost::File(_) => &mut self.files,
        };
        *entries = entries.checked_add(1).ok_or(CapacityError::Overflow)?;
        self.bytes = count.allocation(unit)?.checked_add(self.bytes).ok_or(CapacityError::Overflow)?;
        Ok(())
    }

    fn material_reserve(&self) -> u64 {
        if self.files == 0 && self.directories == 0 {
            64 * 1024
        } else {
            RESERVE
        }
    }

    fn require(&self, space: &Space, reserve: u64) -> Result<()> {
        let entries = self
            .files
            .checked_add(self.directories)
            .ok_or(CapacityError::Overflow)?;
        let bytes = self
            .bytes
            .checked_add(reserve)
            .ok_or(CapacityError::Overflow)?;
        if bytes > space.bytes {
            return Err(CapacityError::Space.into());
        }
        if space.inodes.is_some_and(|available| entries > available) {
            return Err(CapacityError::Inodes.into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy)]
enum EntryCost {
    Directory,
    File(u64),
}
impl EntryCost {
    fn allocation(self, unit: NonZeroU64) -> Result<u64> {
        let units = match self {
            Self::Directory => 1,
            Self::File(length) => length.div_ceil(unit.get()),
        };
        // Each new name can grow its parent, including a preexisting parent.
        // Atomic writers rename their one staged inode to its final name.
        Ok(units.checked_add(1).and_then(|n| n.checked_mul(unit.get()))
            .ok_or(CapacityError::Overflow)?)
    }
}

pub(super) struct Census<'a> {
    root: &'a DiskRoot,
    unit: NonZeroU64,
    // Only the last file's contiguous ancestor chain, never one FD per entry.
    parents: Vec<(RelativePath, bool)>,
    previous: Option<RelativePath>,
    needed: Needed,
}

impl<'a> Census<'a> {
    pub(super) fn new(root: &'a DiskRoot) -> Result<Self> {
        Ok(Self {
            root,
            unit: Space::observe(root)?.unit,
            parents: Vec::new(),
            previous: None,
            needed: Needed::default(),
        })
    }

    fn entry(
        &mut self,
        path: &RelativePath,
        parent_present: bool,
        cost: EntryCost,
    ) -> Result<bool> {
        if parent_present && let Some(stat) = self.root.optional_relative_metadata(path)? {
            match cost {
                EntryCost::Directory if stat.is_dir() => return Ok(true),
                EntryCost::File(_) if stat.is_file() => {
                    if stat.nlink() != 1 {
                        return Err(FsError::MultipleLinks(path.as_path().to_owned()).into());
                    }
                    return Ok(true);
                }
                _ => return Err(FsError::UnsafeFileType(path.as_path().to_owned()).into()),
            }
        }
        self.needed.charge(cost, self.unit)?;
        Ok(false)
    }

    pub(super) fn file(&mut self, path: PathBuf, length: u64) -> Result<()> {
        self.file_present(path, length).map(|_| ())
    }

    /// True means only that a regular, single-link file exists. A caller
    /// enforcing exact retained bytes must still read and compare that file.
    pub(super) fn file_present(&mut self, path: PathBuf, length: u64) -> Result<bool> {
        let file = RelativePath::new(path)?;
        let path = file.as_path();
        if self.previous.as_ref().is_some_and(|previous| {
            path <= previous.as_path() || path.starts_with(previous.as_path())
        }) {
            return Err(CapacityError::Layout.into());
        }
        while self
            .parents
            .last()
            .is_some_and(|(dir, _)| !path.starts_with(dir.as_path()))
        {
            self.parents.pop();
        }
        let mut present = self.parents.last().is_none_or(|(_, present)| *present);
        // The producers yield canonical sibling subtrees contiguously. An absent
        // ancestor accounts for its descendants without probing nonexistent parents.
        let parents: Vec<&Path> = path
            .ancestors()
            .skip(1)
            .take_while(|p| !p.as_os_str().is_empty())
            .collect();
        for parent in parents.into_iter().rev().skip(self.parents.len()) {
            let dir = RelativePath::new(parent)?;
            present = self.entry(&dir, present, EntryCost::Directory)?;
            self.parents.push((dir, present));
        }
        let present = self.entry(&file, present, EntryCost::File(length))?;
        self.previous = Some(file);
        Ok(present)
    }

    pub(super) fn require(
        mut self,
        records: Records<'_>,
        plans: &[PreparedHistoryPackage],
        counts: ReportCounts,
        stop: &AtomicBool,
    ) -> Result<()> {
        let reserve = self.needed.material_reserve();
        records.charge(&mut self, plans, counts, stop)?;
        self.require_with_reserve(reserve)
    }

    pub(super) fn require_with_reserve(self, reserve: u64) -> Result<()> {
        // Sampling the free counts after the whole census matters for large
        // plans. External users may still consume space after this observation.
        let space = Space::observe(self.root)?;
        if self.unit != space.unit {
            return Err(CapacityError::Geometry.into());
        }
        self.needed.require(&space, reserve)
    }
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/backup_copy_capacity_tests.rs"]
mod tests;
