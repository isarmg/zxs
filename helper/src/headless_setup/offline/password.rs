//! Replace the single verifier only after the original roster is held durably.
use super::{
    DESCRIPTOR, EPOCH, LOGIN, OfflineSession, SALT, SCOPED, TAG, random, read, refused, stamp,
};
use crate::{
    fresh_password,
    headless_setup::{IssueError, fresh_verifier, stage_name},
    pinned::Entity,
};
use rustix::fs::{Mode, OFlags, fchmod, openat2, renameat};
use std::{
    fs::File,
    io::{self, Write},
};
use subtle::ConstantTimeEq;
use zeroize::Zeroizing;

impl OfflineSession {
    pub(crate) fn reset_password(self, secret: &str, repeated: &str) -> Result<(), IssueError> {
        if !fresh_password::admissible(secret)
            || !fresh_password::admissible(repeated)
            || !bool::from(secret.as_bytes().ct_eq(repeated.as_bytes()))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "the two valid account password entries must match",
            )
            .into());
        }
        self.require_original()?;
        let epoch = random::<16>()?;
        if epoch == self.epoch || epoch == self.identity.pool {
            return Err(refused().into());
        }
        let salt = random::<16>()?;
        let tag = fresh_verifier(secret, &salt)?;
        if *tag == [0; 32] {
            return Err(refused().into());
        }
        let mut updated = Zeroizing::new(self.original.full.to_vec());
        updated[EPOCH..EPOCH + 16].copy_from_slice(&epoch);
        updated[SALT..TAG].copy_from_slice(&salt);
        updated[TAG..LOGIN].copy_from_slice(&tag[..]);
        let stage = stage_name(&epoch);

        let commit = || -> io::Result<()> {
            let mut candidate = File::from(openat2(
                &self.locked,
                stage.as_str(),
                OFlags::RDWR | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC,
                Mode::RUSR | Mode::WUSR,
                SCOPED,
            )?);
            fchmod(&candidate, Mode::RUSR | Mode::WUSR)?;
            candidate.write_all(&updated)?;
            candidate.sync_all()?;
            let prepared = read(&self.locked, &stage, self.selected.uid)?;
            let expected = stamp(&candidate.metadata()?);
            if stamp(&prepared.meta) != expected || prepared.full.as_slice() != updated.as_slice() {
                return Err(refused());
            }
            self.require_original()?;
            if stamp(&candidate.metadata()?) != expected
                || stamp(&read(&self.locked, &stage, self.selected.uid)?.meta) != expected
            {
                return Err(refused());
            }
            // One intentional overwrite of the proved current descriptor.
            // The state EX excludes Core/Helper adoption throughout; a name
            // changed outside the dedicated service owner is not supported.
            renameat(&self.locked, stage.as_str(), &self.locked, DESCRIPTOR)?;
            self.locked.sync_all()?;
            self.selected.require_site()?;
            self.selected
                .issue(&self.login, &self.identity)?
                .require_roots(true)?;
            let selected = read(&self.locked, DESCRIPTOR, self.selected.uid)?;
            if Entity::of(&selected.meta)? != Entity::of(&candidate.metadata()?)?
                || selected.full.as_slice() != updated.as_slice()
            {
                return Err(refused());
            }
            Ok(())
        };
        commit().map_err(IssueError::Uncertain)
    }
}
