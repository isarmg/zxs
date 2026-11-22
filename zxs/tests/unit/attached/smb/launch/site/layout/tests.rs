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
