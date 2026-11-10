use super::*;
use crate::installation::vfs_payload::{MODULE_DIRECTORY, MODULE_PATH};
use platform_io::fs_safety::RelativePath;

const PARTS: &[&str] = &["lib", "x86_64-linux-gnu", "samba", "vfs"];

pub(crate) struct VfsDestination {
    parents: Vec<OpenAt2Root>,
    destination: ProgramDestination,
}
fn parents() -> Result<Vec<OpenAt2Root>> {
    let mut held = vec![OpenAt2Root::open(
        Path::new("/usr"),
        MountPolicy::AllowNestedMounts,
    )?];
    for part in PARTS {
        let parent = held.last().expect("invariant: /usr is held");
        let child = parent.open_directory(&RelativePath::new(part)?)?;
        held.push(OpenAt2Root::from_directory(
            child,
            MountPolicy::AllowNestedMounts,
        )?);
    }
    for parent in &held {
        let metadata = parent.directory().metadata()?;
        if metadata.uid() != 0 || metadata.mode() & 0o022 != 0 {
            return Err("every fixed VFS parent must be root-owned and not writable by other users; no permissions were changed".into());
        }
    }
    Ok(held)
}
impl VfsDestination {
    pub(crate) fn open() -> Result<Self> {
        let parents = parents()?;
        let destination = ProgramDestination::open(Path::new(MODULE_PATH))?;
        let result = Self {
            parents,
            destination,
        };
        result.recheck()?;
        Ok(result)
    }
    pub(crate) fn recheck(&self) -> Result<()> {
        let current = parents()?;
        if current
            .iter()
            .zip(&self.parents)
            .any(|(named, held)| named.identity() != held.identity())
            || self.destination.parent_identity()
                != current
                    .last()
                    .expect("invariant: fixed VFS parent chain")
                    .identity()
        {
            return Err(format!("fixed {MODULE_DIRECTORY} identity changed; preserve all module publication evidence").into());
        }
        self.destination.recheck_parent()
    }
    pub(crate) fn parent_identity(&self) -> FileIdentity {
        self.destination.parent_identity()
    }
    pub(crate) fn matches_or_absent(&self, bytes: &[u8]) -> Result<bool> {
        self.recheck()?;
        self.destination.matches_or_absent(bytes)
    }
    pub(crate) fn install(&self, bytes: &[u8]) -> Result<bool> {
        self.recheck()?;
        let created = self.destination.install(bytes)?;
        self.recheck()?;
        if !self.destination.matches_or_absent(bytes)? {
            return Err(
                "VFS module disappeared after publication; no completion is claimed".into(),
            );
        }
        Ok(created)
    }
}
