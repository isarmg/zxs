//! Existing-only live-name view under the original offline state lease.
use super::{DataRoot, Id, hex, invalid, recycle, replay_selected_entries, routes};
use crate::fs::{HeldDir, ReadAccess};
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::{CStr, CString},
    io,
};

const ACCESS: ReadAccess = ReadAccess::NoAtime;

struct Namespace<'a> {
    parent: &'a HeldDir,
    name: &'a CStr,
    selected: Option<HeldDir>,
}

impl<'a> Namespace<'a> {
    fn open(parent: &'a HeldDir, name: &'a CStr) -> io::Result<Self> {
        Ok(Self {
            parent,
            name,
            selected: parent.child_directory(name)?,
        })
    }

    fn verify(&self) -> io::Result<()> {
        self.parent.require_named()?;
        let now = self.parent.child_directory(self.name)?;
        match (self.selected.as_ref(), now) {
            (None, None) => Ok(()),
            (Some(original), Some(named))
                if original.entity == named.entity && original.same_mount(&named)? =>
            {
                original.require_named()
            }
            _ => Err(invalid("offline catalog namespace changed")),
        }
    }

    fn require_empty(&self) -> io::Result<()> {
        if let Some(held) = &self.selected {
            held.each_name(ACCESS, |_| {
                Err(invalid("offline catalog namespace is not empty"))
            })?;
        }
        Ok(())
    }
}

/// The caller must supply qualified descriptor selections and positively
/// prove the same exclusive state lease before and after this observation.
/// Nothing here publishes, reconciles, or makes a prior rename durable.
pub(crate) fn live_names(
    system: &HeldDir,
    recycle_root: &HeldDir,
    data: &[DataRoot],
    pool: Id,
    require_original: impl Fn() -> io::Result<()>,
) -> io::Result<BTreeSet<String>> {
    require_original()?;
    recycle::Recycle::require_roles(system, recycle_root, data)?;
    let names = read_live_names(system, recycle_root, data, pool)?;
    recycle::Recycle::require_roles(system, recycle_root, data)?;
    require_original()?;
    Ok(names)
}

fn read_live_names(
    system: &HeldDir,
    recycle_root: &HeldDir,
    data: &[DataRoot],
    pool: Id,
) -> io::Result<BTreeSet<String>> {
    if data.is_empty() || pool == [0; 16] {
        return Err(invalid("offline catalog requires the current DATA roster"));
    }
    let mut volumes = BTreeMap::new();
    for volume in data {
        let directory = Namespace::open(&volume.root, c"data")?;
        if volume.token == [0; 32] || volumes.insert(volume.token, directory).is_some() {
            return Err(invalid("offline catalog sees an invalid DATA token"));
        }
    }

    let catalog = Namespace {
        parent: system,
        name: routes::DIRECTORY,
        selected: routes::existing_directory(system)?,
    };
    let moves = Namespace::open(system, c"container-moves")?;
    let arrived = Namespace::open(recycle_root, c"containers")?;
    let mut names = BTreeSet::new();
    if let Some(routes) = &catalog.selected {
        let ids = routes::read_ids(routes, ACCESS)?;
        if ids.is_empty() {
            require_fresh(&volumes, &moves)?;
        }
        for id in ids {
            let token = routes::lookup_route(routes, id, ACCESS)?
                .ok_or_else(|| invalid("offline catalog route vanished"))?;
            if archived(&moves, &arrived, id)? {
                if !volumes.contains_key(&token)
                    && !recycle::read_retired_data(system, pool, token, ACCESS)?
                {
                    return Err(invalid("recycled route has no known DATA token"));
                }
                continue;
            }
            let volume = volumes
                .get(&token)
                .ok_or_else(|| invalid("offline catalog route has no enrolled DATA"))?;
            if let Some(name) = selected_name(volume, id)?
                && !names.insert(name)
            {
                return Err(invalid("duplicate offline live container name"));
            }
        }
    } else {
        require_fresh(&volumes, &moves)?;
    }
    catalog.verify()?;
    moves.verify()?;
    arrived.verify()?;
    volumes.values().try_for_each(Namespace::verify)?;
    Ok(names)
}

fn require_fresh(
    volumes: &BTreeMap<[u8; 32], Namespace<'_>>,
    moves: &Namespace<'_>,
) -> io::Result<()> {
    moves.require_empty()?;
    volumes.values().try_for_each(Namespace::require_empty)
}

fn archived(moves: &Namespace<'_>, arrived: &Namespace<'_>, id: Id) -> io::Result<bool> {
    let Some(moves) = &moves.selected else {
        return Ok(false);
    };
    let Some(ready) = recycle::read_selector(moves, id, ACCESS)? else {
        return Ok(false);
    };
    let arrived = arrived
        .selected
        .as_ref()
        .ok_or_else(|| invalid("selected offline recycle directory is absent"))?;
    recycle::qualify_arrival(arrived, ready, ACCESS)?.require_named()?;
    Ok(true)
}

fn selected_name(volume: &Namespace<'_>, id: Id) -> io::Result<Option<String>> {
    let parent = volume
        .selected
        .as_ref()
        .ok_or_else(|| invalid("routed offline DATA has no selected data directory"))?;
    let name = CString::new(hex(id)).map_err(|_| invalid("invalid offline container name"))?;
    let Some(held) = parent.child_directory(&name)? else {
        return Ok(None); // A reserved route with no visible container is not a name.
    };
    let state = replay_selected_entries(&held, id, ACCESS)?;
    held.require_named()?;
    Ok(state.name)
}

#[cfg(test)]
#[path = "../../tests/unit/offline_catalog.rs"]
mod tests;
