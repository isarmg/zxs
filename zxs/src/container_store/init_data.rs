//! Initialize only one already enrolled empty DATA root. Cold-start cleanup and
//! other DATA owner leases are deliberately not part of this operation.
use super::{DataRoot, Id, hex, invalid, parse_container_id, recycle, routes};
use crate::fs::{HeldDir, OpaquePin, ReadAccess};
use std::{
    ffi::{CStr, CString},
    io,
    os::unix::fs::MetadataExt,
};

const ACCESS: ReadAccess = ReadAccess::NoAtime;

fn empty_owner(root: &HeldDir, name: &CStr) -> io::Result<OpaquePin> {
    let pin = root
        .opaque_pin_with(name, ACCESS)?
        .ok_or_else(|| invalid("required existing owner absent"))?;
    if pin.file.metadata()?.len() != 0 {
        return Err(invalid("unexpected contents of service owner record"));
    }
    Ok(pin)
}

fn directory(root: &HeldDir, name: &CStr) -> io::Result<HeldDir> {
    root.child_directory(name)?
        .ok_or_else(|| invalid("required initialized namespace is absent"))
}

fn empty(dir: &HeldDir) -> io::Result<()> {
    dir.each_name(ACCESS, |_| {
        Err(invalid(
            "DATA initialization requires empty data and staging directories",
        ))
    })
}

fn bit(name: &[u8], marker: &CStr) -> io::Result<u8> {
    if name == marker.to_bytes() {
        return Ok(1);
    }
    match name {
        b"owner" => Ok(2),
        b"data" => Ok(4),
        b"staging" => Ok(8),
        _ => Err(invalid("unexpected entry at DATA initialization boundary")),
    }
}

fn prefix(bits: u8) -> io::Result<u8> {
    match bits {
        1 => Ok(0),
        3 => Ok(1),
        7 => Ok(2),
        15 => Ok(3),
        _ => Err(invalid("not an exact empty DATA initialization prefix")),
    }
}

fn phase(root: &HeldDir, marker: &CStr) -> io::Result<u8> {
    let mut bits = 0;
    root.each_name(ACCESS, |name| {
        let add = bit(name, marker)?;
        if bits & add != 0 {
            return Err(invalid("repeated entry in DATA root"));
        }
        bits |= add;
        Ok(())
    })?;
    let phase = prefix(bits)?;
    if phase >= 1 {
        empty_owner(root, c"owner")?;
    }
    if phase >= 2 {
        empty(&directory(root, c"data")?)?;
    }
    if phase >= 3 {
        empty(&directory(root, c"staging")?)?;
    }
    Ok(phase)
}

fn require_phase(root: &HeldDir, marker: &CStr, expected: u8) -> io::Result<()> {
    if phase(root, marker)? != expected {
        return Err(invalid("DATA initialization prefix changed"));
    }
    Ok(())
}

fn no_routes(catalog: &HeldDir, token: [u8; 32]) -> io::Result<()> {
    catalog.each_name(ACCESS, |name| {
        // Never ignore a pending route stage when proving a new disk has no history.
        let id = parse_container_id(name)?;
        let routed =
            routes::lookup_route(catalog, id, ACCESS)?.ok_or_else(|| invalid("route vanished"))?;
        if routed == token {
            return Err(invalid("a container already routes to the selected DATA"));
        }
        Ok(())
    })
}

fn require_not_retiring(system: &HeldDir, pool: Id, token: [u8; 32]) -> io::Result<()> {
    if recycle::read_retired_data(system, pool, token, ACCESS)? {
        return Err(invalid("this DATA token has been retired"));
    }
    // Helper may stop after creating its tombstone stage but before naming
    // the final record or staging the roster change. Never complete a
    // different operation on that target; preserve even an empty stage.
    if let Some(ledger) = system.child_directory(c"retired-data")? {
        let stage = CString::new(format!(".{}.incoming", hex(token)))
            .map_err(|_| invalid("invalid DATA retirement stage name"))?;
        if ledger.pin_record(&stage)?.is_some() {
            return Err(invalid("this DATA token has an unfinished retirement"));
        }
        ledger.require_named()?;
    }
    Ok(())
}

pub(crate) fn initialize(
    system: &HeldDir,
    recycle_root: &HeldDir,
    data: &[DataRoot],
    pool: Id,
    target: &DataRoot,
    marker: &CStr,
    authorize: impl Fn() -> io::Result<()>,
) -> io::Result<()> {
    authorize()?;
    recycle::Recycle::require_roles(system, recycle_root, data)?;
    initialize_selected(system, recycle_root, pool, target, marker, authorize)
}

