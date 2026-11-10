use super::*;

#[test]
fn rescue_suffixes_cannot_name_managed_databases() {
    for name in [
        ".zxs-file.sqlite",
        ".zxs-directory.sqlite",
        ".zxs-disk.sqlite",
        "index.sqlite",
    ] {
        assert!(artifact_path(Path::new(name), ".recovered.sqlite").is_err());
        assert!(artifact_path(Path::new(name), ".object-db.json").is_err());
    }
    assert!(artifact_path(Path::new("file.recovered.sqlite"), ".recovered.sqlite").is_ok());
    assert!(artifact_path(Path::new("file.object-db.json"), ".object-db.json").is_ok());
}

#[test]
fn object_rescue_verify_is_read_only_and_requires_explicit_scope_and_image() {
    use command_line::Parser;
    let args = crate::Args::try_parse_from([
        "zxs-helper",
        "object-database",
        "verify",
        "file.object-db.json",
        "scope.json",
        "copy.recovered.sqlite",
    ])
    .unwrap();
    assert!(matches!(
        args.command,
        Some(crate::Command::ObjectDatabase {
            command: Command::Verify { .. }
        })
    ));
}
