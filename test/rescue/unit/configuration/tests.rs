use super::*;

fn registration(root: &Path, config: std::path::PathBuf) -> Command {
    Command::RegisterDisk {
        config,
        slot: "D01".into(),
        disk_id: storage_engine::new_id("disk"),
        path: root.join("fourth"),
        level: 1,
        upstream: None,
        metadata_member: false,
    }
}
#[test]
fn registering_a_disk_keeps_an_exact_backup_and_atomically_replaces_the_draft() {
    let root = tempfile::tempdir().unwrap();
    let draft = Config::development([
        root.path().join("p"),
        root.path().join("b"),
        root.path().join("c"),
    ]);
    let path = root.path().join("config.json");
    let original = json_codec::to_vec(&draft).unwrap();
    private_io::write_new(&path, &original).unwrap();
    run(registration(root.path(), path.clone())).unwrap();
    let saved: Config = storage_engine::decode_strict(&std::fs::read(&path).unwrap()).unwrap();
    assert_eq!(saved.disks.len(), 4);
    assert_eq!(saved.disks.last().unwrap().slot_id, "D01");
    assert_eq!(
        std::fs::read(path.with_extension("json.before-registration")).unwrap(),
        original
    );
}
#[test]
fn different_configuration_extensions_keep_independent_registration_backups() {
    let root = tempfile::tempdir().unwrap();
    let draft = Config::development([
        root.path().join("p"),
        root.path().join("b"),
        root.path().join("c"),
    ]);
    let first = root.path().join("config.json");
    let second = root.path().join("config.draft");
    let original = json_codec::to_vec(&draft).unwrap();
    private_io::write_new(&first, &original).unwrap();
    let mut other = draft.clone();
    other.pool_id = storage_engine::new_id("pool");
    let other_original = json_codec::to_vec(&other).unwrap();
    private_io::write_new(&second, &other_original).unwrap();
    run(registration(root.path(), first)).unwrap();
    run(registration(root.path(), second)).unwrap();
    assert_eq!(
        std::fs::read(root.path().join("config.json.before-registration")).unwrap(),
        original
    );
    assert_eq!(
        std::fs::read(root.path().join("config.draft.before-registration")).unwrap(),
        other_original
    );
}

#[test]
fn registration_backup_preserves_the_complete_native_file_name() {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    for name in ["config", "config.json", "config.draft", ".config"] {
        assert_eq!(
            registration_backup(Path::new(name)).unwrap(),
            Path::new(&format!("{name}.before-registration"))
        );
    }
    let name = std::ffi::OsString::from_vec(b"config-\xff.json".to_vec());
    assert_eq!(
        registration_backup(Path::new(&name))
            .unwrap()
            .as_os_str()
            .as_bytes(),
        b"config-\xff.json.before-registration"
    );
    assert!(registration_backup(Path::new("/")).is_err());
}

#[test]
fn oversized_backup_name_fails_without_changing_the_draft() {
    let root = tempfile::tempdir().unwrap();
    let draft = Config::development([
        root.path().join("p"),
        root.path().join("b"),
        root.path().join("c"),
    ]);
    let path = root.path().join("c".repeat(250));
    let original = json_codec::to_vec(&draft).unwrap();
    private_io::write_new(&path, &original).unwrap();
    let error = run(registration(root.path(), path.clone())).unwrap_err();
    assert!(error.to_string().contains("draft was not changed"));
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn non_utf8_registration_path_is_rejected_before_any_write() {
    use std::os::unix::ffi::OsStringExt;
    let root = tempfile::tempdir().unwrap();
    let draft = Config::development([
        root.path().join("p"),
        root.path().join("b"),
        root.path().join("c"),
    ]);
    let path = root
        .path()
        .join(std::ffi::OsString::from_vec(b"config-\xff.json".to_vec()));
    let original = json_codec::to_vec(&draft).unwrap();
    private_io::write_new(&path, &original).unwrap();
    let error = run(registration(root.path(), path.clone())).unwrap_err();
    assert!(error.to_string().contains("valid UTF-8"));
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert_eq!(std::fs::read_dir(root.path()).unwrap().count(), 1);
}

#[test]
fn repeated_registration_keeps_the_immediately_previous_draft() {
    let root = tempfile::tempdir().unwrap();
    let draft = Config::development([
        root.path().join("p"),
        root.path().join("b"),
        root.path().join("c"),
    ]);
    let path = root.path().join("config.json");
    private_io::write_new(&path, &json_codec::to_vec(&draft).unwrap()).unwrap();
    run(registration(root.path(), path.clone())).unwrap();
    let previous = std::fs::read(&path).unwrap();
    let mut args = registration(root.path(), path.clone());
    if let Command::RegisterDisk { slot, path, .. } = &mut args {
        *slot = "D02".into();
        *path = root.path().join("fifth");
    }
    run(args).unwrap();
    assert_eq!(
        std::fs::read(registration_backup(&path).unwrap()).unwrap(),
        previous
    );
}

#[test]
fn invalid_upstream_level_is_an_error_and_never_modifies_the_draft() {
    let root = tempfile::tempdir().unwrap();
    let mut draft = Config::development([
        root.path().join("p"),
        root.path().join("b"),
        root.path().join("c"),
    ]);
    draft.disks[0].level = u32::MAX;
    let path = root.path().join("config.json");
    let original = json_codec::to_vec(&draft).unwrap();
    private_io::write_new(&path, &original).unwrap();
    assert!(run(registration(root.path(), path.clone())).is_err());
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert!(!path.with_extension("json.before-registration").exists());
}

#[test]
fn templates_accept_one_or_many_disks_without_initializing_them() {
    for count in 1..=5 {
        let root = tempfile::tempdir().unwrap();
        let output = root.path().join("draft.json");
        run(Command::Template {
            production: false,
            disks: (0..count)
                .map(|i| root.path().join(format!("disk{i}")))
                .collect(),
            output: Some(output.clone()),
        })
        .unwrap();
        let config: Config =
            storage_engine::decode_strict(&std::fs::read(output).unwrap()).unwrap();
        assert_eq!(config.disks.len(), count);
        let config = ValidatedConfig::try_from(config).unwrap();
        assert!(config.disks.iter().all(|disk| !disk.root.exists()));
    }
}

#[test]
fn invalid_templates_do_not_write_an_output() {
    let root = tempfile::tempdir().unwrap();
    let output = root.path().join("draft.json");
    for disks in [
        vec![],
        vec![PathBuf::from("relative")],
        vec![root.path().join("disk"), root.path().join("disk")],
        vec![root.path().join("disk"), root.path().join("disk/nested")],
        vec![root.path().join("disk/../other")],
    ] {
        assert!(
            run(Command::Template {
                production: false,
                disks,
                output: Some(output.clone())
            })
            .is_err()
        );
        assert!(!output.exists());
    }
}
