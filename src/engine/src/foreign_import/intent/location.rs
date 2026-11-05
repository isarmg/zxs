//! Physical source separation from held roots, including bind-mount aliases.
use super::*;
use platform_io::fs_safety::linux::FileIdentity;
use rustix::fs::{AtFlags, Mode, OFlags, ResolveFlags, StatxFlags};

struct Location {
    directory: File,
    identity: FileIdentity,
    mount_id: u64,
    // Raw ancestry descriptors close before their admitted operation retires.
    _activity: Option<DiskIoLease>,
}
impl Location {
    fn held(root: &DiskRoot) -> Result<Self> {
        let activity = root.retain_io_activity()?;
        let directory = root.directory(root.path())?;
        let identity = FileIdentity::from_fd(&directory)?;
        let stat = rustix::fs::statx(
            &directory,
            "",
            AtFlags::EMPTY_PATH | AtFlags::SYMLINK_NOFOLLOW,
            StatxFlags::MNT_ID,
        )
        .map_err(std::io::Error::from)?;
        if stat.stx_mask & StatxFlags::MNT_ID.bits() == 0 {
            return Err(Error::new(
                ErrorCode::FormatUnsupported,
                "mount identity is required to prove an independent foreign target",
            ));
        }
        Ok(Self {
            directory,
            identity,
            mount_id: stat.stx_mnt_id,
            _activity: activity,
        })
    }
    fn descends_from(&self, ancestor: &Self) -> Result<bool> {
        if self.identity.device != ancestor.identity.device {
            return Ok(false);
        }
        // A bind view of a subtree hides its original ancestors. A parent walk
        // alone cannot prove separation across distinct views of one filesystem.
        if self.mount_id != ancestor.mount_id {
            return Err(invalid(
                "foreign source and target use unproven mount aliases of the same filesystem",
            ));
        }
        let mut current = self.directory.try_clone()?;
        for _ in 0..256 {
            let identity = FileIdentity::from_fd(&current)?;
            if identity == ancestor.identity {
                return Ok(true);
            }
            let parent = match rustix::fs::openat2(
                &current,
                "..",
                OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                Mode::empty(),
                ResolveFlags::NO_XDEV | ResolveFlags::NO_SYMLINKS | ResolveFlags::NO_MAGICLINKS,
            ) {
                Ok(parent) => File::from(parent),
                Err(rustix::io::Errno::XDEV) => return Ok(false),
                Err(source) => return Err(std::io::Error::from(source).into()),
            };
            if FileIdentity::from_fd(&parent)? == identity {
                return Ok(false);
            }
            current = parent;
        }
        Err(Error::new(
            ErrorCode::ResourceLimit,
            "foreign root ancestry exceeds the supported bound",
        ))
    }
}

pub(super) fn reject_overlap(source: &DiskRoot, target: &DiskRoot) -> Result<()> {
    if source.path().starts_with(target.path()) || target.path().starts_with(source.path()) {
        return Err(invalid("foreign source and target directories overlap"));
    }
    let source = Location::held(source)?;
    let target = Location::held(target)?;
    if source.descends_from(&target)? || target.descends_from(&source)? {
        return Err(invalid("foreign source and target physically overlap"));
    }
    Ok(())
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/foreign_import/intent_location_tests.rs"]
mod tests;
