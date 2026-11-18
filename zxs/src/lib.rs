//! Standalone ZXS storage. Fresh-selection inspection is read-only: it returns
//! what was observed, not a capability to register, publish or write later.
mod fs;
pub use fs::Entity;
use fs::{HeldDir, invalid};
use std::{collections::BTreeSet, io, path::{Path, PathBuf}};

pub const MAX_FILE_ROOTS: usize = 62;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role { System, File, Recycle }

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ObservedVolume {
    pub role: Role,
    pub path: PathBuf,
    pub entity: Entity,
}

struct Selection { roots: Vec<(Role,HeldDir)> }

impl Selection {
    fn open(system: &Path, recycle: &Path, files: &[PathBuf]) -> io::Result<Self> {
        if files.is_empty() || files.len() > MAX_FILE_ROOTS {
            return Err(invalid("select one or more FILE roots, within the registration limit"));
        }
        let mut roots = Vec::with_capacity(files.len() + 2);
        roots.push((Role::System,HeldDir::open(system)?));
        roots.push((Role::Recycle,HeldDir::open(recycle)?));
        for file in files { roots.push((Role::File,HeldDir::open(file)?)); }
        let entities: BTreeSet<_> = roots.iter().map(|(_,d)| d.entity).collect();
        if entities.len() != roots.len() { return Err(invalid("multiple roles or names select the same directory")); }
        let recycle_device = roots[1].1.entity.device;
        if roots.iter().any(|(r,d)| *r != Role::Recycle && d.entity.device == recycle_device) {
            return Err(invalid("recycle storage must use its own filesystem"));
        }
        for (_,dir) in &roots { dir.reject_registered_ancestors(&entities)?; }
        Ok(Self { roots })
    }
    fn view(&self) -> Vec<ObservedVolume> {
        self.roots.iter().map(|(role,d)| ObservedVolume { role:*role, path:d.path.clone(), entity:d.entity }).collect()
    }
}

pub fn inspect_fresh(system: &Path, recycle: &Path, files: &[PathBuf]) -> io::Result<Vec<ObservedVolume>> {
    let selection = Selection::open(system,recycle,files)?;
    // flock acts on live directory FDs without creating sidecars. Nonblocking
    // failure is not a reason to use a different root or drop an operand.
    for (_,dir) in &selection.roots { dir.share()?; }
    for (_,dir) in &selection.roots { dir.require_named()?; dir.require_fresh()?; }
    for (_,dir) in &selection.roots { dir.require_named()?; }
    Ok(selection.view())
}

#[cfg(test)]
#[path = "../tests/unit/fresh.rs"]
mod fresh_tests;
