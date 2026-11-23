//! Pin the independently installed ZXS child for the duration of one offline action.
use super::OfflineSession;
use crate::{
    pinned::{Entity, Mount, Pinned, installed_regular_reader, same_mount_leaf},
    product_layout::BIN,
};
use std::{
    ffi::OsStr,
    fs::{File, Metadata},
    io,
    os::unix::fs::MetadataExt,
    path::Path,
};
fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "cannot confirm the protected installed ZXS executable",
    )
}

impl OfflineSession {
    pub(super) fn with_installed<T>(
        &self,
        run: impl FnOnce(&File) -> io::Result<T>,
    ) -> io::Result<T> {
        self.require_original()?;
        let bin = Pinned::local_storage(Path::new(BIN), &self.selected.scope)?;
        let original_dir = bin.relative_directory()?;
        let leaf = OsStr::new("zxs");
        let original = same_mount_leaf(&original_dir, leaf)?;
        let executable = installed_regular_reader(&original)?;
        let stamp = |info: &Metadata| {
            (
                info.dev(),
                info.ino(),
                info.mode(),
                info.uid(),
                info.len(),
                info.mtime(),
                info.mtime_nsec(),
                info.ctime(),
                info.ctime_nsec(),
                info.nlink(),
            )
        };
        let meta = original.metadata()?;
        let exact = stamp(&meta);
        let verify = || {
            self.require_original()?;
            let mount = bin.same_local_chain(|dir, _| {
                let info = dir.metadata()?;
                if info.uid() != 0 || info.mode() & 0o022 != 0 {
                    return Err(invalid());
                }
                Ok(())
            })?;
            let named = same_mount_leaf(&original_dir, leaf)?;
            if Entity::of(&original_dir.metadata()?)? != bin.entity()
                || Mount::on(&original_dir)? != Some(mount)
                || Mount::on(&original)? != Some(mount)
                || Mount::on(&executable)? != Some(mount)
                || Mount::on(&named)? != Some(mount)
                || meta.mode() & 0o111 == 0
                || stamp(&named.metadata()?) != exact
                || stamp(&original.metadata()?) != exact
                || stamp(&executable.metadata()?) != exact
            {
                return Err(invalid());
            }
            Ok(())
        };
        verify()?;
        let result = run(&executable)?;
        verify()?;
        Ok(result)
    }
}
