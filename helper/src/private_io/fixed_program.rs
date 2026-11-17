//! Live proof of the complete declared system-directory chain. Reopening only
//! the last named parent would miss a substituted ancestor which carried the
//! original last directory (and original executable) into a different tree.
use super::{EntryName, File, MetadataExt, MountPolicy, OpenAt2Root, Path, Result};
use std::path::PathBuf;

pub(crate) struct FixedProgramParents {
    base_path: PathBuf,
    named_path: PathBuf,
    base: OpenAt2Root,
    descendants: Vec<(EntryName, OpenAt2Root)>,
    owner: u32,
}
fn trusted(file: &File, owner: u32) -> Result<()> {
    let metadata = file.metadata()?;
    if !metadata.is_dir() || metadata.uid() != owner || metadata.mode() & 0o022 != 0 {
        return Err("a fixed executable ancestor is not the expected privately administered directory; no permissions were changed".into());
    }
    Ok(())
}
impl FixedProgramParents {
    pub(super) fn executable() -> Result<Self> {
        Self::capture(Path::new("/usr"), &["local", "bin"], 0)
    }
    pub(crate) fn vfs() -> Result<Self> {
        Self::capture(
            Path::new("/usr"),
            &["lib", "x86_64-linux-gnu", "samba", "vfs"],
            0,
        )
    }
    fn capture(base_path: &Path, parts: &[&str], owner: u32) -> Result<Self> {
        let base = OpenAt2Root::open(base_path, MountPolicy::AllowNestedMounts)?;
        trusted(base.directory(), owner)?;
        let mut result = Self {
            base_path: base_path.to_owned(),
            named_path: base_path.to_owned(),
            base,
            descendants: Vec::with_capacity(parts.len()),
            owner,
        };
        for part in parts {
            let name = EntryName::new(part)?;
            let child = result.leaf().open_directory(&name.as_relative())?;
            trusted(&child, owner)?;
            let opened = OpenAt2Root::from_directory(child, MountPolicy::AllowNestedMounts)?;
            result.named_path.push(name.as_os_str());
            result.descendants.push((name, opened));
        }
        result.require_current()?;
        Ok(result)
    }
    pub(super) fn path(&self) -> &Path {
        &self.named_path
    }
    pub(crate) fn leaf(&self) -> &OpenAt2Root {
        self.descendants.last().map_or(&self.base, |(_, held)| held)
    }
    pub(crate) fn require_current(&self) -> Result<()> {
        let current = OpenAt2Root::open(&self.base_path, MountPolicy::AllowNestedMounts)?;
        if current.identity() != self.base.identity() {
            return Err(
                "fixed executable root now names a different original; preserve both directories"
                    .into(),
            );
        }
        trusted(self.base.directory(), self.owner)?;
        let mut parent = &self.base;
        for (name, child) in &self.descendants {
            parent.verify_entry_identity(name, child.identity())?;
            trusted(child.directory(), self.owner)?;
            parent = child;
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../test/helper/unit/private_io/fixed_program_parents.rs"]
mod tests;
