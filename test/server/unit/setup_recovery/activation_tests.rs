use super::*;
use std::io::Write;
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

#[path = "primary_import_recovery_tests.rs"]
mod primary_import_recovery;
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
        .with_discovery_root(discovery.to_str().unwrap().parse().unwrap())
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
                assert!(
                    engine
                        .config()
                        .disks
                        .iter()
                        .all(|disk| { disk.level == 1 && disk.upstream_slot_id.is_none() })
                );
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
        engine
            .config()
            .disks
            .iter()
            .all(|disk| { disk.level == 1 && disk.upstream_slot_id.is_none() })
    );
    assert!(
        fs::read_dir(&f.service.disks["three"])
            .unwrap()
            .next()
            .is_none()
    );
}

#[test]
fn interrupted_independent_roots_complete_without_rebinding_or_backup_edges() {
    let f = fixture();
    initialize(&f.service);
    let failure = f.service.activate(
        &admin(),
        request(&f.service, &["two", "one"]),
        |_| Ok(()),
        |_| Err(ApiError::unauthorized()),
    );
    assert!(failure.is_err());
    let original: Config = read_control(&f.service.data_root.join("config.json")).unwrap();
    let seals = original
        .disks
        .iter()
        .map(|disk| fs::read(disk.root.join("disk.json")).unwrap())
        .collect::<Vec<_>>();
    let restarted = SetupRecovery::new(
        f.service.data_root.clone(),
        f.service.management_root.clone(),
        BTreeMap::new(),
    )
    .unwrap()
    .with_discovery_root(f.service.discovery_root.clone());
    restarted
        .complete_activation(
            &admin(),
            |_| Ok(()),
            |engine| {
                assert_eq!(value_digest(engine.config())?, value_digest(&original)?);
                assert!(
                    engine
                        .config()
                        .disks
                        .iter()
                        .all(|disk| { disk.level == 1 && disk.upstream_slot_id.is_none() })
                );
                Ok(())
            },
        )
        .unwrap();
    restarted.activation_admitted().unwrap();
    for (disk, seal) in original.disks.iter().zip(seals) {
        assert_eq!(fs::read(disk.root.join("disk.json")).unwrap(), seal);
    }
}

