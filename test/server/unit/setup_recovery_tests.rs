use super::*;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

struct Fixture {
    _temp: tempfile::TempDir,
    service: SetupRecovery,
}
fn fixture() -> Fixture {
    fixture_with_disks(&["one", "two", "three"])
}
fn fixture_with_disks(ids: &[&str]) -> Fixture {
    let temp = tempfile::tempdir().unwrap();
    let data = temp.path().join("system");
    let management = temp.path().join("management");
    fs::create_dir(&data).unwrap();
    fs::set_permissions(&data, fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir(&management).unwrap();
    let mut disks = BTreeMap::new();
    for &id in ids {
        let path = temp.path().join(id);
        fs::create_dir(&path).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        disks.insert(id.into(), path);
    }
    let mut key = fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(management.join("bootstrap.key"))
        .unwrap();
    key.write_all(b"0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
        .unwrap();
    Fixture {
        _temp: temp,
        service: SetupRecovery::new(data, management, disks)
            .unwrap()
            .allow_development_mode(),
    }
}
fn permit(service: &SetupRecovery) -> LocalBootstrapPermit {
    service
        .authorize_bootstrap("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
        .unwrap()
}
fn request() -> SetupRequest {
    SetupRequest {
        disk_ids: vec!["one".into(), "two".into(), "three".into()],
        mode: Mode::Development,
        username: "admin".into(),
        password: "long-valid-password".into(),
    }
}
fn admin() -> Identity {
    Identity {
        admission_session: None,
        smb_binding: None,
        principal: "admin".into(),
    }
}

#[test]
fn never_treats_old_disk_or_lost_config_as_empty() {
    let f = fixture();
    assert_eq!(f.service.status().state, InstallationState::Empty);
    fs::write(f.service.disks["one"].join("disk.json"), b"old identity").unwrap();
    assert_eq!(
        f.service.status().state,
        InstallationState::RecoveryRequired
    );
    let result = f
        .service
        .initialize(&permit(&f.service), request(), |_, _, _| {
            panic!("must not initialize")
        });
    assert_eq!(
        result.unwrap_err().code,
        storage_contract::ErrorCode::RecoveryRequired
    );
    assert!(!f.service.data_root.join("config.json").exists());
}
#[test]
fn missing_disk_does_not_become_new_directory() {
    let f = fixture();
    fs::remove_dir(&f.service.disks["one"]).unwrap();
    assert_eq!(f.service.status().state, InstallationState::Unavailable);
    assert!(
        f.service
            .initialize(&permit(&f.service), request(), |_, _, _| panic!())
            .is_err()
    );
    assert!(!f.service.disks["one"].exists());
}
#[test]
fn auth_or_authority_loss_requires_recovery() {
    for path in [
        "auth.json",
        "management-authority.json",
        "setup-intent.json",
    ] {
        let f = fixture();
        fs::write(f.service.data_root.join(path), b"existing").unwrap();
        assert_eq!(
            f.service.status().state,
            InstallationState::RecoveryRequired
        );
    }
    let f = fixture();
    fs::write(
        f.service.management_root.join("management-auth.json"),
        b"existing",
    )
    .unwrap();
    assert_eq!(
        f.service.status().state,
        InstallationState::RecoveryRequired
    );
}
#[test]
fn bootstrap_rejects_missing_bad_public_or_symlink_key() {
    let f = fixture();
    assert!(f.service.authorize_bootstrap(&"0".repeat(64)).is_err());
    let path = f.service.management_root.join("bootstrap.key");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(
        f.service
            .authorize_bootstrap("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
            .is_err()
    );
    fs::remove_file(&path).unwrap();
    std::os::unix::fs::symlink("missing", &path).unwrap();
    assert!(
        f.service
            .authorize_bootstrap("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
            .is_err()
    );
}
#[test]
fn permit_is_bound_to_service_and_revoked_key() {
    let a = fixture();
    let b = fixture();
    let authorization = permit(&a.service);
    assert!(b.service.verify_permit(&authorization).is_err());
    fs::write(
        a.service.management_root.join("bootstrap.key"),
        "f".repeat(64),
    )
    .unwrap();
    assert!(a.service.verify_permit(&authorization).is_err());
}
#[test]
fn rejects_unknown_duplicate_or_changed_disk() {
    let f = fixture();
    assert!(f.service.selected(&["/tmp".into()]).is_err());
    assert!(f.service.selected(&["one".into(), "one".into()]).is_err());
    fs::remove_dir(&f.service.disks["one"]).unwrap();
    std::os::unix::fs::symlink(&f.service.disks["two"], &f.service.disks["one"]).unwrap();
    assert!(f.service.selected(&["one".into()]).is_err());
}
#[test]
fn validates_credentials_before_writes() {
    let f = fixture();
    let mut req = request();
    req.password = "short".into();
    assert!(
        f.service
            .initialize(&permit(&f.service), req, |_, _, _| panic!())
            .is_err()
    );
    assert!(!f.service.data_root.join("setup-intent.json").exists());
    assert!(
        fs::read_dir(&f.service.disks["one"])
            .unwrap()
            .next()
            .is_none()
    );
}
#[test]
fn partial_setup_is_fenced_and_success_never_reinitializes() {
    let f = fixture();
    let result = f
        .service
        .initialize(&permit(&f.service), request(), |_, _, _| {
            Err(ApiError::conflict(
                storage_contract::ErrorCode::InjectedFailure,
                "simulated auth failure",
            ))
        });
    assert_eq!(
        result.unwrap_err().code,
        storage_contract::ErrorCode::InjectedFailure
    );
    assert!(f.service.data_root.join("setup-intent.json").exists());
    fs::remove_file(f.service.data_root.join("config.json")).unwrap();
    assert_eq!(
        f.service.status().state,
        InstallationState::RecoveryRequired
    );
    assert!(
        f.service
            .initialize(&permit(&f.service), request(), |_, _, _| panic!())
            .is_err()
    );
}
#[test]
fn exclusive_lock_blocks_concurrent_setup() {
    let f = fixture();
    let _guard = f.service.lock().unwrap();
    let result = f
        .service
        .initialize(&permit(&f.service), request(), |_, _, _| panic!());
    assert_eq!(
        result.unwrap_err().code,
        storage_contract::ErrorCode::SetupBusy
    );
}
#[test]
fn recovery_requires_explicit_permanent_loss_acknowledgement() {
    let f = fixture();
    assert_eq!(
        f.service
            .recovery_commit(
                &admin(),
                RecoveryCommitRequest {
                    recovery_id: "recovery".into(),
                    digest: "digest".into(),
                    accept_permanent_loss: false
                },
                |_| panic!("unacknowledged recovery must not reach final admission"),
            )
            .unwrap_err()
            .status,
        400
    );
}
#[test]
fn restore_requires_two_old_members_and_never_initializes_them() {
    let f = fixture();
    assert!(
        f.service
            .restore_system(
                &permit(&f.service),
                RestoreRequest {
                    accept_stale_credentials: true,
                    disk_ids: vec!["one".into()]
                },
                |_| panic!()
            )
            .is_err()
    );
    assert!(
        f.service
            .restore_system(
                &permit(&f.service),
                RestoreRequest {
                    accept_stale_credentials: true,
                    disk_ids: vec!["one".into(), "two".into()]
                },
                |_| panic!()
            )
            .is_err()
    );
    assert!(
        fs::read_dir(&f.service.disks["one"])
            .unwrap()
            .next()
            .is_none()
    );
}

#[test]
fn completed_setup_preserves_identity_and_requires_restart_integration() {
    let f = fixture();
    let mut installed = false;
    f.service
        .initialize(
            &permit(&f.service),
            request(),
            |engine, username, password| {
                assert_eq!(username, "admin");
                assert_eq!(password, "long-valid-password");
                assert_eq!(engine.config().disks.len(), 3);
                installed = true;
                Ok(())
            },
        )
        .unwrap();
    assert!(installed);
    assert_eq!(f.service.status().state, InstallationState::Existing);
    assert!(
        f.service
            .initialize(&permit(&f.service), request(), |_, _, _| panic!())
            .is_err()
    );
}

#[test]
fn local_bootstrap_never_bypasses_existing_management_auth() {
    let f = fixture();
    fs::write(
        f.service.management_root.join("management-auth.json"),
        b"existing or corrupt credentials",
    )
    .unwrap();
    let error = f
        .service
        .restore_system(
            &permit(&f.service),
            RestoreRequest {
                accept_stale_credentials: true,
                disk_ids: vec!["one".into(), "two".into()],
            },
            |_| panic!(),
        )
        .unwrap_err();
    assert_eq!(error.code, storage_contract::ErrorCode::SystemAuthRequired);
    assert!(!f.service.data_root.join("config.json").exists());
}

#[test]
fn rejects_symbolic_link_lock_and_unknown_request_fields() {
    let f = fixture();
    let target = f.service.management_root.join("untouched");
    fs::write(&target, b"do not change").unwrap();
    std::os::unix::fs::symlink(&target, f.service.data_root.join("setup.lock")).unwrap();
    assert!(f.service.lock().is_err());
    assert_eq!(fs::read(target).unwrap(), b"do not change");
    assert!(json_codec::from_str::<SetupRequest>(r#"{"disk_ids":["one","two","three"],"mode":"development","username":"admin","password":"long-valid-password","output":"/tmp/unsafe"}"#).is_err());
}

#[test]
fn bootstrap_expiration_and_directory_replacement_are_fenced() {
    let f = fixture();
    let mut authorization = permit(&f.service);
    authorization.issued = std::time::Instant::now() - std::time::Duration::from_secs(601);
    assert!(f.service.verify_permit(&authorization).is_err());
    let key = fs::OpenOptions::new()
        .write(true)
        .open(f.service.management_root.join("bootstrap.key"))
        .unwrap();
    key.set_times(
        fs::FileTimes::new()
            .set_modified(std::time::SystemTime::now() - std::time::Duration::from_secs(1801)),
    )
    .unwrap();
    let error = f
        .service
        .authorize_bootstrap("0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef")
        .err()
        .unwrap();
    assert_eq!(error.code, storage_contract::ErrorCode::BootstrapExpired);
    let disk = &f.service.disks["one"];
    fs::rename(disk, disk.with_extension("old")).unwrap();
    fs::create_dir(disk).unwrap();
    assert!(f.service.selected(&["one".into()]).is_err());
}

#[test]
fn production_requires_mount_and_simulation_needs_local_opt_in() {
    let mut f = fixture();
    assert!(require_mountpoint(&f.service.disks["one"]).is_err());
    f.service.allow_development = false;
    assert_eq!(
        f.service
            .initialize(&permit(&f.service), request(), |_, _, _| panic!())
            .unwrap_err()
            .status,
        403
    );
    assert!(!f.service.data_root.join("setup-intent.json").exists());
}

#[test]
fn configured_management_subdirectory_does_not_prevent_first_setup() {
    let f = fixture();
    let nested = f.service.data_root.join("management");
    fs::rename(&f.service.management_root, &nested).unwrap();
    let service =
        SetupRecovery::new(f.service.data_root.clone(), nested, f.service.disks.clone()).unwrap();
    assert_eq!(service.status().state, InstallationState::Empty);
}

#[test]
fn unavailable_unrelated_member_does_not_block_survivor_checks_or_restore_selection() {
    let f = fixture();
    let config = Config::development([
        f.service.disks["one"].clone(),
        f.service.disks["two"].clone(),
        f.service.disks["three"].clone(),
    ]);
    drop(Engine::initialize(&f.service.data_root, config).unwrap());
    fs::rename(
        &f.service.disks["three"],
        f.service.disks["three"].with_extension("offline"),
    )
    .unwrap();
    let service = SetupRecovery::new(
        f.service.data_root.clone(),
        f.service.management_root.clone(),
        f.service.disks.clone(),
    )
    .unwrap();
    assert!(
        service
            .offline_check(&admin(), "one")
            .unwrap()
            .issues
            .is_empty()
    );
    assert_eq!(
        service.selected(&["three".into()]).unwrap_err().code,
        storage_contract::ErrorCode::DiskUnavailable
    );
    assert_eq!(
        service
            .selected(&["one".into(), "two".into()])
            .unwrap()
            .len(),
        2
    );
    let restored = f._temp.path().join("restored-system");
    fs::create_dir(&restored).unwrap();
    fs::set_permissions(&restored, fs::Permissions::from_mode(0o700)).unwrap();
    let recovery = SetupRecovery::new(
        restored.clone(),
        f.service.management_root.clone(),
        f.service.disks.clone(),
    )
    .unwrap();
    // No auth was installed in this fixture. Reaching this exact post-restore
    // error proves the two survivor members were accepted by core recovery.
    let result = recovery.restore_system(
        &permit(&recovery),
        RestoreRequest {
            disk_ids: vec!["one".into(), "two".into()],
            accept_stale_credentials: true,
        },
        |_| panic!("no credentials may be invented"),
    );
    assert_eq!(
        result.unwrap_err().code,
        storage_contract::ErrorCode::AuthRecoveryRequired
    );
    assert!(restored.join("config.json").exists());
    assert!(!f.service.disks["three"].exists());
}

#[test]
fn unavailable_descriptor_fences_new_setup_without_poisoning_other_selections() {
    let f = fixture();
    fs::remove_dir(&f.service.disks["three"]).unwrap();
    let service = SetupRecovery::new(
        f.service.data_root.clone(),
        f.service.management_root.clone(),
        f.service.disks.clone(),
    )
    .unwrap()
    .allow_development_mode();
    assert_eq!(service.status().state, InstallationState::Unavailable);
    assert!(service.selected(&["one".into()]).is_ok());
    assert!(
        service
            .initialize(&permit(&service), request(), |_, _, _| panic!())
            .is_err()
    );
    assert!(!f.service.data_root.join("setup-intent.json").exists());
}

fn interrupted(f: &Fixture) {
    assert_eq!(
        f.service
            .initialize(&permit(&f.service), request(), |_, _, _| Err(
                ApiError::conflict(
                    storage_contract::ErrorCode::InjectedFailure,
                    "test interruption"
                )
            ))
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::InjectedFailure
    );
}
fn completion() -> CompleteSetupRequest {
    CompleteSetupRequest {
        username: "admin".into(),
        password: "long-valid-password".into(),
    }
}
#[test]
fn completes_only_bound_pre_authority_interruption() {
    let f = fixture();
    interrupted(&f);
    // A normal daemon probe must not consume eligibility for pristine setup.
    drop(Engine::open(&f.service.data_root).unwrap());
    assert!(f.service.status().completion_available);
    let mut called = false;
    f.service
        .complete_initialization(&permit(&f.service), completion(), |engine, user, _| {
            assert!(engine.auth_state().is_none());
            assert_eq!(user, "admin");
            called = true;
            fs::write(
                f.service.data_root.join("management-authority.json"),
                b"claimed",
            )
            .unwrap();
            Ok(())
        })
        .unwrap();
    assert!(called);
    assert!(!f.service.status().completion_available);
    assert_eq!(
        f.service
            .complete_initialization(&permit(&f.service), completion(), |_, _, _| panic!())
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::AuthorityRecoveryRequired
    );
}
#[test]
fn completion_cannot_reset_old_pool_or_partial_authority_or_changed_data() {
    let f = fixture();
    drop(
        Engine::initialize(
            &f.service.data_root,
            Config::development([
                f.service.disks["one"].clone(),
                f.service.disks["two"].clone(),
                f.service.disks["three"].clone(),
            ]),
        )
        .unwrap(),
    );
    assert!(
        f.service
            .complete_initialization(&permit(&f.service), completion(), |_, _, _| panic!())
            .is_err()
    );
    for artifact in ["management-authority.json", "auth.json"] {
        let f = fixture();
        interrupted(&f);
        fs::write(f.service.data_root.join(artifact), b"existing").unwrap();
        assert_eq!(
            f.service
                .complete_initialization(&permit(&f.service), completion(), |_, _, _| panic!())
                .unwrap_err()
                .code,
            storage_contract::ErrorCode::AuthorityRecoveryRequired
        );
    }
    let f = fixture();
    interrupted(&f);
    fs::write(
        f.service.management_root.join("management-auth.json"),
        b"corrupt",
    )
    .unwrap();
    assert_eq!(
        f.service
            .complete_initialization(&permit(&f.service), completion(), |_, _, _| panic!())
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::AuthorityRecoveryRequired
    );
    let f = fixture();
    interrupted(&f);
    fs::write(f.service.disks["one"].join("histories/user-data"), b"keep").unwrap();
    assert!(
        f.service
            .complete_initialization(&permit(&f.service), completion(), |_, _, _| panic!())
            .is_err()
    );
    assert_eq!(
        fs::read(f.service.disks["one"].join("histories/user-data")).unwrap(),
        b"keep"
    );
}
#[test]
fn completion_requires_original_key_and_ready_proof() {
    let f = fixture();
    interrupted(&f);
    fs::write(
        f.service.management_root.join("bootstrap.key"),
        "f".repeat(64),
    )
    .unwrap();
    let changed = f.service.authorize_bootstrap(&"f".repeat(64)).unwrap();
    assert!(
        f.service
            .complete_initialization(&changed, completion(), |_, _, _| panic!())
            .is_err()
    );
    let f = fixture();
    interrupted(&f);
    fs::remove_file(f.service.data_root.join("setup-ready.json")).unwrap();
    assert!(!f.service.status().completion_available);
    assert!(
        f.service
            .complete_initialization(&permit(&f.service), completion(), |_, _, _| panic!())
            .is_err()
    );
}

fn preset_fixture() -> (Fixture, Config, PathBuf) {
    let mut f = fixture();
    let extra = f._temp.path().join("four");
    fs::create_dir(&extra).unwrap();
    fs::set_permissions(&extra, fs::Permissions::from_mode(0o700)).unwrap();
    f.service.disks.insert("four".into(), extra.clone());
    f.service
        .identities
        .insert("four".into(), directory_identity(&extra).unwrap());
    let mut config = Config::development([
        f.service.disks["one"].clone(),
        f.service.disks["two"].clone(),
        f.service.disks["three"].clone(),
    ]);
    let mut fourth = config.disks[2].clone();
    fourth.root = extra;
    fourth.slot_id = "D01".into();
    fourth.disk_id = storage_engine::new_id("disk");
    fourth.level = 4;
    fourth.upstream_slot_id = Some("C01".into());
    fourth.metadata_member = false;
    config.disks.push(fourth);
    config.max_file_bytes = 1 << 30;
    config.max_workspace_bytes = 2 << 30;
    config.max_workspaces = 9;
    config.max_entries = 123;
    config.metadata_reserve_bytes = 1 << 20;
    config.workspace_ttl_seconds = 3600;
    let path = f.service.management_root.join("custom.json");
    fs::write(&path, json_codec::to_vec(&config).unwrap()).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    f.service.presets.insert("custom".into(), path.clone());
    (f, config, path)
}
#[test]
fn local_preset_preserves_full_configuration_and_redacts_paths() {
    let (f, config, _) = preset_fixture();
    let preview = f
        .service
        .preview_preset(
            &permit(&f.service),
            PresetPreviewRequest {
                preset_id: "custom".into(),
            },
        )
        .unwrap();
    assert!(
        !preview
            .to_string()
            .contains(f._temp.path().to_str().unwrap())
    );
    assert!(preview["configuration"]["disks"][0].get("root").is_none());
    assert_eq!(preview["configuration"]["disks"][3]["candidate_id"], "four");
    let digest = preview["config_digest"].as_str().unwrap().to_owned();
    let expected = value_digest(&config).unwrap();
    f.service
        .initialize_preset(
            &permit(&f.service),
            PresetSetupRequest {
                preset_id: "custom".into(),
                config_digest: digest,
                username: "admin".into(),
                password: "long-valid-password".into(),
            },
            |engine, _, _| {
                assert_eq!(value_digest(engine.config()).unwrap(), expected);
                Ok(())
            },
        )
        .unwrap();
}
#[test]
fn preset_change_or_unapproved_roots_cannot_be_applied() {
    let (f, mut config, path) = preset_fixture();
    let preview = f
        .service
        .preview_preset(
            &permit(&f.service),
            PresetPreviewRequest {
                preset_id: "custom".into(),
            },
        )
        .unwrap();
    config.max_entries += 1;
    fs::write(&path, json_codec::to_vec(&config).unwrap()).unwrap();
    let error = f
        .service
        .initialize_preset(
            &permit(&f.service),
            PresetSetupRequest {
                preset_id: "custom".into(),
                config_digest: preview["config_digest"].as_str().unwrap().into(),
                username: "admin".into(),
                password: "long-valid-password".into(),
            },
            |_, _, _| panic!(),
        )
        .unwrap_err();
    assert_eq!(error.code, storage_contract::ErrorCode::SetupPresetChanged);
    assert!(!f.service.data_root.join("setup-intent.json").exists());
    config.disks[0].root = f.service.management_root.clone();
    fs::write(&path, json_codec::to_vec(&config).unwrap()).unwrap();
    assert!(
        f.service
            .preview_preset(
                &permit(&f.service),
                PresetPreviewRequest {
                    preset_id: "custom".into()
                }
            )
            .is_err()
    );
}
#[test]
fn unavailable_or_unsafe_presets_do_not_prevent_status() {
    let (mut f, _, path) = preset_fixture();
    f.service
        .presets
        .insert("missing".into(), f._temp.path().join("not-present.json"));
    assert_eq!(f.service.status().state, InstallationState::Empty);
    assert_eq!(f.service.status().config_presets.len(), 2);
    assert!(
        f.service
            .preview_preset(
                &permit(&f.service),
                PresetPreviewRequest {
                    preset_id: "missing".into()
                }
            )
            .is_err()
    );
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(read_preset(&path).is_err());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();
    assert!(read_preset(&path).is_ok());
    let hard = f._temp.path().join("hard.json");
    fs::hard_link(&path, &hard).unwrap();
    assert!(read_preset(&path).is_err());
    fs::remove_file(hard).unwrap();
    let link = f._temp.path().join("link.json");
    std::os::unix::fs::symlink(&path, &link).unwrap();
    assert!(read_preset(&link).is_err());
    assert!(json_codec::from_str::<PresetSetupRequest>(r#"{"preset_id":"custom","config_digest":"x","username":"admin","password":"long-valid-password","config_path":"/tmp/evil"}"#).is_err());
}

#[test]
fn preset_builder_never_exposes_path_shaped_identifiers() {
    let f = fixture();
    let service = f.service.with_presets(BTreeMap::from([
        (
            "/private/config.json".into(),
            PathBuf::from("/private/config.json"),
        ),
        ("safe_id".into(), PathBuf::from("/unavailable")),
    ]));
    assert_eq!(service.status().config_presets, vec!["safe_id".to_string()]);
}

#[test]
fn local_setup_identifiers_keep_the_existing_ascii_language() {
    for id in [
        "A",
        "1",
        "_",
        "-",
        "1_Primary",
        "_preset",
        "-local",
        &"a".repeat(64),
    ] {
        assert!(valid_local_setup_id(id), "{id:?}");
    }
    for id in [
        "",
        "layout.prod",
        "磁盘",
        "a/b",
        "a b",
        "a\n",
        &"a".repeat(65),
    ] {
        assert!(!valid_local_setup_id(id), "{id:?}");
    }
}

#[test]
fn single_disk_setup_is_empty_and_creates_exactly_one_member() {
    let f = fixture_with_disks(&["one"]);
    assert_eq!(f.service.status().state, InstallationState::Empty);
    let mut input = request();
    input.disk_ids = vec!["one".into()];
    f.service
        .initialize(&permit(&f.service), input, |engine, _, _| {
            assert_eq!(engine.config().disks.len(), 1);
            assert_eq!(
                engine
                    .config()
                    .disks
                    .iter()
                    .filter(|d| d.metadata_member)
                    .count(),
                1
            );
            Ok(())
        })
        .unwrap();
    assert_eq!(f.service.status().state, InstallationState::Existing);
}

#[test]
fn single_disk_setup_retains_selection_and_mount_fences() {
    for ids in [vec![], vec!["one", "one"], vec!["unapproved"]] {
        let f = fixture_with_disks(&["one"]);
        let mut input = request();
        input.disk_ids = ids.into_iter().map(str::to_owned).collect();
        assert!(
            f.service
                .initialize(&permit(&f.service), input, |_, _, _| panic!())
                .is_err()
        );
        assert!(!f.service.data_root.join("config.json").exists());
        assert!(!f.service.data_root.join("setup-intent.json").exists());
    }
    let f = fixture_with_disks(&["one"]);
    let mut input = request();
    input.disk_ids = vec!["one".into()];
    input.mode = Mode::Production;
    assert!(
        f.service
            .initialize(&permit(&f.service), input, |_, _, _| panic!())
            .is_err()
    );
    assert!(!f.service.data_root.join("config.json").exists());
    fs::remove_dir(&f.service.disks["one"]).unwrap();
    assert_eq!(f.service.status().state, InstallationState::Unavailable);
    assert!(!f.service.disks["one"].exists());
}

#[test]
fn setup_rejects_storage_overlapping_system_control_directories() {
    for placement in [
        "management-child",
        "management",
        "system-child",
        "system",
        "ancestor",
    ] {
        let f = fixture_with_disks(&["one"]);
        let path = match placement {
            "management-child" => f.service.management_root.join("disk"),
            "management" => f.service.management_root.clone(),
            "system-child" => f.service.data_root.join("disk"),
            "system" => f.service.data_root.clone(),
            _ => f.service.data_root.parent().unwrap().to_owned(),
        };
        fs::create_dir_all(&path).unwrap();
        let service = SetupRecovery::new(
            f.service.data_root.clone(),
            f.service.management_root.clone(),
            [("one".into(), path)].into_iter().collect(),
        )
        .unwrap()
        .allow_development_mode();
        assert!(service.selected(&["one".into()]).is_err(), "{placement}");
        let mut input = request();
        input.disk_ids = vec!["one".into()];
        assert!(
            service
                .initialize(&permit(&service), input, |_, _, _| panic!())
                .is_err()
        );
        assert!(!service.data_root.join("setup-intent.json").exists());
    }
}

#[test]
fn restore_single_member_uses_original_membership_not_survivor_count() {
    for ids in [&["one"][..], &["one", "two", "three"][..]] {
        let f = fixture_with_disks(ids);
        drop(
            Engine::initialize(
                &f.service.data_root,
                Config::development(
                    ids.iter()
                        .map(|id| f.service.disks[*id].clone())
                        .collect::<Vec<_>>(),
                ),
            )
            .unwrap(),
        );
        let restored = f._temp.path().join("restored-system");
        fs::create_dir(&restored).unwrap();
        fs::set_permissions(&restored, fs::Permissions::from_mode(0o700)).unwrap();
        let recovery = SetupRecovery::new(
            restored.clone(),
            f.service.management_root.clone(),
            f.service.disks.clone(),
        )
        .unwrap();
        let error = recovery
            .restore_system(
                &permit(&recovery),
                RestoreRequest {
                    disk_ids: vec!["one".into()],
                    accept_stale_credentials: true,
                },
                |_| panic!("fixture contains no administrator credentials"),
            )
            .unwrap_err();
        if ids.len() == 1 {
            assert_eq!(
                error.code,
                storage_contract::ErrorCode::AuthRecoveryRequired
            );
            assert!(restored.join("config.json").exists());
        } else {
            assert_ne!(
                error.code,
                storage_contract::ErrorCode::AuthRecoveryRequired,
                "one surviving member must not shrink a three-member quorum"
            );
            assert!(!restored.join("config.json").exists());
        }
    }
}

#[test]
fn single_disk_interrupted_setup_still_requires_bound_completion_proof() {
    let f = fixture_with_disks(&["one"]);
    let mut input = request();
    input.disk_ids = vec!["one".into()];
    assert_eq!(
        f.service
            .initialize(&permit(&f.service), input, |_, _, _| Err(
                ApiError::conflict(
                    storage_contract::ErrorCode::InjectedFailure,
                    "test interruption"
                )
            ))
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::InjectedFailure
    );
    assert!(f.service.status().completion_available);
    f.service
        .complete_initialization(&permit(&f.service), completion(), |engine, user, _| {
            assert_eq!(engine.config().disks.len(), 1);
            assert_eq!(user, "admin");
            fs::write(
                f.service.data_root.join("management-authority.json"),
                b"claimed",
            )
            .unwrap();
            Ok(())
        })
        .unwrap();
    assert!(!f.service.status().completion_available);
}

thread_local! {
    static AFTER_RESTORE: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
    static AFTER_RECOVERY_OPEN: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        const { std::cell::RefCell::new(None) };
}

pub(super) fn after_next_restore(action: impl FnOnce() + 'static) {
    AFTER_RESTORE.with(|hook| *hook.borrow_mut() = Some(Box::new(action)));
}

pub(super) fn after_restore() {
    if let Some(action) = AFTER_RESTORE.with(|hook| hook.borrow_mut().take()) {
        action();
    }
}

pub(super) fn after_next_recovery_open(action: impl FnOnce() + 'static) {
    AFTER_RECOVERY_OPEN.with(|hook| *hook.borrow_mut() = Some(Box::new(action)));
}

pub(super) fn after_recovery_open() {
    if let Some(action) = AFTER_RECOVERY_OPEN.with(|hook| hook.borrow_mut().take()) {
        action();
    }
}

#[test]
fn bootstrap_restore_rechecks_authorization_after_disk_recovery() {
    for change in ["delete", "rotate", "expire", "revoke"] {
        let f = recoverable_fixture();
        let recovery = &f.service;
        let authorization = permit(recovery);
        let key = recovery.management_root.join("bootstrap.key");
        AFTER_RESTORE.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || match change {
                "delete" => fs::remove_file(key).unwrap(),
                "rotate" => fs::write(key, "f".repeat(64)).unwrap(),
                "expire" => fs::File::open(key)
                    .unwrap()
                    .set_times(fs::FileTimes::new().set_modified(
                        std::time::SystemTime::now() - std::time::Duration::from_secs(1801),
                    ))
                    .unwrap(),
                "revoke" => fs::set_permissions(key, fs::Permissions::from_mode(0o644)).unwrap(),
                _ => unreachable!(),
            }));
        });
        let result = recovery.restore_system(
            &authorization,
            RestoreRequest {
                disk_ids: vec!["one".into()],
                accept_stale_credentials: true,
            },
            |_| panic!("revoked local authorization must not publish credentials: {change}"),
        );
        assert!(result.is_err(), "{change}");
        assert!(
            recovery.data_root.join("config.json").exists(),
            "disk recovery completed: {change}"
        );
        assert!(
            !recovery
                .management_root
                .join("management-auth.json")
                .exists()
        );
        assert!(AFTER_RESTORE.with(|hook| hook.borrow().is_none()));
    }
}

fn recoverable_fixture() -> Fixture {
    let mut f = fixture_with_disks(&["one"]);
    let mut engine = Engine::initialize(
        &f.service.data_root,
        Config::development(vec![f.service.disks["one"].clone()]),
    )
    .unwrap();
    engine
        .persist_auth_state(
            crate::auth::AuthStore::initialize(
                &f._temp.path().join("snapshot-auth"),
                "admin",
                "long-test-password",
            )
            .unwrap()
            .snapshot()
            .unwrap(),
            "system_auth",
            &storage_engine::new_id("op"),
        )
        .unwrap();
    drop(engine);
    let restored = f._temp.path().join("restored-system");
    fs::create_dir(&restored).unwrap();
    fs::set_permissions(&restored, fs::Permissions::from_mode(0o700)).unwrap();
    f.service = SetupRecovery::new(
        restored.clone(),
        f.service.management_root.clone(),
        f.service.disks.clone(),
    )
    .unwrap();
    f
}

#[test]
fn authenticated_restore_does_not_require_bootstrap_authorization() {
    let f = recoverable_fixture();
    fs::remove_file(f.service.management_root.join("bootstrap.key")).unwrap();
    let mut published = false;
    f.service
        .restore_authenticated(
            &admin(),
            RestoreRequest {
                disk_ids: vec!["one".into()],
                accept_stale_credentials: false,
            },
            |engine| {
                assert!(engine.auth_state().is_some());
                published = true;
                Ok(())
            },
        )
        .unwrap();
    assert!(published);
}

#[test]
fn offline_engine_failures_preserve_http_outcome_and_retry_classification() {
    for (internal, public, status) in [
        (
            storage_contract::ErrorCode::IoError,
            storage_contract::ErrorCode::IoError,
            500,
        ),
        (
            storage_contract::ErrorCode::DurabilityUnknown,
            storage_contract::ErrorCode::DurabilityUnknown,
            500,
        ),
        (
            storage_contract::ErrorCode::NoSpace,
            storage_contract::ErrorCode::NoSpace,
            507,
        ),
        (
            storage_contract::ErrorCode::InDoubt,
            storage_contract::ErrorCode::CommitInDoubt,
            503,
        ),
        (
            storage_contract::ErrorCode::MetadataUnavailable,
            storage_contract::ErrorCode::MetadataUnavailable,
            503,
        ),
        (
            storage_contract::ErrorCode::RecoveryRequired,
            storage_contract::ErrorCode::RecoveryRequired,
            503,
        ),
        (
            storage_contract::ErrorCode::ResourceLimit,
            storage_contract::ErrorCode::ResourceLimitExceeded,
            429,
        ),
        (
            storage_contract::ErrorCode::PermissionDenied,
            storage_contract::ErrorCode::Forbidden,
            403,
        ),
        (
            storage_contract::ErrorCode::HeadConflict,
            storage_contract::ErrorCode::HeadConflict,
            409,
        ),
        (
            storage_contract::ErrorCode::InjectedFailure,
            storage_contract::ErrorCode::InjectedFailure,
            400,
        ),
    ] {
        let mapped = engine_error(storage_engine::Error::new(
            internal,
            "private /srv/zxs/disk/config.json diagnostic",
        ));
        assert_eq!(mapped.code, public);
        assert_eq!(mapped.status, status, "{internal}");
        assert_eq!(mapped.retryable, status >= 500 || status == 429);
        assert_eq!(
            storage_contract::client_protocol::mutation_outcome_unknown(mapped.status),
            status >= 500,
            "{internal} must not make uncertain storage writes look rejected"
        );
        assert!(!mapped.message.contains("/srv/zxs"));
    }
    assert!(
        engine_error(
            storage_engine::Error::new(storage_contract::ErrorCode::WorkspaceBusy, "private")
                .retryable()
        )
        .retryable
    );
}

fn raised_workspace_limits(config: &Config) -> RetainedWorkspaceLimits {
    RetainedWorkspaceLimits {
        max_retained_workspaces: config.max_retained_workspaces + 1,
        max_retained_workspaces_per_principal: config.max_retained_workspaces_per_principal + 1,
        max_retained_workspace_metadata_bytes: config.max_retained_workspace_metadata_bytes + 1,
        max_retained_workspace_metadata_bytes_per_principal: config
            .max_retained_workspace_metadata_bytes_per_principal
            + 1,
    }
}

fn assert_workspace_limits(engine: &Engine, limits: RetainedWorkspaceLimits) {
    let capacity = engine.workspace_capacity("admin");
    let global = capacity.global;
    assert_eq!(global.records.limit, limits.max_retained_workspaces as u64);
    assert_eq!(
        global.metadata_bytes.limit,
        limits.max_retained_workspace_metadata_bytes
    );
    assert_eq!(
        capacity.principal.records.limit,
        limits.max_retained_workspaces_per_principal as u64
    );
    assert_eq!(
        capacity.principal.metadata_bytes.limit,
        limits.max_retained_workspace_metadata_bytes_per_principal
    );
    assert!(!capacity.automatic_retirement);
}

#[test]
fn runtime_workspace_limits_below_selected_setup_baseline_do_not_publish_intent() {
    for preset in [false, true] {
        let (mut f, config, path) = preset_fixture();
        let mut limits = raised_workspace_limits(&config);
        if preset {
            // Exceeds the host limit but is still a valid locally reviewed preset.
            let mut selected = config.clone();
            selected.max_retained_workspaces = limits.max_retained_workspaces + 1;
            fs::write(&path, json_codec::to_vec(&selected).unwrap()).unwrap();
        } else {
            limits.max_retained_workspaces = config.max_retained_workspaces - 1;
        }
        f.service = f.service.with_workspace_limits(Some(limits)).unwrap();
        let error = if preset {
            let preview = f
                .service
                .preview_preset(
                    &permit(&f.service),
                    PresetPreviewRequest {
                        preset_id: "custom".into(),
                    },
                )
                .unwrap();
            f.service.initialize_preset(
                &permit(&f.service),
                PresetSetupRequest {
                    preset_id: "custom".into(),
                    config_digest: preview["config_digest"].as_str().unwrap().into(),
                    username: "admin".into(),
                    password: "long-valid-password".into(),
                },
                |_, _, _| panic!("invalid host limits cannot install authority"),
            )
        } else {
            f.service
                .initialize(&permit(&f.service), request(), |_, _, _| {
                    panic!("invalid host limits cannot install authority")
                })
        }
        .unwrap_err();
        assert_eq!(error.status, 400);
        assert!(!f.service.data_root.join("setup-intent.json").exists());
        assert!(!f.service.data_root.join("config.json").exists());
        assert!(
            !f.service
                .management_root
                .join("management-auth.json")
                .exists()
        );
        for disk in f.service.disks.values() {
            assert_eq!(fs::read_dir(disk).unwrap().count(), 0);
        }
    }
}

#[test]
fn initialization_and_bound_completion_keep_runtime_workspace_limits_out_of_config() {
    let mut f = fixture();
    let baseline = Config::development(f.service.disks.values().cloned());
    let limits = raised_workspace_limits(&baseline);
    f.service = f.service.with_workspace_limits(Some(limits)).unwrap();
    let error = f
        .service
        .initialize(&permit(&f.service), request(), |engine, _, _| {
            assert_workspace_limits(&engine, limits);
            assert_eq!(
                engine.config().max_retained_workspaces,
                baseline.max_retained_workspaces
            );
            Err(ApiError::conflict(
                storage_contract::ErrorCode::InjectedFailure,
                "test before authority publication",
            ))
        })
        .unwrap_err();
    assert_eq!(error.code, storage_contract::ErrorCode::InjectedFailure);
    let persisted = fs::read(f.service.data_root.join("config.json")).unwrap();
    f.service
        .complete_initialization(
            &permit(&f.service),
            CompleteSetupRequest {
                username: "admin".into(),
                password: "long-valid-password".into(),
            },
            |engine, _, _| {
                assert_workspace_limits(&engine, limits);
                Ok(())
            },
        )
        .unwrap();
    assert_eq!(
        fs::read(f.service.data_root.join("config.json")).unwrap(),
        persisted
    );
}

#[test]
fn system_restore_passes_runtime_workspace_limits_without_rewriting_baseline() {
    let mut f = recoverable_fixture();
    let original = fs::read(f._temp.path().join("system/config.json")).unwrap();
    let config: Config = storage_engine::decode_strict(&original).unwrap();
    let limits = raised_workspace_limits(&config);
    f.service = f.service.with_workspace_limits(Some(limits)).unwrap();
    f.service
        .restore_system(
            &permit(&f.service),
            RestoreRequest {
                disk_ids: vec!["one".into()],
                accept_stale_credentials: true,
            },
            |engine| {
                assert_workspace_limits(&engine, limits);
                assert_eq!(
                    json_codec::to_value(engine.config()).unwrap(),
                    json_codec::to_value(&config).unwrap()
                );
                Ok(())
            },
        )
        .unwrap();
    let restored: Config =
        storage_engine::decode_strict(&fs::read(f.service.data_root.join("config.json")).unwrap())
            .unwrap();
    assert_eq!(
        json_codec::to_value(restored).unwrap(),
        json_codec::to_value(config).unwrap()
    );
}
