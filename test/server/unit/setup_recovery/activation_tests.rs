use super::*;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
struct Fixture {
    _temp: tempfile::TempDir,
    service: SetupRecovery,
}
fn fixture() -> Fixture {
    fixture_with_disks(&["one", "two", "three"])
}
fn fixture_with_disks(ids: &[&str]) -> Fixture {
    let temp = tempfile::tempdir_in(std::env::current_dir().unwrap()).unwrap();
    let service = service_at(temp.path(), ids);
    Fixture {
        _temp: temp,
        service,
    }
}
fn service_at(base: &Path, ids: &[&str]) -> SetupRecovery {
    let data = base.join("system");
    let management = base.join("management");
    fs::create_dir(&data).unwrap();
    fs::set_permissions(&data, fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir(&management).unwrap();
    fs::set_permissions(&management, fs::Permissions::from_mode(0o700)).unwrap();
    let mut disks = BTreeMap::new();
    let discovery = base.join("disks");
    if !ids.is_empty() {
        fs::create_dir(&discovery).unwrap();
    }
    for &id in ids {
        let path = discovery.join(id);
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
    SetupRecovery::new(data, management, disks)
        .unwrap()
        .allow_development_mode()
}
fn admin() -> Identity {
    Identity {
        admission_session: None,
        smb_binding: None,
        principal: "admin".into(),
    }
}

fn account() -> CompleteSetupRequest {
    CompleteSetupRequest {
        username: "admin".into(),
        password: "long-valid-password".into(),
    }
}

fn initialize(service: &SetupRecovery) {
    service
        .initialize_management(account(), |username, password| {
            crate::auth::AuthStore::initialize_management(
                &service.management_root,
                username,
                password,
            )?;
            crate::management::claim_authority(&service.data_root)
        })
        .unwrap();
}

fn request(service: &SetupRecovery, ids: &[&str]) -> StorageActivationRequest {
    let root = service.disks[ids[0]]
        .parent()
        .unwrap()
        .to_str()
        .unwrap()
        .to_owned();
    let current = service.discovery_root().unwrap();
    if current.discovery_root != root {
        service
            .set_discovery_root(
                &admin(),
                storage_contract::storage_directories::SetDiscoveryRootRequest {
                    expected_discovery_root: current.discovery_root,
                    discovery_root: root.clone(),
                },
                |_| Ok(()),
            )
            .unwrap();
    }
    let selections = ids
        .iter()
        .map(|id| {
            let path = service.disks[*id].to_str().unwrap();
            let location = crate::storage_directories::browse(path).unwrap();
            crate::storage_directories::inspect_within(
                &root,
                &storage_contract::storage_directories::InspectRequest {
                    mount_path: location.mount_path,
                    root_path: path.into(),
                },
                &[service.data_root.clone(), service.management_root.clone()],
            )
            .unwrap()
        })
        .collect();
    StorageActivationRequest {
        selections,
        mode: Mode::Development,
    }
}

#[test]
fn management_initialization_needs_no_storage_directory_or_pool() {
    let f = fixture_with_disks(&[]);
    assert_eq!(f.service.status().state, InstallationState::Empty);
    fs::remove_file(f.service.management_root.join("bootstrap.key")).unwrap();
    initialize(&f.service);
    assert_eq!(f.service.status().state, InstallationState::ManagementReady);
    assert_eq!(
        f.service.storage_status().state,
        StorageActivationState::Pending
    );
    assert!(!f.service.data_root.join("config.json").exists());
    assert!(!f.service.data_root.join("workspaces").exists());
    assert!(!f.service.data_root.join(ACTIVATION_INTENT).exists());
}

#[test]
fn unrelated_unavailable_and_nonempty_directories_do_not_block_account_setup() {
    let f = fixture();
    fs::remove_dir(&f.service.disks["one"]).unwrap();
    fs::write(f.service.disks["two"].join("unmanaged"), b"preserved").unwrap();
    initialize(&f.service);
    assert_eq!(
        f.service.storage_status().state,
        StorageActivationState::Pending
    );
    assert_eq!(
        fs::read(f.service.disks["two"].join("unmanaged")).unwrap(),
        b"preserved"
    );
    assert!(!f.service.disks["one"].exists());
}

#[test]
fn initialized_management_never_resets_credentials() {
    let f = fixture();
    initialize(&f.service);
    let before = fs::read(f.service.management_root.join("management-auth.json")).unwrap();
    assert!(
        f.service
            .initialize_management(account(), |_, _| panic!())
            .is_err()
    );
    assert_eq!(
        fs::read(f.service.management_root.join("management-auth.json")).unwrap(),
        before
    );
}

#[test]
fn uncommitted_account_intent_can_retry_only_before_authority_exists() {
    let f = fixture_with_disks(&[]);
    let error = f
        .service
        .initialize_management(account(), |_, _| {
            Err(ApiError::conflict(
                storage_contract::ErrorCode::InjectedFailure,
                "before authority",
            ))
        })
        .unwrap_err();
    assert_eq!(error.code, storage_contract::ErrorCode::InjectedFailure);
    assert_eq!(f.service.status().state, InstallationState::Empty);
    initialize(&f.service);
    assert_eq!(f.service.status().state, InstallationState::ManagementReady);
}

#[test]
fn first_pair_genesis_retains_fixed_quorum_and_reopens() {
    let f = fixture();
    initialize(&f.service);
    f.service
        .activate(
            &admin(),
            request(&f.service, &["one", "two"]),
            |_| Ok(()),
            |engine| {
                assert_eq!(engine.config().disks.len(), 2);
                assert_eq!(engine.config().metadata_quorum(), 2);
                assert_eq!(engine.state().topology_generation, 1);
                Ok(())
            },
        )
        .unwrap();
    assert!(f.service.activation_admitted().is_ok());
    assert_eq!(
        f.service.storage_status().state,
        StorageActivationState::Active
    );
    let engine = Engine::open(&f.service.data_root).unwrap();
    assert_eq!(engine.config().disks.len(), 2);
    assert!(
        fs::read_dir(&f.service.disks["three"])
            .unwrap()
            .next()
            .is_none()
    );
}

#[test]
fn first_single_disk_is_not_a_zero_member_pool() {
    let f = fixture();
    initialize(&f.service);
    f.service
        .activate(
            &admin(),
            request(&f.service, &["one"]),
            |_| Ok(()),
            |engine| {
                assert_eq!(engine.config().metadata_quorum(), 1);
                assert_eq!(engine.config().disks.len(), 1);
                Ok(())
            },
        )
        .unwrap();
}

#[test]
fn nonempty_selected_directory_is_rejected_before_intent_and_preserved() {
    let f = fixture();
    initialize(&f.service);
    let selected = request(&f.service, &["one"]);
    fs::write(f.service.disks["one"].join("important"), b"unchanged").unwrap();
    assert!(
        f.service
            .activate(&admin(), selected, |_| Ok(()), |_| panic!())
            .is_err()
    );
    assert!(!f.service.data_root.join(ACTIVATION_INTENT).exists());
    assert_eq!(
        fs::read(f.service.disks["one"].join("important")).unwrap(),
        b"unchanged"
    );
}

#[test]
fn failed_replica_installation_fences_admission_and_can_complete() {
    let f = fixture();
    initialize(&f.service);
    let error = f
        .service
        .activate(
            &admin(),
            request(&f.service, &["one"]),
            |_| Ok(()),
            |_| {
                Err(ApiError::conflict(
                    storage_contract::ErrorCode::InjectedFailure,
                    "replica interrupted",
                ))
            },
        )
        .unwrap_err();
    assert_eq!(error.code, storage_contract::ErrorCode::InjectedFailure);
    assert_eq!(
        f.service.storage_status().state,
        StorageActivationState::RecoveryRequired
    );
    assert!(f.service.storage_status().completion_available);
    assert!(f.service.activation_admitted().is_err());
    assert!(
        f.service
            .activate(
                &admin(),
                request(&f.service, &["two"]),
                |_| Ok(()),
                |_| panic!()
            )
            .is_err()
    );
    f.service
        .complete_activation(&admin(), |_| Ok(()), |_| Ok(()))
        .unwrap();
    assert!(f.service.activation_admitted().is_ok());
    f.service
        .complete_activation(
            &admin(),
            |_| Ok(()),
            |_| panic!("completed retry must not reinstall"),
        )
        .unwrap();
}

#[test]
fn revoked_authority_after_storage_creation_never_completes_activation() {
    let f = fixture();
    initialize(&f.service);
    let mut checks = 0;
    let error = f
        .service
        .activate(
            &admin(),
            request(&f.service, &["one"]),
            |_| {
                checks += 1;
                if checks == 3 {
                    Err(ApiError::unauthorized())
                } else {
                    Ok(())
                }
            },
            |_| panic!(),
        )
        .unwrap_err();
    assert_eq!(error.status, ::http::StatusCode::UNAUTHORIZED);
    assert!(f.service.data_root.join("config.json").exists());
    assert!(!f.service.data_root.join(ACTIVATION_COMPLETE).exists());
    assert!(f.service.activation_admitted().is_err());
}

#[test]
fn missing_config_after_activation_never_looks_unconfigured() {
    let f = fixture();
    initialize(&f.service);
    f.service
        .activate(
            &admin(),
            request(&f.service, &["one"]),
            |_| Ok(()),
            |_| Ok(()),
        )
        .unwrap();
    fs::remove_file(f.service.data_root.join("config.json")).unwrap();
    assert_eq!(
        f.service.status().state,
        InstallationState::RecoveryRequired
    );
    assert!(
        f.service
            .activate(
                &admin(),
                request(&f.service, &["two"]),
                |_| Ok(()),
                |_| panic!()
            )
            .is_err()
    );
}

#[test]
fn changed_config_or_seal_blocks_completion_without_resetting_data() {
    for change_config in [true, false] {
        let f = fixture();
        initialize(&f.service);
        let _ = f.service.activate(
            &admin(),
            request(&f.service, &["one"]),
            |_| Ok(()),
            |_| Err(ApiError::unauthorized()),
        );
        let path = if change_config {
            f.service.data_root.join("config.json")
        } else {
            f.service.disks["one"].join("disk.json")
        };
        let bytes = fs::read(&path).unwrap();
        let mut value: json_codec::Value = json_codec::from_slice(&bytes).unwrap();
        value["pool_id"] = json_codec::json!("pool_ffffffffffffffffffffffffffffffff");
        fs::write(&path, json_codec::to_vec(&value).unwrap()).unwrap();
        let changed = fs::read(&path).unwrap();
        assert!(
            f.service
                .complete_activation(&admin(), |_| Ok(()), |_| panic!())
                .is_err()
        );
        assert_eq!(fs::read(&path).unwrap(), changed);
    }
}

#[test]
fn complete_genesis_without_ready_receipt_is_checked_and_completed_not_reinitialized() {
    let f = fixture();
    initialize(&f.service);
    let _ = f.service.activate(
        &admin(),
        request(&f.service, &["one"]),
        |_| Ok(()),
        |_| Err(ApiError::unauthorized()),
    );
    fs::remove_file(f.service.data_root.join(ACTIVATION_READY)).unwrap();
    let seal_before = fs::read(f.service.disks["one"].join("disk.json")).unwrap();
    f.service
        .complete_activation(&admin(), |_| Ok(()), |_| Ok(()))
        .unwrap();
    assert!(f.service.activation_admitted().is_ok());
    assert_eq!(
        fs::read(f.service.disks["one"].join("disk.json")).unwrap(),
        seal_before
    );
}

#[test]
fn account_generation_cannot_admit_a_pool_with_lost_activation_evidence() {
    let f = fixture();
    initialize(&f.service);
    f.service
        .activate(
            &admin(),
            request(&f.service, &["one"]),
            |_| Ok(()),
            |_| Ok(()),
        )
        .unwrap();
    fs::remove_file(f.service.data_root.join(ACTIVATION_INTENT)).unwrap();
    fs::remove_file(f.service.data_root.join(ACTIVATION_COMPLETE)).unwrap();
    assert!(f.service.activation_admitted().is_err());
}

#[test]
fn activation_completion_replays_published_auth_without_recreating_account_or_whitelist() {
    let f = fixture();
    initialize(&f.service);
    let snapshot = crate::auth::AuthStore::open_management(&f.service.management_root)
        .unwrap()
        .snapshot()
        .unwrap();
    let credentials = fs::read(f.service.management_root.join("management-auth.json")).unwrap();
    let error = f
        .service
        .activate(
            &admin(),
            request(&f.service, &["one"]),
            |_| Ok(()),
            |engine| {
                crate::engine_backend::EngineBackend::new(engine)
                    .replicate_auth(snapshot.clone())?;
                Err(ApiError::conflict(
                    storage_contract::ErrorCode::InjectedFailure,
                    "receipt interrupted after replica publication",
                ))
            },
        )
        .unwrap_err();
    assert_eq!(error.code, storage_contract::ErrorCode::InjectedFailure);
    let engine = Engine::open(&f.service.data_root).unwrap();
    let sequence = engine.state().sequence;
    assert!(sequence > 0);
    assert_eq!(engine.auth_state(), Some(snapshot.clone()));
    drop(engine);
    let restarted = SetupRecovery::new(
        f.service.data_root.clone(),
        f.service.management_root.clone(),
        BTreeMap::new(),
    )
    .unwrap();
    assert!(restarted.storage_status().completion_available);
    restarted
        .complete_activation(
            &admin(),
            |_| Ok(()),
            |engine| {
                assert_eq!(engine.state().sequence, sequence);
                crate::engine_backend::EngineBackend::new(engine).replicate_auth(snapshot.clone())
            },
        )
        .unwrap();
    assert_eq!(
        fs::read(f.service.management_root.join("management-auth.json")).unwrap(),
        credentials
    );
    let engine = Engine::open(&f.service.data_root).unwrap();
    assert_eq!(engine.state().sequence, sequence);
    assert_eq!(engine.auth_state(), Some(snapshot));
}

#[test]
fn keyless_account_creation_serializes_to_one_credential_writer() {
    let f = fixture_with_disks(&[]);
    fs::remove_file(f.service.management_root.join("bootstrap.key")).unwrap();
    let (entered, observed) = std::sync::mpsc::channel();
    let (release, proceed) = std::sync::mpsc::channel();
    std::thread::scope(|scope| {
        let service = &f.service;
        let first = scope.spawn(move || {
            service.initialize_management(account(), |username, password| {
                entered.send(()).unwrap();
                proceed.recv().unwrap();
                let _auth = crate::auth::AuthStore::initialize_management(
                    &service.management_root,
                    username,
                    password,
                )?;
                crate::management::claim_authority(&service.data_root)
            })
        });
        observed.recv().unwrap();
        let second =
            service.initialize_management(account(), |_, _| panic!("second credential writer"));
        assert_eq!(
            second.unwrap_err().code,
            storage_contract::ErrorCode::SetupBusy
        );
        release.send(()).unwrap();
        first.join().unwrap().unwrap();
    });
    assert_eq!(f.service.status().state, InstallationState::ManagementReady);
    assert!(
        f.service
            .initialize_management(account(), |_, _| panic!())
            .is_err()
    );
}

#[test]
fn durable_credentials_before_authority_failure_restart_without_password_reset() {
    let f = fixture_with_disks(&[]);
    let error = f
        .service
        .initialize_management(account(), |username, password| {
            let _auth = crate::auth::AuthStore::initialize_management(
                &f.service.management_root,
                username,
                password,
            )?;
            Err(ApiError::conflict(
                storage_contract::ErrorCode::InjectedFailure,
                "authority publication not attempted",
            ))
        })
        .unwrap_err();
    assert_eq!(error.code, storage_contract::ErrorCode::InjectedFailure);
    assert!(
        !f.service
            .data_root
            .join("management-authority.json")
            .exists()
    );
    let before = fs::read(f.service.management_root.join("management-auth.json")).unwrap();
    assert!(
        f.service
            .initialize_management(account(), |_, _| panic!())
            .is_err()
    );
    let (_backend, mut auth) =
        crate::management::open_service(&f.service.data_root, &f.service.management_root).unwrap();
    assert!(auth.is_available());
    assert_eq!(
        fs::read(f.service.management_root.join("management-auth.json")).unwrap(),
        before
    );
    assert!(auth.login("admin", "long-valid-password").is_ok());
    assert_eq!(f.service.status().state, InstallationState::ManagementReady);
    assert!(
        f.service
            .initialize_management(account(), |_, _| panic!())
            .is_err()
    );
}

#[test]
fn discovery_defaults_without_touching_storage_and_persists_compare_and_swap() {
    use storage_contract::storage_directories::{DEFAULT_DISCOVERY_ROOT, SetDiscoveryRootRequest};
    let f = fixture();
    assert_eq!(
        f.service.discovery_root().unwrap().discovery_root,
        "/zxs/data/disks"
    );
    assert_eq!(fs::read_dir(&f.service.data_root).unwrap().count(), 0);
    initialize(&f.service);
    let root = f.service.disks["one"].parent().unwrap();
    let mode = fs::metadata(root).unwrap().mode();
    let settings = f
        .service
        .set_discovery_root(
            &admin(),
            SetDiscoveryRootRequest {
                expected_discovery_root: DEFAULT_DISCOVERY_ROOT.into(),
                discovery_root: root.to_str().unwrap().into(),
            },
            |_| Ok(()),
        )
        .unwrap();
    assert_eq!(fs::metadata(root).unwrap().mode(), mode);
    assert_eq!(fs::read_dir(root).unwrap().count(), 3);
    let fresh = SetupRecovery::new(
        f.service.data_root.clone(),
        f.service.management_root.clone(),
        BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(
        fresh.discovery_root().unwrap().discovery_root,
        settings.discovery_root
    );
    assert_eq!(
        f.service
            .set_discovery_root(
                &admin(),
                SetDiscoveryRootRequest {
                    expected_discovery_root: DEFAULT_DISCOVERY_ROOT.into(),
                    discovery_root: root.to_str().unwrap().into()
                },
                |_| Ok(())
            )
            .unwrap_err()
            .code,
        storage_contract::ErrorCode::ConfigConflict
    );
}

#[test]
fn discovery_root_changes_require_current_authority_and_existing_safe_directory() {
    use storage_contract::storage_directories::{DEFAULT_DISCOVERY_ROOT, SetDiscoveryRootRequest};
    let f = fixture();
    let make = |path: &Path| SetDiscoveryRootRequest {
        expected_discovery_root: DEFAULT_DISCOVERY_ROOT.into(),
        discovery_root: path.to_str().unwrap().into(),
    };
    let root = f.service.disks["one"].parent().unwrap();
    assert!(
        f.service
            .set_discovery_root(&admin(), make(root), |_| Ok(()))
            .is_err()
    );
    initialize(&f.service);
    assert!(
        f.service
            .set_discovery_root(&admin(), make(root), |_| Err(ApiError::unauthorized()))
            .is_err()
    );
    for path in [
        f.service.data_root.clone(),
        f.service.management_root.clone(),
        f._temp.path().join("missing"),
    ] {
        assert!(
            f.service
                .set_discovery_root(&admin(), make(&path), |_| Ok(()))
                .is_err()
        );
    }
    assert!(!f._temp.path().join("missing").exists());
    assert_eq!(
        f.service.discovery_root().unwrap().discovery_root,
        DEFAULT_DISCOVERY_ROOT
    );
}

#[test]
fn changed_discovery_root_rejects_old_preview_before_intent_or_disk_writes() {
    use storage_contract::storage_directories::SetDiscoveryRootRequest;
    let f = fixture();
    initialize(&f.service);
    let selected = request(&f.service, &["one"]);
    let old = f.service.discovery_root().unwrap().discovery_root;
    let other = f._temp.path().join("other");
    fs::create_dir(&other).unwrap();
    f.service
        .set_discovery_root(
            &admin(),
            SetDiscoveryRootRequest {
                expected_discovery_root: old,
                discovery_root: other.to_str().unwrap().into(),
            },
            |_| Ok(()),
        )
        .unwrap();
    assert!(
        f.service
            .activate(&admin(), selected, |_| Ok(()), |_| panic!())
            .is_err()
    );
    assert!(!f.service.data_root.join(ACTIVATION_INTENT).exists());
    assert_eq!(fs::read_dir(&f.service.disks["one"]).unwrap().count(), 0);
}

#[test]
fn pending_activation_fences_discovery_changes_and_completion_keeps_original_roots() {
    use storage_contract::storage_directories::SetDiscoveryRootRequest;
    let f = fixture();
    initialize(&f.service);
    assert!(
        f.service
            .activate(
                &admin(),
                request(&f.service, &["one"]),
                |_| Ok(()),
                |_| Err(ApiError::unauthorized())
            )
            .is_err()
    );
    let old = f.service.discovery_root().unwrap().discovery_root;
    let other = f._temp.path().join("other");
    fs::create_dir(&other).unwrap();
    let change = || SetDiscoveryRootRequest {
        expected_discovery_root: old.clone(),
        discovery_root: other.to_str().unwrap().into(),
    };
    assert!(
        f.service
            .set_discovery_root(&admin(), change(), |_| Ok(()))
            .is_err()
    );
    f.service
        .complete_activation(&admin(), |_| Ok(()), |_| Ok(()))
        .unwrap();
    f.service
        .set_discovery_root(&admin(), change(), |_| Ok(()))
        .unwrap();
    f.service
        .complete_activation(
            &admin(),
            |_| Ok(()),
            |_| panic!("completed activation is validation only"),
        )
        .unwrap();
    assert_eq!(
        f.service.storage_status().state,
        StorageActivationState::Active
    );
}

#[test]
fn durable_control_reads_reject_nonprivate_permissions_on_held_file() {
    let f = fixture();
    initialize(&f.service);
    let path = f.service.data_root.join(MANAGEMENT_INTENT);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(f.service.management_intent().is_err());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    f.service
        .activate(
            &admin(),
            request(&f.service, &["one"]),
            |_| Ok(()),
            |_| Ok(()),
        )
        .unwrap();
    let path = f.service.data_root.join(ACTIVATION_COMPLETE);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o666)).unwrap();
    assert!(f.service.activation_admitted().is_err());
    assert_ne!(
        f.service.storage_status().state,
        StorageActivationState::Active
    );
}

/// Run only in isolated children through activation-io.sh with the real libc interposer.
#[test]
#[ignore = "requires isolated control file and directory fsync failure injection"]
fn published_control_requires_durable_retry_before_account_or_storage_admission() {
    let root = PathBuf::from(std::env::var_os("ZXS_ATOMIC_ROOT").unwrap());
    let trigger = PathBuf::from(std::env::var_os("ZXS_ATOMIC_TRIGGER").unwrap());
    let recovery = PathBuf::from(std::env::var_os("ZXS_ATOMIC_RECOVERY_TRIGGER").unwrap());
    let log = PathBuf::from(std::env::var_os("ZXS_ATOMIC_LOG").unwrap());
    let mode = std::env::var("ZXS_ACTIVATION_IO_MODE").unwrap();
    let stage = std::env::var("ZXS_ATOMIC_STAGE").unwrap();
    let service = service_at(root.parent().unwrap(), &["one"]);
    if mode == "management" {
        fs::write(&trigger, b"").unwrap();
        let result = service.initialize_management(account(), |_, _| {
            assert_eq!(stage, "published-file-sync");
            Err(ApiError::conflict(
                storage_contract::ErrorCode::InjectedFailure,
                "stop before credentials",
            ))
        });
        assert!(result.is_err());
        assert!(root.join(MANAGEMENT_INTENT).exists());
        assert!(
            !service
                .management_root
                .join("management-auth.json")
                .exists()
        );
        assert!(
            service
                .initialize_management(account(), |_, _| panic!(
                    "undurable intent reached credential writer"
                ))
                .is_err()
        );
        assert_eq!(service.status().state, InstallationState::Unavailable);
        fs::write(&recovery, b"").unwrap();
        initialize(&service);
        assert_eq!(service.status().state, InstallationState::ManagementReady);
    } else {
        assert_eq!(mode, "activation");
        initialize(&service);
        let result = service.activate(
            &admin(),
            request(&service, &["one"]),
            |_| Ok(()),
            |_| {
                fs::write(&trigger, b"").unwrap();
                Ok(())
            },
        );
        assert_eq!(result.is_ok(), stage == "published-file-sync");
        assert!(root.join(ACTIVATION_COMPLETE).exists());
        assert!(service.activation_admitted().is_err());
        assert!(
            service
                .complete_activation(
                    &admin(),
                    |_| Ok(()),
                    |_| panic!("undurable receipt reopened engine")
                )
                .is_err()
        );
        assert_ne!(
            service.storage_status().state,
            StorageActivationState::Active
        );
        fs::write(&recovery, b"").unwrap();
        service
            .complete_activation(
                &admin(),
                |_| Ok(()),
                |_| panic!("receipt retry is validation only"),
            )
            .unwrap();
        assert_eq!(
            service.storage_status().state,
            StorageActivationState::Active
        );
    }
    let events = fs::read_to_string(log).unwrap();
    assert!(events.contains("file-sync\ndirectory-sync\n"));
    assert!(events.contains("published-file-sync\ndirectory-sync\n"));
    println!("ZXS_ACTIVATION_IO_COMPLETE mode={mode} stage={stage}");
}
