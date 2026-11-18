//! Names retained from one verified same-parent transition, never a disk search.
use super::*;
use name_policy::LookupName;

pub(super) struct Rename {
    pub(super) source: LookupName,
}

pub(super) fn prepare(
    parent: &DiskRoot,
    change: &crate::engine::fresh_layout::containers::ContainerNameChange,
    entity: ContainerEntity,
) -> Result<(DiskRoot, Rename)> {
    let target = parent.path().join(layout::namespace::directory(&change.target().name).as_path());
    if parent.optional_metadata(&target)?.is_some() {
        return Err(Failure::ContainerIdentity.into());
    }
    let held = open_published_container(parent, change.source(), entity)?;
    Ok((held, Rename { source: change.source().clone() }))
}

/// The caller has verified the original predecessor/candidate pair. Only its
/// two names are candidates, and both/neither is always corruption or collision.
pub(super) fn recover(
    parent: &DiskRoot,
    old: &FileContainerRecord,
    next: &FileContainerRecord,
    entity: ContainerEntity,
    permit_target: bool,
) -> Result<(DiskRoot, Rename, bool)> {
    if !permit_target {
        let target = parent.path()
            .join(layout::namespace::directory(&next.namespace.name).as_path());
        if parent.optional_metadata(&target)?.is_some() {
            return Err(Failure::ContainerIdentity.into());
        }
        let held = open_published_container(parent, &old.namespace.name, entity)?;
        let binding = Rename { source: old.namespace.name.clone() };
        return Ok((held, binding, false));
    }
    recover_names(parent, &old.namespace.name, &next.namespace.name, entity)
}

pub(super) fn recover_names(
    parent: &DiskRoot,
    before_name: &LookupName,
    after_name: &LookupName,
    entity: ContainerEntity,
) -> Result<(DiskRoot, Rename, bool)> {
    let source = parent.path().join(layout::namespace::directory(before_name).as_path());
    let target = parent.path().join(layout::namespace::directory(after_name).as_path());
    let before = parent.optional_metadata(&source)?.is_some();
    let after = parent.optional_metadata(&target)?.is_some();
    let (name, installed) = match (before, after) {
        (true, false) => (before_name, false),
        (false, true) => (after_name, true),
        (true, true) | (false, false) => return Err(Failure::ContainerIdentity.into()),
    };
    let held = open_published_container(parent, name, entity)?;
    Ok((held, Rename { source: before_name.clone() }, installed))
}