#[test]
fn competing_first_activations_publish_one_identity_and_preserve_the_losing_root() {
    let f = fixture();
    initialize(&f.service);
    let selections = [request(&f.service, &["one"]), request(&f.service, &["two"])];
    let barrier = std::sync::Barrier::new(2);
    let outcomes = std::thread::scope(|scope| {
        let workers = selections.map(|selected| {
            let service = &f.service;
            let barrier = &barrier;
            scope.spawn(move || {
                let mut first = true;
                service.activate(
                    &admin(),
                    selected,
                    |_| {
                        if first {
                            first = false;
                            barrier.wait();
                        }
                        Ok(())
                    },
                    |_| Ok(()),
                )
            })
        });
        workers.map(|worker| worker.join().unwrap())
    });
    assert_eq!(outcomes.iter().filter(|result| result.is_ok()).count(), 1);
    let rejected = outcomes.into_iter().find_map(Result::err).unwrap();
    assert!(matches!(
        rejected.code,
        storage_contract::ErrorCode::SetupBusy | storage_contract::ErrorCode::RecoveryRequired
    ));
    f.service.activation_admitted().unwrap();
    let engine = Engine::open(&f.service.data_root).unwrap();
    let [disk] = engine.config().disks.as_slice() else {
        panic!("exactly one first activation must win")
    };
    assert_eq!(disk.level, 1);
    assert!(disk.upstream_slot_id.is_none());
    for name in ["one", "two"] {
        if f.service.disks[name] != disk.root {
            assert!(
                fs::read_dir(&f.service.disks[name])
                    .unwrap()
                    .next()
                    .is_none()
            );
        }
    }
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
fn unbrowseable_disk_names_are_rejected_before_activation_intent_or_writes() {
    use storage_contract::ErrorCode;
    for (names, expected) in [
        (["CON", "other"], ErrorCode::InvalidRequest),
        (["a:b", "other"], ErrorCode::InvalidRequest),
        (["cafe\u{301}", "other"], ErrorCode::InvalidRequest),
        (["Disk", "disk"], ErrorCode::NameConflict),
        (["Straße", "STRASSE"], ErrorCode::NameConflict),
    ] {
        let mut f = fixture_with_disks(&["one", "two"]);
        for (id, name) in ["one", "two"].into_iter().zip(names) {
            let path = f.service.disks.get_mut(id).unwrap();
            let renamed = path.parent().unwrap().join(name);
            fs::rename(&*path, &renamed).unwrap();
            *path = renamed;
        }
        initialize(&f.service);
        let selected = request(&f.service, &["one", "two"]);
        let error = f
            .service
            .activate(
                &admin(),
                selected,
                |_| Ok(()),
                |_| panic!("invalid namespace installed"),
            )
            .unwrap_err();
        assert_eq!(error.code, expected, "{names:?}");
        assert_eq!(f.service.status().state, InstallationState::ManagementReady);
        for name in [
            ACTIVATION_INTENT,
            ACTIVATION_READY,
            ACTIVATION_COMPLETE,
            "config.json",
        ] {
            assert!(!f.service.data_root.join(name).try_exists().unwrap());
        }
        for path in f.service.disks.values() {
            assert!(fs::read_dir(path).unwrap().next().is_none());
        }
    }
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

fn activated_engine(service: &SetupRecovery, data_only_target: bool) -> Engine {
    initialize(service);
    let request = request(service, &["one", "two", "three"]);
    let mut config = Config::development(
        request
            .selections
            .iter()
            .map(|selection| PathBuf::from(&selection.root_path)),
    )
    .unwrap();
    config.disks[0].metadata_member = !data_only_target;
    let intent = StorageActivationIntent {
        format: ActivationFormat::InitialStorage,
        config: ValidatedConfig::try_from(config).unwrap(),
        selections: request.selections,
    };
    service.write_control(ACTIVATION_INTENT, &intent).unwrap();
    let directories = service.activation_directories(&intent).unwrap();
    let engine =
        Engine::initialize_held(&service.data_root, intent.config.clone(), directories).unwrap();
    service
        .write_activation_ready(&intent, service.activation_seals(&intent).unwrap())
        .unwrap();
    service
        .write_control(
            ACTIVATION_COMPLETE,
            &StorageActivationComplete {
                intent_digest: value_digest(&intent).unwrap(),
            },
        )
        .unwrap();
    engine
}

fn publish_activation_source(engine: &mut Engine) {
    let slot = engine.config().disks[0].slot_id.clone();
    engine
        .create_repository(
            &storage_contract::RepositoryName::parse("activation-exit-source").unwrap(),
            &slot,
            1,
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
}

fn exit_activation_target(engine: &mut Engine, action: storage_engine::MembershipExitAction) {
    publish_activation_source(engine);
    let disk = engine.config().disks[0].clone();
    engine
        .begin_disk_exit(
            &disk.disk_id,
            action,
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    let archive = engine.build_managed_membership_control_archive().unwrap();
    engine
        .enroll_pending_members(&archive.archive_path, &archive.root_hash, Vec::new())
        .unwrap();
    engine.resume_membership_joint_decision().unwrap();
    engine.activate_pending_disk_exit().unwrap();
}

fn add_activation_successor(engine: &mut Engine, base: &Path) {
    publish_activation_source(engine);
    let mut target = engine.config().clone();
    let additions =
        Config::development((0..4).map(|index| base.join(format!("added-{index}")))).unwrap();
    let added = additions.disks.into_iter().nth(3).unwrap();
    fs::create_dir(&added.root).unwrap();
    fs::set_permissions(&added.root, fs::Permissions::from_mode(0o700)).unwrap();
    target.disks.push(added.clone());
    engine
        .begin_membership_handoff(
            target,
            "admin",
            storage_contract::OperationId::generate().unwrap(),
        )
        .unwrap();
    let archive = engine.build_managed_membership_control_archive().unwrap();
    engine
        .enroll_pending_members(
            &archive.archive_path,
            &archive.root_hash,
            vec![storage_engine::PendingMemberDirectory {
                disk_id: added.disk_id,
                directory: File::open(added.root).unwrap(),
            }],
        )
        .unwrap();
    engine.resume_membership_joint_decision().unwrap();
    engine.activate_pending_membership().unwrap();
}

fn lose_activation_completion(service: &SetupRecovery, mode: &str) {
    let path = service.data_root.join(ACTIVATION_COMPLETE);
    if mode == "missing" {
        fs::remove_file(path).unwrap();
        return;
    }
    let bytes = match mode {
        "corrupt" => b"{incomplete".to_vec(),
        "mismatched" => json_codec::to_vec(&StorageActivationComplete {
            intent_digest: "0".repeat(64),
        })
        .unwrap(),
        other => panic!("unknown completion loss: {other}"),
    };
    fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)
        .unwrap()
        .write_all(&bytes)
        .unwrap();
}

#[test]
fn lifecycle_exit_cannot_replay_original_activation_selections_with_stale_or_equal_config() {
    for data_only in [false, true] {
        for action in [
            storage_engine::MembershipExitAction::PlannedOffline,
            storage_engine::MembershipExitAction::Deregister,
        ] {
            let f = fixture();
            let mut engine = activated_engine(&f.service, data_only);
            let original = fs::read(f.service.data_root.join("config.json")).unwrap();
            exit_activation_target(&mut engine, action);
            if data_only {
                let initial: Config = json_codec::from_slice(&original).unwrap();
                assert_eq!(
                    value_digest(engine.config()).unwrap(),
                    value_digest(&initial).unwrap(),
                );
                // A data-only Exit changes authority/registration, not Config.
                assert!(f.service.activation_admitted().is_ok());
            }
            drop(engine);
            fs::write(f.service.data_root.join("config.json"), original).unwrap();
            let detached = f._temp.path().join("physically-detached");
            fs::rename(&f.service.disks["one"], &detached).unwrap();
            let seal = fs::read(detached.join("disk.json")).unwrap();
            for receipt in ["missing", "corrupt", "mismatched"] {
                lose_activation_completion(&f.service, receipt);
                let status = f.service.storage_status();
                assert!(!status.completion_available);
                assert_ne!(status.state, StorageActivationState::Active);
                assert!(
                    f.service
                        .complete_activation(&admin(), |_| Ok(()), |_| panic!("replayed genesis"),)
                        .is_err()
                );
            }
            assert_eq!(fs::read(detached.join("disk.json")).unwrap(), seal);
        }
    }
}

#[test]
fn initial_probe_guard_survives_until_real_held_resources_and_writer_lock_take_over() {
    let f = fixture();
    let engine = activated_engine(&f.service, false);
    drop(engine);
    lose_activation_completion(&f.service, "missing");
    let proof = f.service.activation_proof().unwrap();
    assert!(
        Engine::begin_initial_activation_probe(&f.service.data_root, &proof.intent.config).is_err()
    );
    let directories = f.service.activation_directories(&proof.intent).unwrap();
    let engine =
        Engine::open_initial_activation_for_recovery_held(proof.probe, None, directories).unwrap();
    assert!(Engine::open(&f.service.data_root).is_err());
    drop(engine);
    f.service
        .complete_activation(&admin(), |_| Ok(()), |_| Ok(()))
        .unwrap();
    assert!(f.service.activation_admitted().is_ok());
}

fn target_directory_handles(path: &Path) -> usize {
    fs::read_dir("/proc/self/fd")
        .unwrap()
        .filter(
            |entry| match fs::read_link(entry.as_ref().unwrap().path()) {
                Ok(target) => target == path,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                Err(error) => panic!("cannot inspect probe descriptor: {error}"),
            },
        )
        .count()
}

#[test]
fn failed_initial_probe_transfer_releases_raw_directories_and_the_guard() {
    let f = fixture();
    drop(activated_engine(&f.service, false));
    lose_activation_completion(&f.service, "missing");
    let proof = f.service.activation_proof().unwrap();
    let directories = f.service.activation_directories(&proof.intent).unwrap();
    assert_eq!(target_directory_handles(&f.service.disks["one"]), 1);
    let path = f.service.data_root.join("config.json");
    let original = fs::read(&path).unwrap();
    let mut changed: Config = json_codec::from_slice(&original).unwrap();
    changed.max_workspaces += 1;
    fs::write(&path, json_codec::to_vec(&changed).unwrap()).unwrap();
    assert!(
        Engine::open_initial_activation_for_recovery_held(proof.probe, None, directories).is_err()
    );
    assert_eq!(target_directory_handles(&f.service.disks["one"]), 0);
    let guard =
        Engine::begin_initial_activation_probe(&f.service.data_root, &proof.intent.config).unwrap();
    drop(guard);
    fs::write(path, original).unwrap();
    f.service
        .complete_activation(&admin(), |_| Ok(()), |_| Ok(()))
        .unwrap();
}

fn damage_activation_locator(service: &SetupRecovery, mode: &str) {
    let locator = service.data_root.join("bootstrap-discovery");
    match mode {
        "intact" => (),
        "missing" => fs::rename(&locator, service.data_root.join("lost-locator")).unwrap(),
        "head-missing" => fs::remove_file(locator.join("head.json")).unwrap(),
        "history-missing" => {
            fs::remove_file(locator.join("record-00000000000000000001.json")).unwrap()
        }
        "head-corrupt" => fs::write(locator.join("head.json"), b"{incomplete").unwrap(),
        "dangling-record" => {
            fs::write(locator.join("record-00000000000000000003.json"), b"orphan").unwrap()
        }
        other => panic!("unknown locator damage: {other}"),
    }
}

/// Isolated libc observer checks the actual first selection stat, including an
/// explicit negative control against the physically removed target pathname.
#[test]
#[ignore = "requires activation-probe-io.sh and its child-only first-stat observer"]
fn activation_completion_never_probes_stopped_or_uncertain_initial_roots() {
    let base = PathBuf::from(std::env::var_os("ZXS_ACTIVATION_PROBE_BASE").unwrap());
    let trigger = PathBuf::from(std::env::var_os("ZXS_ACTIVATION_PROBE_TRIGGER").unwrap());
    let log = PathBuf::from(std::env::var_os("ZXS_ACTIVATION_PROBE_LOG").unwrap());
    let action = std::env::var("ZXS_ACTIVATION_PROBE_ACTION").unwrap();
    let receipt = std::env::var("ZXS_ACTIVATION_PROBE_RECEIPT").unwrap();
    let locator = std::env::var("ZXS_ACTIVATION_PROBE_LOCATOR").unwrap();
    let data_only = std::env::var("ZXS_ACTIVATION_PROBE_DATA_ONLY").unwrap() == "1";
    let service = service_at(&base, &["one", "two", "three"]);
    let mut engine = activated_engine(&service, data_only);
    let original = fs::read(service.data_root.join("config.json")).unwrap();
    match action.as_str() {
        "initial" => (),
        "add" => add_activation_successor(&mut engine, &base),
        "offline" => exit_activation_target(
            &mut engine,
            storage_engine::MembershipExitAction::PlannedOffline,
        ),
        "deregister" => exit_activation_target(
            &mut engine,
            storage_engine::MembershipExitAction::Deregister,
        ),
        other => panic!("unknown lifecycle action: {other}"),
    }
    if data_only && action != "add" {
        let initial: Config = json_codec::from_slice(&original).unwrap();
        assert_eq!(
            value_digest(engine.config()).unwrap(),
            value_digest(&initial).unwrap(),
        );
    }
    drop(engine);
    // Both stale and byte-equivalent Config projections must stay restricted.
    fs::write(service.data_root.join("config.json"), original).unwrap();
    let permits_completion = action == "initial" && locator == "intact";
    if !permits_completion {
        fs::rename(&service.disks["one"], base.join("physically-detached")).unwrap();
    }
    lose_activation_completion(&service, &receipt);
    damage_activation_locator(&service, &locator);
    fs::write(&log, b"").unwrap();
    fs::write(&trigger, b"observe first selection stat").unwrap();
    let status = service.storage_status();
    assert_eq!(status.completion_available, permits_completion);
    let completed = service.complete_activation(
        &admin(),
        |_| Ok(()),
        |_| {
            assert!(
                permits_completion,
                "lifecycle fallback reached installation"
            );
            Ok(())
        },
    );
    if permits_completion {
        completed.unwrap();
        assert!(service.activation_admitted().is_ok());
        assert!(
            !fs::read(&log).unwrap().is_empty(),
            "positive probe was not observed",
        );
    } else {
        assert!(completed.is_err());
        assert!(
            fs::read(&log).unwrap().is_empty(),
            "attempted a forbidden initial target stat",
        );
    }
    // Calibrate the exact boundary used by activation_seals even for ENOENT.
    fs::write(&log, b"").unwrap();
    assert_eq!(
        checked_directory(&service.disks["one"]).is_ok(),
        permits_completion,
    );
    assert!(
        !fs::read(&log).unwrap().is_empty(),
        "first-stat observer was inactive",
    );
    fs::remove_file(&trigger).unwrap();
    println!(
        "ZXS_ACTIVATION_PROBE_COMPLETE action={action} receipt={receipt} locator={locator} data_only={data_only}"
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
fn discovery_uses_only_immutable_host_configuration_without_data_io() {
    use storage_contract::storage_directories::DEFAULT_DISCOVERY_ROOT;
    let f = fixture();
    let fresh = SetupRecovery::new(
        f.service.data_root.clone(),
        f.service.management_root.clone(),
        BTreeMap::new(),
    )
    .unwrap();
    assert_eq!(
        fresh.discovery_root().discovery_root,
        DEFAULT_DISCOVERY_ROOT
    );
    assert_eq!(fs::read_dir(&f.service.data_root).unwrap().count(), 0);
    let unavailable = f._temp.path().join("unavailable-discovery");
    let fresh = fresh.with_discovery_root(unavailable.to_str().unwrap().parse().unwrap());
    assert_eq!(
        fresh.discovery_root().discovery_root,
        unavailable.to_str().unwrap()
    );
    assert!(!unavailable.exists());
    assert_eq!(fs::read_dir(&f.service.data_root).unwrap().count(), 0);
    initialize(&fresh);
    fs::write(
        fresh.data_root.join("storage-discovery.json"),
        b"preserve-obsolete-settings",
    )
    .unwrap();
    assert_eq!(
        fresh.discovery_root().discovery_root,
        unavailable.to_str().unwrap()
    );
    assert_eq!(
        fs::read(fresh.data_root.join("storage-discovery.json")).unwrap(),
        b"preserve-obsolete-settings"
    );
}

#[test]
fn changed_host_discovery_rejects_old_preview_before_intent_or_disk_writes() {
    let mut f = fixture();
    initialize(&f.service);
    let selected = request(&f.service, &["one"]);
    let other = f._temp.path().join("other");
    fs::create_dir(&other).unwrap();
    f.service = f
        .service
        .with_discovery_root(other.to_str().unwrap().parse().unwrap());
    assert!(
        f.service
            .activate(&admin(), selected, |_| Ok(()), |_| panic!())
            .is_err()
    );
    assert!(!f.service.data_root.join(ACTIVATION_INTENT).exists());
    assert_eq!(fs::read_dir(&f.service.disks["one"]).unwrap().count(), 0);
}

#[test]
fn pending_activation_completion_keeps_original_roots_after_host_config_change() {
    let mut f = fixture();
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
    let other = f._temp.path().join("other");
    f.service = f
        .service
        .with_discovery_root(other.to_str().unwrap().parse().unwrap());
    f.service
        .complete_activation(&admin(), |_| Ok(()), |_| Ok(()))
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
    assert!(!other.exists());
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

#[test]
fn exact_offline_activation_rejects_another_original_selection_or_mode() {
    let f = fixture();
    initialize(&f.service);
    let original = request(&f.service, &["one"]);
    let selected = original.clone();
    f.service
        .activate(&admin(), original, |_| Ok(()), |_| Ok(()))
        .unwrap();
    assert!(f.service.initial_activation_matches(&selected).unwrap());
    let mut other = selected.clone();
    other.mode = Mode::Production;
    assert!(f.service.initial_activation_matches(&other).is_err());
    assert!(
        f.service
            .complete_activation_exact(
                &admin(),
                &other,
                |_| Ok(()),
                |_| panic!("must not install a mismatched operation")
            )
            .is_err()
    );
    let mut other = selected;
    other.selections[0].root_identity.inode += 1;
    assert!(f.service.initial_activation_matches(&other).is_err());
}

#[test]
fn exact_completed_activation_still_runs_the_offline_materialization_callback() {
    let f = fixture();
    initialize(&f.service);
    let original = request(&f.service, &["one"]);
    f.service
        .activate(&admin(), original.clone(), |_| Ok(()), |_| Ok(()))
        .unwrap();
    let mut called = false;
    f.service
        .complete_activation_exact(
            &admin(),
            &original,
            |_| Ok(()),
            |engine| {
                called = true;
                assert_eq!(engine.config().disks.len(), 1);
                Ok(())
            },
        )
        .unwrap();
    assert!(called);
    assert!(f.service.activation_admitted().is_ok());
}
