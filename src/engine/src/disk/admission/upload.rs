//! One revocable owner for upload directory, source, verified inode and cleanup.
use super::{DiskFile, DiskIoResource};
use crate::{
    ContentHash, Error, ErrorCode, Result,
    disk::{DiskRoot, FullCopyProgress, full_copy_held_observed, remove_owned_file},
    file_stamp::FileStamp,
};
use platform_io::fs_safety::{
    EntryName, RelativePath,
    linux::{FileIdentity, MountPolicy, OpenAt2Root},
};
use std::{fs::File, os::unix::ffi::OsStrExt, path::Path};

pub(crate) struct UploadIo {
    resources: DiskIoResource<UploadResources>,
}

struct UploadResources {
    directory: OpenAt2Root,
    // A caller-supplied file is not inferred to belong to the target disk. This
    // owner closes only its transferred descriptor, never its external pathname.
    source: UploadSource,
    scratch: Option<UploadScratch>,
    verified: Option<VerifiedUploadFile>,
}

/// The original managed lease stays with the source; no untracked descriptor
/// clone may outlive revocation of the disk that supplied the draft body.
pub(crate) enum UploadSource {
    Local(File),
    Managed(DiskFile),
}

impl UploadSource {
    fn with_file<T>(&self, inspect: impl FnOnce(&File) -> Result<T>) -> Result<T> {
        match self {
            Self::Local(file) => inspect(file),
            Self::Managed(file) => {
                let mut access = file.access_checked()?;
                inspect(&access.value().resource)
            }
        }
    }

    fn check_running(&self) -> Result<()> {
        match self {
            Self::Local(_) => Ok(()),
            Self::Managed(file) => file.check_running(),
        }
    }

    pub(crate) fn admitted_length(&self) -> Result<u64> {
        self.with_file(|file| {
            platform_io::fs_safety::linux::SingleLinkRequirement::check(file)
                .map_err(preparation_fs_error)?;
            Ok(file.metadata()?.len())
        })
    }
}

struct UploadScratch {
    name: EntryName,
    identity: FileIdentity,
}

struct VerifiedUploadFile {
    file: File,
    stamp: FileStamp,
}

pub(crate) enum UploadFinish {
    Replay,
    Publish {
        max_workspace_bytes: u64,
        max_entries: usize,
    },
}

fn preparation_fs_error(error: platform_io::fs_safety::Error) -> Error {
    let message = error.to_string();
    Error::from(error).context(ErrorCode::UnsafePath, message)
}

impl UploadIo {
    pub(crate) fn new(root: &DiskRoot, path: &Path, source: File) -> Result<Self> {
        Self::with_source(root, path, UploadSource::Local(source))
    }

    pub(crate) fn with_source(
        root: &DiskRoot,
        path: &Path,
        source: UploadSource,
    ) -> Result<Self> {
        source.check_running()?;
        let lease = root.admit_io()?;
        let directory = OpenAt2Root::from_private_directory(
            root.private_directory(path)?,
            MountPolicy::SameMount,
        )
        .map_err(preparation_fs_error)?;
        let resources = lease.hold(UploadResources {
            directory,
            source,
            scratch: None,
            verified: None,
        });
        resources.check_running()?;
        Ok(Self { resources })
    }

    pub(crate) fn target_exists(&self, name: &EntryName) -> Result<bool> {
        let mut access = self.resources.access_checked()?;
        let relative =
            RelativePath::new(Path::new(name.as_os_str())).map_err(preparation_fs_error)?;
        match access
            .value()
            .resource
            .directory
            .open_file(&relative, false)
        {
            Ok(_) => Ok(true),
            Err(platform_io::fs_safety::Error::Io(error))
                if error.kind() == std::io::ErrorKind::NotFound =>
            {
                Ok(false)
            }
            Err(error) => Err(preparation_fs_error(error)),
        }
    }

