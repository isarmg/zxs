//! Original read-only SYSTEM database and marker evidence across the paired cut.
use super::{
    bad,
    transport::{Channel, Kind},
};
use crate::sqlite_install::{markers::SystemObservation, pair::fresh_issuer::InitialDatabase};
use std::{fs::File, io, time::Instant};
fn wrong() -> io::Error {
    bad("the original SYSTEM observation was not confirmed")
}
pub(super) fn object_body(stamp: [u8; 64]) -> [u8; 65] {
    let mut frame = [0; 65];
    frame[0] = 1;
    frame[1..].copy_from_slice(&stamp);
    frame
}
pub(super) fn read_object_body(frame: [u8; 65]) -> io::Result<[u8; 64]> {
    if frame[0] != 1 {
        return Err(wrong());
    }
    Ok(frame[1..].try_into().expect("fixed observation stamp"))
}
pub(super) struct Objects {
    db: InitialDatabase,
    marker: SystemObservation,
    pub(super) initial: [u8; 64],
    original_mark: [u8; 64],
}
impl Objects {
    pub(super) fn received(
        channel: &Channel,
        scope: &[u8; 32],
        until: Instant,
        system: &File,
        uid: u32,
        check: &impl Fn() -> io::Result<()>,
    ) -> io::Result<Self> {
        let (frame, [fd]) =
            channel.wait_checked::<1>(Kind::DeployedDatabase, scope, until, check)?;
        let db = InitialDatabase::received(fd, uid)?;
        let initial = read_object_body(frame)?;
        let (frame, [fd]) = channel.wait_checked::<1>(Kind::DeployedMarker, scope, until, check)?;
        let marker = SystemObservation::received(fd, system, uid)?;
        let result = Self {
            db,
            marker,
            initial,
            original_mark: read_object_body(frame)?,
        };
        result.require(system, &initial)?;
        Ok(result)
    }
    pub(super) fn require_database(&self, system: &File, original: [u64; 3]) -> io::Result<()> {
        if self.db.named(system)? != original {
            return Err(wrong());
        }
        Ok(())
    }
    pub(super) fn require(&self, system: &File, db: &[u8; 64]) -> io::Result<()> {
        self.db.named(system)?;
        if self.db.observation()? != *db || self.marker.require(system)? != self.original_mark {
            return Err(wrong());
        }
        Ok(())
    }
}
