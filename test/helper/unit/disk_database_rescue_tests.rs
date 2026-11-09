use super::*;
use command_line::Parser;

#[test]
fn rescue_destinations_cannot_be_active_database_names() {
    for name in [
        ".zxs-disk.sqlite",
        ".zxs-disk-history.sqlite",
        ".zxs-file.sqlite",
        ".zxs-directory.sqlite",
        "index.sqlite",
    ] {
        assert!(require_suffix(Path::new(name), ".recovered.sqlite").is_err());
        assert!(require_suffix(Path::new(name), ".disk-db.json").is_err());
    }
    assert!(require_suffix(Path::new("disk.disk-db.json"), ".disk-db.json").is_ok());
    assert!(require_suffix(Path::new("disk.recovered.sqlite"), ".recovered.sqlite").is_ok());
}

#[test]
fn export_holds_service_exclusion_but_archive_rebuild_needs_no_host_state() {
    let export = crate::Args::try_parse_from([
        "zxs-helper",
        "disk-database-rescue",
        "export",
        "selected.json",
        "disk.disk-db.json",
    ])
    .unwrap()
    .command
    .unwrap();
    assert!(crate::requires_service_exclusion(&export));
    let rebuild = crate::Args::try_parse_from([
        "zxs-helper",
        "disk-database-rescue",
        "rebuild",
        "disk.disk-db.json",
        "identity.json",
        "--source",
        "current-from-history",
        "disk.recovered.sqlite",
    ])
    .unwrap()
    .command
    .unwrap();
    assert!(!crate::requires_service_exclusion(&rebuild));
    assert!(
        crate::Args::try_parse_from([
            "zxs-helper",
            "disk-database-rescue",
            "rebuild",
            "disk.disk-db.json",
            "identity.json",
            "disk.recovered.sqlite",
        ])
        .is_err()
    );
}