    pub(crate) fn retained_usage(&self) -> Result<(u64, usize)> {
        let mut access = self.resources.access_checked()?;
        access
            .value()
            .resource
            .retained_usage(&mut || self.resources.check_running())
    }

    pub(crate) fn fingerprint_source(&self, maximum: u64) -> Result<(u64, ContentHash)> {
        let mut access = self.resources.access_checked()?;
        let source = &access.value().resource.source;
        source.with_file(|file| fingerprint(file, maximum, &mut || {
            source.check_running()?;
            self.resources.check_running()
        }))
    }

    pub(crate) fn verify_retained(
        &self,
        name: &EntryName,
        size: u64,
        hash: &ContentHash,
        maximum: u64,
    ) -> Result<()> {
        let mut access = self.resources.access_checked()?;
        let resources = &mut access.value().resource;
        let file = resources.open(name)?;
        let stamp = FileStamp::of(&file)?;
        let actual = fingerprint(&file, maximum, &mut || self.resources.check_running())?;
        self.resources.check_running()?;
        if actual.0 != size || actual.1 != *hash || FileStamp::of(&file)? != stamp {
            return Err(Error::new(
                ErrorCode::ChecksumMismatch,
                "retained upload generation no longer matches its receipt",
            ));
        }
        resources.verified = Some(VerifiedUploadFile { file, stamp });
        Ok(())
    }

    pub(crate) fn copy(
        &self,
        scratch: EntryName,
        chunk_bytes: u32,
        maximum: u64,
    ) -> Result<(u64, ContentHash)> {
        let mut access = self.resources.access_checked()?;
        access
            .value()
            .resource
            .copy(scratch, chunk_bytes, maximum, &mut || {
                self.resources.check_running()
            })
    }

    pub(crate) fn finish(
        &self,
        target: &EntryName,
        current_directory: FileIdentity,
        finish: UploadFinish,
    ) -> Result<()> {
        let mut access = self.resources.access_checked()?;
        access
            .value()
            .resource
            .finish(target, current_directory, finish, &mut || {
                self.resources.check_running()
            })
    }
}

impl UploadResources {
    fn open(&self, name: &EntryName) -> Result<File> {
        let relative =
            RelativePath::new(Path::new(name.as_os_str())).map_err(preparation_fs_error)?;
        self.directory
            .open_file(&relative, false)
            .map_err(preparation_fs_error)
    }

    fn retained_usage(&self, check: &mut impl FnMut() -> Result<()>) -> Result<(u64, usize)> {
        let mut retained = 0u64;
        let mut generations = 0usize;
        check()?;
        let entries =
            rustix::fs::Dir::read_from(self.directory.directory()).map_err(std::io::Error::from)?;
        for entry in entries {
            check()?;
            let entry = entry.map_err(std::io::Error::from)?;
            let path = Path::new(std::ffi::OsStr::from_bytes(entry.file_name().to_bytes()));
            if path
                .extension()
                .is_some_and(|ext| ext == "full" || ext == "prepared")
                || entry.file_name().to_bytes().starts_with(b".zxs-full-")
            {
                generations = generations.checked_add(1).ok_or_else(|| {
                    Error::new(
                        ErrorCode::ResourceLimit,
                        "retained generation count overflow",
                    )
                })?;
                let relative = RelativePath::new(path).map_err(preparation_fs_error)?;
                let size = self
                    .directory
                    .open_file(&relative, false)
                    .map_err(preparation_fs_error)?
                    .metadata()?
                    .len();
                retained = retained.checked_add(size).ok_or_else(|| {
                    Error::new(
                        ErrorCode::ResourceLimit,
                        "retained workspace byte count overflow",
                    )
                })?;
            }
        }
        check()?;
        Ok((retained, generations))
    }

