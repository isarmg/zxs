//! Linux download publication stays relative to the originally selected directory.
//! A private staging directory prevents another user with access to the output
//! parent from replacing the verified temporary file before publication.
use rustix::fs::{AtFlags, FileType, Mode, OFlags, fstat, mkdirat, open, openat, statat, unlinkat};
use std::os::fd::AsRawFd;
use std::{ffi::OsString, fs::File, io, path::Path};

pub(super) struct Target {
    directory: File,
    name: OsString,
}

impl Target {
    pub(super) fn open(path: &Path) -> io::Result<Self> {
        let name = path
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "invalid download target"))?
            .to_owned();
        // Path::file_name normalizes trailing separators and "/.". Do not
        // reinterpret a directory spelling as a different child filename.
        if !path
            .as_os_str()
            .as_encoded_bytes()
            .ends_with(name.as_encoded_bytes())
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid download target",
            ));
        }
        let parent = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
            .unwrap_or_else(|| Path::new("."));
        // Local aliases are allowed, but resolved only once, before networking.
        let directory = File::from(open(
            parent,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )?);
        match statat(&directory, &name, AtFlags::SYMLINK_NOFOLLOW) {
            Ok(_) => {
                return Err(io::Error::new(
                    io::ErrorKind::AlreadyExists,
                    "download destination already exists",
                ));
            }
            Err(rustix::io::Errno::NOENT) => {}
            Err(error) => return Err(error.into()),
        }
        Ok(Self { directory, name })
    }

    pub(super) fn stage(self) -> io::Result<(File, Stage)> {
        // tempfile supplies bounded random-name retries only. Its pathname
        // cleanup is disabled: all filesystem work uses our held descriptors.
        let temporary = tempfile::Builder::new()
            .prefix(".zxs-download-")
            .disable_cleanup(true)
            .make_in(".", |path| {
                let name = path.file_name().expect("tempfile supplies an entry name");
                mkdirat(&self.directory, name, Mode::from_raw_mode(0o700))?;
                // If opening the newly created directory fails, no held
                // identity exists for safe cleanup. Retain a possible empty
                // directory rather than unlink an unverified replacement.
                let directory = File::from(openat(
                    &self.directory,
                    name,
                    OFlags::RDONLY | OFlags::DIRECTORY | OFlags::NOFOLLOW | OFlags::CLOEXEC,
                    Mode::empty(),
                )?);
                let stat = fstat(&directory)?;
                if stat.st_uid != rustix::process::geteuid().as_raw()
                    || stat.st_mode & 0o777 != 0o700
                {
                    return Err(io::Error::new(
                        io::ErrorKind::PermissionDenied,
                        "unsafe download staging directory",
                    ));
                }
                Ok(directory)
            })?;
        let (directory, path) = temporary.into_parts();
        let mut stage = Stage {
            target: self,
            directory,
            name: path
                .file_name()
                .expect("tempfile supplies an entry name")
                .to_owned(),
            payload: None,
            cleanup_pending: true,
        };
        let file = File::from(openat(
            &stage.directory,
            "payload",
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )?);
        stage.payload = Some(file);
        // Retain the exact inode that the caller will write and verify. If
        // duplication fails, Stage already owns cleanup of the new payload.
        let file = stage
            .payload
            .as_ref()
            .expect("payload was installed")
            .try_clone()?;
        Ok((file, stage))
    }
}

pub(super) struct Stage {
    target: Target,
    directory: File,
    name: OsString,
    payload: Option<File>,
    cleanup_pending: bool,
}

impl Stage {
    pub(super) fn publish(mut self) -> io::Result<()> {
        // AT_EMPTY_PATH requires capabilities for ordinary files. The
        // procfs descriptor alias provides the same inode-bound link for
        // unprivileged callers. Do not fall back to a mutable source name.
        let payload = self.payload.as_ref().expect("payload was installed");
        rustix::fs::linkat(
            rustix::fs::CWD,
            format!("/proc/self/fd/{}", payload.as_raw_fd()),
            &self.target.directory,
            &self.target.name,
            AtFlags::SYMLINK_FOLLOW,
        )?;
        self.target.directory.sync_all()?;
        self.cleanup();
        Ok(())
    }

    fn remove_payload(&self) -> io::Result<()> {
        let Some(payload) = &self.payload else {
            return Ok(());
        };
        let named = match statat(&self.directory, "payload", AtFlags::SYMLINK_NOFOLLOW) {
            Ok(named) => named,
            Err(rustix::io::Errno::NOENT) => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        let held = fstat(payload)?;
        if (held.st_dev, held.st_ino) == (named.st_dev, named.st_ino) {
            unlinkat(&self.directory, "payload", AtFlags::empty())?;
        }
        Ok(())
    }
}

impl Stage {
    fn cleanup(&mut self) {
        if !self.cleanup_pending {
            return;
        }
        self.cleanup_pending = false;
        // Only remove the directory if its name still refers to our held inode.
        let cleanup = (|| -> io::Result<()> {
            self.remove_payload()?;
            let held = fstat(&self.directory)?;
            let named = match statat(
                &self.target.directory,
                &self.name,
                AtFlags::SYMLINK_NOFOLLOW,
            ) {
                Ok(named) => named,
                Err(rustix::io::Errno::NOENT) => return Ok(()),
                Err(error) => return Err(error.into()),
            };
            if FileType::from_raw_mode(named.st_mode) == FileType::Directory
                && (held.st_dev, held.st_ino) == (named.st_dev, named.st_ino)
            {
                match unlinkat(&self.target.directory, &self.name, AtFlags::REMOVEDIR) {
                    Ok(()) | Err(rustix::io::Errno::NOENT) => {}
                    Err(error) => return Err(error.into()),
                }
            }
            Ok(())
        })();
        if let Err(error) = cleanup {
            tracing::warn!(error = ?error, "download staging directory retained after cleanup failure");
        }
    }
}

impl Drop for Stage {
    fn drop(&mut self) {
        self.cleanup();
    }
}

#[cfg(test)]
#[path = "../../../test/client/unit/download_target_tests.rs"]
mod tests;
