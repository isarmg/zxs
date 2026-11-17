//! VFS module publication in fixed existing system directories.
use super::*;
use crate::installation::vfs_payload::MODULE_PATH;

pub(crate) struct VfsDestination {
    destination: ProgramDestination,
}
impl VfsDestination {
    pub(crate) fn open() -> Result<Self> {
        Ok(Self {
            destination: ProgramDestination::fixed(
                Path::new(MODULE_PATH),
                FixedProgramParents::vfs()?,
            )?,
        })
    }
    pub(crate) fn recheck(&self) -> Result<()> {
        self.destination.recheck_parent()
    }
    pub(crate) fn parent_identity(&self) -> FileIdentity {
        self.destination.parent_identity()
    }
    pub(crate) fn matches_or_absent(&self, bytes: &[u8]) -> Result<bool> {
        self.destination.matches_or_absent(bytes)
    }
    pub(crate) fn capture(&self, bytes: &[u8]) -> Result<Option<HeldProgram>> {
        self.destination.matching_file(bytes)
    }
    pub(crate) fn require_held(&self, program: &HeldProgram) -> Result<()> {
        self.destination.require_held(program)
    }
    pub(crate) fn install_captured(
        &self,
        bytes: &[u8],
        captured: Option<HeldProgram>,
        complete: bool,
    ) -> Result<(bool, HeldProgram)> {
        self.destination.install_captured(bytes, captured, complete)
    }
}