    fn copy(
        &mut self,
        scratch_name: EntryName,
        chunk_bytes: u32,
        maximum: u64,
        check: &mut impl FnMut() -> Result<()>,
    ) -> Result<(u64, ContentHash)> {
        let scratch = &mut self.scratch;
        let source = &self.source;
        let (size, hash, _, file) = source.with_file(|file| full_copy_held_observed(
            file,
            &self.directory,
            &scratch_name,
            chunk_bytes,
            maximum,
            &mut |progress| {
                match progress {
                    FullCopyProgress::Check => (),
                    FullCopyProgress::Created { name, identity } => {
                        *scratch = Some(UploadScratch { name, identity });
                    }
                    FullCopyProgress::Published => {
                        scratch
                            .as_mut()
                            .expect("invariant: copied scratch owns its created inode")
                            .name = scratch_name.clone();
                    }
                }
                source.check_running()?;
                check()
            },
        ))?;
        source.check_running()?;
        check()?;
        self.verified = Some(VerifiedUploadFile {
            stamp: FileStamp::of(&file)?,
            file,
        });
        Ok((size, hash))
    }

    fn finish(
        &mut self,
        target: &EntryName,
        current_directory: FileIdentity,
        finish: UploadFinish,
        check: &mut impl FnMut() -> Result<()>,
    ) -> Result<()> {
        if current_directory != self.directory.identity() {
            return Err(Error::new(
                ErrorCode::UnsafePath,
                "workspace data directory identity changed during preparation",
            ));
        }
        check()?;
        let verified = self
            .verified
            .as_ref()
            .expect("invariant: executed upload owns its verified file");
        let current = self.open(
            self.scratch
                .as_ref()
                .map_or(target, |scratch| &scratch.name),
        )?;
        if FileStamp::of(&verified.file)? != verified.stamp
            || FileStamp::of(&current)? != verified.stamp
        {
            return Err(Error::new(
                ErrorCode::ChecksumMismatch,
                "upload generation changed during preparation",
            ));
        }
        drop(current);
        let UploadFinish::Publish {
            max_workspace_bytes,
            max_entries,
        } = finish
        else {
            return check();
        };
        let (retained, generations) = self.retained_usage(check)?;
        if retained > max_workspace_bytes || generations > max_entries {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "retained generations exceed the workspace byte or count budget",
            ));
        }
        if let Some(scratch) = &self.scratch {
            check()?;
            rustix::fs::renameat_with(
                self.directory.directory(),
                scratch.name.as_os_str(),
                self.directory.directory(),
                target.as_os_str(),
                rustix::fs::RenameFlags::NOREPLACE,
            )
            .map_err(std::io::Error::from)?;
            self.scratch = None;
        }
        check()?;
        self.directory.directory().sync_all()?;
        check()
    }
}

impl Drop for UploadResources {
    fn drop(&mut self) {
        let Some(scratch) = &self.scratch else { return };
        // Cleanup is part of this resource's already admitted tail, before its
        // directory and file descriptors close and ordinary responsibility ends.
        if let Err(error) =
            remove_owned_file(self.directory.directory(), &scratch.name, scratch.identity)
        {
            tracing::error!(%error, "upload scratch cleanup failed");
        }
    }
}

fn fingerprint(
    input: &File,
    maximum: u64,
    check: &mut impl FnMut() -> Result<()>,
) -> Result<(u64, ContentHash)> {
    let mut digest = blake3::Hasher::new();
    let mut size = 0u64;
    let mut buffer = [0u8; 65536];
    loop {
        check()?;
        let n = std::os::unix::fs::FileExt::read_at(input, &mut buffer, size)?;
        if n == 0 {
            break;
        }
        size = size
            .checked_add(n as u64)
            .ok_or_else(|| Error::new(ErrorCode::ResourceLimit, "upload length overflow"))?;
        if size > maximum {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "upload grew beyond file budget",
            ));
        }
        digest.update(&buffer[..n]);
    }
    check()?;
    Ok((size, ContentHash::from_digest(digest.finalize())))
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/disk/upload_resources_tests.rs"]
mod tests;