fn initialize_selected(
    system: &HeldDir,
    recycle_root: &HeldDir,
    pool: Id,
    target: &DataRoot,
    marker: &CStr,
    authorize: impl Fn() -> io::Result<()>,
) -> io::Result<()> {
    let system_owner = empty_owner(system, c"container-owner")?;
    let recycle_owner = empty_owner(recycle_root, c"container-owner")?;
    // Lock the exact existing records, without creating or syncing non-target roots.
    system_owner.file.try_lock().map_err(io::Error::from)?;
    recycle_owner.file.try_lock().map_err(io::Error::from)?;
    let catalog = routes::existing_directory(system)?
        .ok_or_else(|| invalid("existing pool is not initialized"))?;
    let namespaces = [
        directory(system, c"container-moves")?,
        directory(recycle_root, c"incoming-containers")?,
        directory(recycle_root, c"containers")?,
    ];
    let root = &target.root;
    for name in [c"owner", c"data", c"staging"] {
        root.checked_child_path(name)?;
    }
    let current = || -> io::Result<()> {
        authorize()?;
        system_owner.require(system, c"container-owner")?;
        recycle_owner.require(recycle_root, c"container-owner")?;
        catalog.require_named()?;
        for dir in &namespaces {
            dir.require_named()?;
        }
        require_not_retiring(system, pool, target.token)?;
        root.require_named()
    };
    current()?;
    no_routes(&catalog, target.token)?;
    complete_prefix(root, marker, &catalog, target.token, current)
}

fn complete_prefix(
    root: &HeldDir,
    marker: &CStr,
    catalog: &HeldDir,
    token: [u8; 32],
    current: impl Fn() -> io::Result<()>,
) -> io::Result<()> {
    let before = phase(root, marker)?;
    let old_owner = (before >= 1)
        .then(|| empty_owner(root, c"owner"))
        .transpose()?;
    let old_data = (before >= 2)
        .then(|| directory(root, c"data"))
        .transpose()?;
    let old_staging = (before >= 3)
        .then(|| directory(root, c"staging"))
        .transpose()?;
    let original = || -> io::Result<()> {
        if let Some(pin) = &old_owner {
            pin.require(root, c"owner")?;
        }
        if let Some(dir) = &old_data {
            dir.require_named()?;
            empty(dir)?;
        }
        if let Some(dir) = &old_staging {
            dir.require_named()?;
            empty(dir)?;
        }
        Ok(())
    };
    current()?;
    original()?;
    // New entries are exclusive creates. A race cannot substitute an
    // unrelated name for an observed absence or a different existing prefix.
    let owner = if before == 0 {
        let file = root.create_opaque_record(c"owner")?;
        file.try_lock().map_err(io::Error::from)?;
        file
    } else {
        root.existing_opaque_owner(c"owner")?
    };
    let pinned = empty_owner(root, c"owner")?;
    let a = owner.metadata()?;
    let b = pinned.file.metadata()?;
    if (a.dev(), a.ino()) != (b.dev(), b.ino()) || !root.shares_file_mount(&owner)? {
        return Err(invalid(
            "selected DATA owner is no longer the locked original",
        ));
    }
    let check = || -> io::Result<()> {
        current()?;
        original()?;
        pinned.require(root, c"owner")
    };
    check()?;
    owner.sync_all()?;
    root.file.sync_all()?;
    require_phase(root, marker, before.max(1))?;
    check()?;
    let contents = if before >= 2 {
        directory(root, c"data")?
    } else {
        root.new_child_directory(c"data")?
    };
    empty(&contents)?;
    contents.file.sync_all()?;
    root.file.sync_all()?;
    require_phase(root, marker, before.max(2))?;
    check()?;
    contents.require_named()?;
    let staging = if before >= 3 {
        directory(root, c"staging")?
    } else {
        root.new_child_directory(c"staging")?
    };
    empty(&staging)?;
    staging.file.sync_all()?;
    check()?;
    require_phase(root, marker, 3)?;
    contents.require_named()?;
    staging.require_named()?;
    root.file.sync_all()?;
    no_routes(catalog, token)?;
    require_phase(root, marker, 3)?;
    contents.require_named()?;
    staging.require_named()?;
    check()
}

#[cfg(test)]
#[path = "../../tests/unit/init_data_prefix.rs"]
mod tests;
