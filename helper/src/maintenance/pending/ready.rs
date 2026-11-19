//! Read-only evidence at the first published frontier. No volume path from the
//! register is opened, no parent sync or repair occurs, and no current FILE,
//! account, service status or operator authorization follows from this result.
use super::{ColdPending, FreshGenesis, InitialImage, ObservedOriginal, RegisterName, Source};
use crate::pinned::invalid;
use std::{io, path::Path};

#[must_use]
pub(in crate::maintenance) struct ColdEmptyReady { source: Source, observed: InitialImage }

impl ColdEmptyReady {
    pub(in crate::maintenance) fn inspect(system: &Path, storage_uid: u32, op: [u8; 16]) -> io::Result<Self> {
        let (source, observed) = Source::open(system, storage_uid, op, RegisterName::Ready, None)?;
        Ok(Self { source, observed })
    }

    // Move the existing (cold or warm) EX description, never reacquire a lock
    // under a name. This returns only the same read-only evidence as inspect:
    // rename and parent synchronization are separately proved by its caller.
    pub(in crate::maintenance) fn read_renamed(original: ColdPending) -> io::Result<Self> {
        let ColdPending { mut source, observed } = original;
        if source.known != RegisterName::Pending || !observed.system_labeled {
            return Err(invalid("expected the original labelled pending registration"));
        }
        source.known = RegisterName::Ready;
        let ready = Self { source, observed };
        ready.recheck()?;
        Ok(ready)
    }

    pub(in crate::maintenance) fn original_image(&self) -> io::Result<FreshGenesis<'_>> {
        self.recheck()?;
        FreshGenesis::decode(&self.observed.bytes).map_err(|_| invalid("not the observed empty registration"))
    }
    pub(in crate::maintenance) fn storage_uid(&self) -> u32 { self.source.uid }
    pub(in crate::maintenance) fn same_system(&self, root: &ObservedOriginal) -> bool { root == &self.source.s }

    pub(in crate::maintenance) fn recheck(&self) -> io::Result<()> {
        if self.source.read_original()? != self.observed {
            return Err(invalid("original published empty registration changed since observation"));
        }
        Ok(())
    }
}
