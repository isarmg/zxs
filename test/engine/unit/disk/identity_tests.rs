use super::*;
use std::error::Error as _;

fn invalid_seals(original: &[u8]) -> Vec<Vec<u8>> {
    let mut invalid = vec![b"{".to_vec(), b"[]".to_vec(), b"{}".to_vec()];
    let seal: json_codec::Value = json_codec::from_slice(original).unwrap();
    for generation in [
        json_codec::json!("not-an-epoch"),
        json_codec::json!("18446744073709551616"),
        json_codec::json!(1),
    ] {
        let mut changed = seal.clone();
        changed["binding_generation"] = generation;
        invalid.push(canonical_json(&changed).unwrap());
    }
    let mut unknown = seal;
    unknown["unknown_identity_field"] = json_codec::json!(true);
    invalid.push(canonical_json(&unknown).unwrap());
    invalid
}

fn assert_identity_json_error(error: &Error) {
    assert_eq!(error.code(), ErrorCode::DiskIdentityMismatch);
    assert!(!error.is_retryable());
    assert!(error.published_result().is_none());
    let Error::Context { source, .. } = error else {
        panic!("disk identity context was lost");
    };
    let Error::Contract(storage_contract::Error::Json { source, .. }) = source.as_ref() else {
        panic!("concrete JSON error was lost");
    };
    assert!(std::ptr::eq(
        source,
        error
            .source()
            .unwrap()
            .source()
            .unwrap()
            .downcast_ref::<json_codec::Error>()
            .unwrap(),
    ));
}

#[test]
fn invalid_identity_bytes_keep_json_sources_at_disk_read_boundaries() {
    let temporary = tempfile::tempdir().unwrap();
    let config = Config::development([temporary.path().join("member")]);
    let system = temporary.path().join("system");
    drop(Engine::initialize(&system, config.clone()).unwrap());
    let config = ValidatedConfig::try_from(config).unwrap();
    let disk = &config.disks[0];
    let path = disk.root.join("disk.json");
    let original = fs::read(&path).unwrap();
    for bytes in invalid_seals(&original) {
        fs::write(&path, &bytes).unwrap();
        assert_identity_json_error(&validate_seal(&config, disk).unwrap_err());
        assert_identity_json_error(&offline_check(&disk.root).unwrap_err());
        assert_identity_json_error(
            &offline_export(&disk.root, &new_id("ver"), &temporary.path().join("export"))
                .unwrap_err(),
        );
        assert_identity_json_error(&Engine::open(&system).err().unwrap());
        assert_identity_json_error(
            &Engine::restore_from_disks(
                &temporary.path().join("restore"),
                std::slice::from_ref(&disk.root),
            )
            .err()
            .unwrap(),
        );
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
    fs::write(&path, &original).unwrap();
    drop(Engine::open(&system).unwrap());
    assert_eq!(fs::read(&path).unwrap(), original);
}

#[test]
fn invalid_identity_bytes_cannot_publish_state_or_append_journals() {
    let temporary = tempfile::tempdir().unwrap();
    let config = Config::development([temporary.path().join("member")]);
    let system = temporary.path().join("system");
    let mut engine = Engine::initialize(&system, config.clone()).unwrap();
    engine
        .create_repository(
            &RepositoryName::parse("published").unwrap(),
            "P01",
            1,
            "owner",
            OperationId::generate(),
        )
        .unwrap();
    let path = config.disks[0].root.join("disk.json");
    let original = fs::read(&path).unwrap();
    for bytes in invalid_seals(&original) {
        let before = engine.state().clone();
        let journals = config
            .disks
            .iter()
            .map(|disk| fs::read(crate::journal::log_path(disk)).unwrap())
            .collect::<Vec<_>>();
        let operation = OperationId::generate();
        fs::write(&path, &bytes).unwrap();
        let error = engine
            .create_repository(
                &RepositoryName::parse("rejected").unwrap(),
                "P01",
                1,
                "owner",
                operation,
            )
            .unwrap_err();
        assert_identity_json_error(&error);
        assert_eq!(engine.state(), &before);
        assert!(!engine.state().operations.contains_key(&operation));
        for (disk, journal) in config.disks.iter().zip(&journals) {
            assert_eq!(fs::read(crate::journal::log_path(disk)).unwrap(), *journal);
        }
        assert_eq!(fs::read(&path).unwrap(), bytes);
        drop(engine);
        assert_identity_json_error(&Engine::open(&system).err().unwrap());
        for (disk, journal) in config.disks.iter().zip(&journals) {
            assert_eq!(fs::read(crate::journal::log_path(disk)).unwrap(), *journal);
        }

        fs::write(&path, &original).unwrap();
        engine = Engine::open(&system).unwrap();
        assert_eq!(engine.state(), &before);
        assert!(!engine.state().operations.contains_key(&operation));
    }
    let operation = OperationId::generate();
    engine
        .create_repository(
            &RepositoryName::parse("after-reopen").unwrap(),
            "P01",
            1,
            "owner",
            operation,
        )
        .unwrap();
    assert!(engine.state().operations.contains_key(&operation));
}

#[test]
fn identity_reads_preserve_open_errors_and_metadata_limits() {
    let temporary = tempfile::tempdir().unwrap();
    let config = Config::development([temporary.path().join("member")]);
    let system = temporary.path().join("system");
    drop(Engine::initialize(&system, config.clone()).unwrap());
    let config = ValidatedConfig::try_from(config).unwrap();
    let disk = &config.disks[0];
    let path = disk.root.join("disk.json");
    let original = fs::read(&path).unwrap();
    fs::remove_file(&path).unwrap();
    let error = validate_seal(&config, disk).unwrap_err();
    assert!(matches!(
        error,
        Error::Filesystem(platform_io::fs_safety::Error::Io(ref source))
            if source.kind() == std::io::ErrorKind::NotFound
    ));
    assert_eq!(error.code(), ErrorCode::IoError);

    File::create(&path)
        .unwrap()
        .set_len(MAX_METADATA as u64 + 1)
        .unwrap();
    for error in [
        validate_seal(&config, disk).unwrap_err(),
        Engine::open(&system).err().unwrap(),
        offline_check(&disk.root).unwrap_err(),
    ] {
        assert_eq!(error.code(), ErrorCode::ResourceLimit);
        assert!(matches!(error, Error::Contract(_)));
    }
    assert_eq!(fs::metadata(&path).unwrap().len(), MAX_METADATA as u64 + 1);
    fs::write(&path, original).unwrap();
    validate_seal(&config, disk).unwrap();
    drop(Engine::open(&system).unwrap());
}
