//! Host paths, never cwd, environment defaults or grants to create objects.
//! The selected S and ELF, and the media reported by the original S, remain
//! exact absolute bindings. Names below reserve the separate control areas.
use std::path::Path;

pub(super) const ROOT: &str = "/zxs";
pub(super) const STATE: &str = "/zxs/state";
pub(super) const LOG: &str = "/zxs/log";
const BIN: &str = "/zxs/bin";
const DISKS: &str = "/zxs/disks";
const DATA: &str = "/zxs/disks/data";

fn overlaps(path: &Path, reserved: &str) -> bool {
    path.starts_with(reserved) || Path::new(reserved).starts_with(path)
}

// Call only after canonical/bounded path validation. These are additional
// name exclusions, not a replacement for the held mount/inode checks.
pub(super) fn media(path: &Path) -> bool {
    path != Path::new(DISKS)
        && path != Path::new(DATA)
        && ![STATE, BIN, LOG].iter().any(|p| overlaps(path, p))
}

pub(super) fn image(path: &Path) -> bool {
    ![STATE, LOG, DISKS].iter().any(|p| overlaps(path, p))
}

#[cfg(test)]
#[path = "../../../../tests/unit/attached/smb/launch/site/layout/tests.rs"]
mod tests;
