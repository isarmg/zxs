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
mod tests {
    use super::*;

    #[test]
    fn control_paths_are_not_media_and_grouping_directories_are_not_volumes() {
        for path in [
            "/", ROOT, STATE, "/zxs/state/instance", BIN, "/zxs/bin/zxs",
            LOG, "/zxs/log/archive", DISKS, DATA,
        ] {
            assert!(!media(Path::new(path)), "{path}");
        }
        for path in [
            "/zxs/disks/system", "/zxs/disks/data/one", "/zxs/disks/recycle",
            "/mnt/new-volume", "/zxs/ordinary", "/zxs/logbook", "/zxs-other/state",
        ] {
            assert!(media(Path::new(path)), "{path}");
        }
        for path in [ROOT, STATE, "/zxs/state/zxs", "/zxs/log/zxs", DISKS, "/zxs/disks/data/one/zxs"] {
            assert!(!image(Path::new(path)), "{path}");
        }
        assert!(image(Path::new("/zxs/bin/zxs")));
        assert!(image(Path::new("/opt/explicit/zxs")));
    }
}
