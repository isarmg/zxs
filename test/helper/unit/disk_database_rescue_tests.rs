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
fn export_and_archive_rebuild_have_explicit_input_scopes() {
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
    assert!(matches!(
        export,
        crate::Command::DiskDatabaseRescue {
            command: Command::Export { .. }
        }
    ));
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
    assert!(matches!(
        rebuild,
        crate::Command::DiskDatabaseRescue {
            command: Command::Rebuild { .. }
        }
    ));
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

#[test]
fn disk_rescue_verify_is_read_only_and_has_no_implicit_source_selection() {
    let args = crate::Args::try_parse_from([
        "zxs-helper",
        "disk-database-rescue",
        "verify",
        "disk.disk-db.json",
        "identity.json",
        "--source",
        "history",
        "copy.recovered.sqlite",
    ])
    .unwrap();
    assert!(matches!(
        args.command,
        Some(crate::Command::DiskDatabaseRescue {
            command: Command::Verify { .. }
        })
    ));
    assert!(
        crate::Args::try_parse_from([
            "zxs-helper",
            "disk-database-rescue",
            "verify",
            "disk.disk-db.json",
            "identity.json",
            "copy.recovered.sqlite",
        ])
        .is_err()
    );
}
