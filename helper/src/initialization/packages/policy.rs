//! Temporary Debian service-start policy, only for the retained install request.
use crate::Result;
use platform_io::{
    fs_safety::{
        EntryName, Error,
        linux::{FileIdentity, MountPolicy, OpenAt2Root, SingleLinkRequirement},
    },
    publication::write_atomic_at_mode,
};
use rustix::fs::{AtFlags, statat, unlinkat};
use std::{io::Read, os::unix::fs::MetadataExt, path::Path};

const PATH: &str = "/usr/sbin";
const NAME: &str = "policy-rc.d";
pub(super) struct Policy(OpenAt2Root);
impl Policy {
    pub fn open() -> Result<Self> {
        let root = OpenAt2Root::open(Path::new(PATH), MountPolicy::AllowNestedMounts)?;
        let metadata = root.directory().metadata()?;
        if metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
            return Err(
                "Debian policy directory must be root-owned and not writable by other users".into(),
            );
        }
        Ok(Self(root))
    }
    pub fn absent_or_original(&self, expected: Option<&[u8]>) -> Result<Option<FileIdentity>> {
        let name = EntryName::new(NAME)?;
        let file = match self.0.open_file(&name.as_relative(), false) {
            Ok(file) => file,
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(None);
            }
            Err(error) => return Err(error.into()),
        };
        let expected = expected.ok_or("an existing policy-rc.d belongs to the administrator; it was not replaced, install these packages under that policy outside the helper")?;
        let metadata = file.metadata()?;
        if metadata.uid() != 0
            || metadata.mode() & 0o7777 != 0o555
            || metadata.len() != expected.len() as u64
        {
            return Err("service-start policy differs from this original package request; preserve it for inspection".into());
        }
        let identity = SingleLinkRequirement::check(&file)?;
        let mut bytes = Vec::new();
        file.take(expected.len() as u64 + 1)
            .read_to_end(&mut bytes)?;
        if bytes != expected {
            return Err(
                "service-start policy is not the retained helper request; it was not removed"
                    .into(),
            );
        }
        Ok(Some(identity))
    }
    pub fn install(&self, expected: &[u8]) -> Result<()> {
        write_atomic_at_mode(&self.0, &EntryName::new(NAME)?, expected, false, 0o555)?;
        Ok(())
    }
    pub fn remove_original(&self, expected: &[u8]) -> Result<()> {
        let Some(identity) = self.absent_or_original(Some(expected))? else {
            self.0.directory().sync_all()?;
            return Ok(());
        };
        if OpenAt2Root::open(Path::new(PATH), MountPolicy::AllowNestedMounts)?.identity()
            != self.0.identity()
        {
            return Err(
                "service policy directory changed; the original policy was not removed".into(),
            );
        }
        let current = statat(self.0.directory(), NAME, AtFlags::SYMLINK_NOFOLLOW)?;
        if (current.st_dev, current.st_ino) != (identity.device, identity.inode) {
            return Err(
                "service policy changed before cleanup; replacement was not removed".into(),
            );
        }
        unlinkat(self.0.directory(), NAME, AtFlags::empty())?;
        self.0.directory().sync_all()?;
        Ok(())
    }
}
pub(super) fn contents(digest: &str) -> Vec<u8> {
    format!("#!/bin/sh\n# zxs-helper system packages {digest}\nexit 101\n").into_bytes()
}
