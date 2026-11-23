//! In-memory witnesses for completed staged entries with no permanently open FDs.
use super::{FlatStage, invalid};
use crate::fs::{Entity, HeldDir, OPAQUE_STAMP_BYTES};
use std::{
    ffi::{CStr, CString},
    fs::Metadata,
    io,
    os::unix::fs::MetadataExt,
};

pub(in crate::container_store) struct SealedStage {
    name: CString,
    identity: Entity,
    modified: [i64; 4],
    files: Vec<(&'static CStr, [u8; OPAQUE_STAMP_BYTES])>,
}

impl SealedStage {
    pub(super) fn capture(stage: &FlatStage<'_>) -> io::Result<Self> {
        let files = stage
            .files
            .iter()
            .map(|(name, file)| {
                let pin = stage
                    .current_file(name, file)?
                    .ok_or_else(|| invalid("completed staging record is absent"))?;
                Ok((*name, pin.stamp()))
            })
            .collect::<io::Result<_>>()?;
        Ok(Self {
            name: stage.name.clone(),
            identity: stage.directory.entity,
            modified: times(&stage.directory.file.metadata()?),
            files,
        })
    }

    pub(in crate::container_store) fn clean(&self, parent: &HeldDir) -> io::Result<()> {
        let Some(original) = parent.child_directory(&self.name)? else {
            return Ok(());
        };
        if original.entity != self.identity || times(&original.file.metadata()?) != self.modified {
            return Err(invalid("completed staging directory changed"));
        }
        // Qualify both expected records before unlinking either. Additional
        // unrecognized contents are never traversed or unlinked.
        let files = self
            .files
            .iter()
            .map(|(name, expected)| {
                let pin = original
                    .opaque_pin(name)?
                    .ok_or_else(|| invalid("completed staging record lost its name"))?;
                if &pin.stamp() != expected {
                    return Err(invalid("completed staging record changed"));
                }
                Ok((*name, pin))
            })
            .collect::<io::Result<Vec<_>>>()?;
        for (name, pin) in &files {
            original.unlink_opaque_original(name, pin)?;
        }
        parent.unlink_empty_opaque_original(&self.name, &original)
    }
}

fn times(meta: &Metadata) -> [i64; 4] {
    [
        meta.mtime(),
        meta.mtime_nsec(),
        meta.ctime(),
        meta.ctime_nsec(),
    ]
}
