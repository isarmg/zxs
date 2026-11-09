use super::*;

#[test]
fn authority_check_requires_an_explicit_selection_and_accepts_no_write_confirmation() {
    let args =
        Args::try_parse_from(["zxs-helper", "authority", "check", "/selection.json"]).unwrap();
    let Command::Authority {
        command: authority::Command::Check { selection },
    } = args.command.unwrap()
    else {
        panic!("wrong command")
    };
    assert_eq!(selection, PathBuf::from("/selection.json"));
    assert!(Args::try_parse_from(["zxs-helper", "authority", "check"]).is_err());
    assert!(
        Args::try_parse_from([
            "zxs-helper",
            "authority",
            "check",
            "/selection.json",
            "--confirm-write",
        ])
        .is_err()
    );
}

#[test]
fn workspace_inventory_is_an_explicit_read_only_command() {
    let args =
        Args::try_parse_from(["zxs-helper", "--root", "/pool", "workspace-inventory"]).unwrap();
    assert!(matches!(&args.command, Some(Command::WorkspaceInventory)));
    assert_eq!(args.root, PathBuf::from("/pool"));
    assert_eq!(args.workspace_limits().unwrap(), None);
}

#[test]
fn restore_system_accepts_one_original_member_but_requires_a_path() {
    let args =
        Args::try_parse_from(["zxs-helper", "restore-system", "--disks", "/disk/only"]).unwrap();
    let Command::RestoreSystem { disks } = args.command.unwrap() else {
        panic!("wrong command")
    };
    assert_eq!(disks, [PathBuf::from("/disk/only")]);
    assert!(Args::try_parse_from(["zxs-helper", "restore-system", "--disks"]).is_err());
}

#[test]
fn package_restore_defaults_to_one_copy_and_accepts_explicit_backup_policy() {
    for (extra, expected) in [
        (vec![], 1),
        (vec!["--copies", "1"], 1),
        (vec!["--copies", "4"], 4),
        (vec!["--copies", "4294967295"], u32::MAX),
    ] {
        let mut argv = vec![
            "zxs-helper",
            "package",
            "restore",
            "/package",
            "--primary-slot",
            "P01",
        ];
        argv.extend(extra);
        let args = Args::try_parse_from(argv).unwrap();
        let Command::Package {
            command: PackageCommand::Restore { copies, .. },
        } = args.command.unwrap()
        else {
            panic!("wrong command")
        };
        assert_eq!(copies, expected);
    }
}

#[test]
fn package_restore_rejects_zero_copies_as_an_argument_error() {
    let error = Args::try_parse_from([
        "zxs-helper",
        "package",
        "restore",
        "/package",
        "--primary-slot",
        "P01",
        "--copies",
        "0",
    ])
    .err()
    .expect("zero copies cannot restore even the primary copy");
    assert_eq!(
        error.kind(),
        command_line::error::ErrorKind::ValueValidation
    );
    assert!(error.to_string().contains("--copies"));
}

const WORKSPACE_LIMIT_FLAGS: [&str; 4] = [
    "--max-retained-workspaces",
    "--max-retained-workspaces-per-principal",
    "--max-retained-workspace-metadata-bytes",
    "--max-retained-workspace-metadata-bytes-per-principal",
];

#[test]
fn retained_workspace_flags_require_all_positive_values_or_none() {
    let base = vec!["zxs-helper", "package", "check", "/missing-package"];
    assert_eq!(
        Args::try_parse_from(base.clone())
            .unwrap()
            .workspace_limits()
            .unwrap(),
        None
    );
    for flag in WORKSPACE_LIMIT_FLAGS {
        let mut argv = base.clone();
        argv.extend([flag, "1"]);
        assert!(Args::try_parse_from(argv).is_err(), "{flag}");
    }
    for zero in 0..4 {
        let mut argv = base.clone();
        for (index, flag) in WORKSPACE_LIMIT_FLAGS.iter().enumerate() {
            argv.extend([*flag, if index == zero { "0" } else { "1" }]);
        }
        assert!(Args::try_parse_from(argv).is_err(), "zero dimension {zero}");
    }
    let mut argv = base;
    for (flag, value) in WORKSPACE_LIMIT_FLAGS.iter().zip(["7", "3", "11", "5"]) {
        argv.extend([*flag, value]);
    }
    let limits = Args::try_parse_from(argv)
        .unwrap()
        .workspace_limits()
        .unwrap()
        .unwrap();
    assert_eq!(
        limits,
        storage_engine::RetainedWorkspaceLimits {
            max_retained_workspaces: 7,
            max_retained_workspaces_per_principal: 3,
            max_retained_workspace_metadata_bytes: 11,
            max_retained_workspace_metadata_bytes_per_principal: 5,
        }
    );
}

#[test]
fn retained_workspace_flags_are_global_for_every_engine_command() {
    for command in [
        vec!["scrub"],
        vec!["package", "export", "history", "/package"],
        vec!["package", "restore", "/package", "--primary-slot", "P01"],
        vec!["restore-system", "--disks", "/disk"],
        vec![
            "recovery-preview",
            "op_00000000000000000000000000000000",
            "--reason",
            "review",
        ],
        vec![
            "recovery-commit",
            "recovery",
            "--digest",
            "digest",
            "--accept-permanent-loss",
        ],
    ] {
        for before_command in [true, false] {
            let mut argv = vec!["zxs-helper"];
            let limits = WORKSPACE_LIMIT_FLAGS
                .into_iter()
                .flat_map(|flag| [flag, "1"]);
            if before_command {
                argv.extend(limits);
                argv.extend(command.clone());
            } else {
                argv.extend(command.clone());
                argv.extend(limits);
            }
            assert!(
                Args::try_parse_from(argv)
                    .unwrap()
                    .workspace_limits()
                    .unwrap()
                    .is_some()
            );
        }
    }
}

#[test]
fn operation_identifiers_are_parsed_at_the_command_line_boundary() {
    for value in [
        "op_00000000000000000000000000000000",
        "op_ffffffffffffffffffffffffffffffff",
    ] {
        let args = Args::try_parse_from([
            "zxs-helper",
            "recovery-preview",
            value,
            "--reason",
            "review",
        ])
        .unwrap();
        let Command::RecoveryPreview { operation, .. } = args.command.unwrap() else {
            panic!("wrong command")
        };
        assert_eq!(operation.to_string(), value);
        let args = Args::try_parse_from([
            "zxs-helper",
            "package",
            "restore",
            "/package",
            "--primary-slot",
            "P01",
            "--operation-id",
            value,
        ])
        .unwrap();
        let Command::Package {
            command: PackageCommand::Restore { operation_id, .. },
        } = args.command.unwrap()
        else {
            panic!("wrong command")
        };
        assert_eq!(operation_id, Some(operation));
    }
    for value in [
        "",
        "operation",
        "op_0000000000000000000000000000000A",
        "op_000000000000000000000000000000000",
    ] {
        for argv in [
            vec![
                "zxs-helper",
                "recovery-preview",
                value,
                "--reason",
                "review",
            ],
            vec![
                "zxs-helper",
                "package",
                "restore",
                "/package",
                "--primary-slot",
                "P01",
                "--operation-id",
                value,
            ],
        ] {
            let error = Args::try_parse_from(argv)
                .err()
                .expect("invalid external operation ID");
            assert_eq!(
                error.kind(),
                command_line::error::ErrorKind::ValueValidation
            );
        }
    }
}

#[test]
fn diagnostic_chain_keeps_nested_sources_and_escapes_terminal_controls() {
    let error = storage_engine::Error::Filesystem(std::io::Error::from_raw_os_error(5).into())
        .context(storage_engine::ErrorCode::HistoryPackageInvalid, "inner")
        .context(
            storage_engine::ErrorCode::HistoryPackageInvalid,
            "outer\n\x1b[31m",
        );
    let diagnostic = diagnostic_chain(&error);
    assert!(diagnostic.contains("outer\\n\\u{1b}[31m"), "{diagnostic}");
    assert!(
        diagnostic.contains("Input/output error (os error 5)"),
        "{diagnostic}"
    );
    assert!(!diagnostic.contains('\n'));
    assert!(!diagnostic.contains('\x1b'));
}

#[test]
fn no_command_selects_the_manual_console_and_serve_parses_an_explicit_listener() {
    assert!(
        Args::try_parse_from(["zxs-helper"])
            .unwrap()
            .command
            .is_none()
    );
    let args = Args::try_parse_from(["zxs-helper", "serve", "--listen", "0.0.0.0:9041"]).unwrap();
    assert!(matches!(args.command, Some(Command::Serve(_))));
    assert!(Args::try_parse_from(["zxs-helper", "serve", "--listen", "bad-listener"]).is_err());
}

#[test]
fn program_installation_is_explicit_and_does_not_accept_an_arbitrary_destination() {
    let args = Args::try_parse_from([
        "zxs-helper",
        "install",
        "commit",
        "--request-digest",
        "review",
        "--confirm-program-only",
    ])
    .unwrap();
    assert!(matches!(
        args.command,
        Some(Command::Install {
            command: installation::Command::Commit {
                confirm_program_only: true,
                ..
            }
        })
    ));
    assert!(
        Args::try_parse_from(["zxs-helper", "install", "commit", "--confirm-program-only"])
            .is_err()
    );
    assert!(
        Args::try_parse_from([
            "zxs-helper",
            "install",
            "preview",
            "--destination",
            "/arbitrary"
        ])
        .is_err()
    );
}

#[test]
fn deployment_requires_complete_smb_parameters_and_has_no_activation_flag() {
    let base = [
        "zxs-helper",
        "deployment",
        "preview",
        "--service-user",
        "zxs",
        "--service-group",
        "zxs",
    ];
    assert!(matches!(
        Args::try_parse_from(base).unwrap().command,
        Some(Command::Deployment { .. })
    ));
    let mut incomplete = base.to_vec();
    incomplete.extend(["--smb-account", "zxs-owner"]);
    assert!(Args::try_parse_from(incomplete).is_err());
    let mut activate = base.to_vec();
    activate.push("--activate");
    assert!(Args::try_parse_from(activate).is_err());
}

#[test]
fn engine_routes_require_lifetime_exclusion_but_pure_confirmation_errors_do_not() {
    for args in [
        vec!["zxs-helper", "scrub"],
        vec!["zxs-helper", "restore-system", "--disks", "/disk"],
        vec!["zxs-helper", "package", "export", "history", "/destination"],
    ] {
        assert!(requires_service_exclusion(
            &Args::try_parse_from(args).unwrap().command.unwrap()
        ));
    }
    let args = Args::try_parse_from([
        "zxs-helper",
        "recovery-commit",
        "receipt",
        "--digest",
        "digest",
    ])
    .unwrap();
    assert!(!requires_service_exclusion(&args.command.unwrap()));
    let host = Args::try_parse_from(["zxs-helper", "host", "preview"]).unwrap();
    assert!(!requires_service_exclusion(&host.command.unwrap()));
}

#[test]
fn private_layout_cli_is_named_as_preparation_not_system_initialization() {
    let args = Args::try_parse_from([
        "zxs-helper",
        "initialization",
        "layout-preview",
        "--service-user",
        "zxs",
    ])
    .unwrap();
    assert!(matches!(args.command, Some(Command::Initialization { .. })));
    assert!(
        Args::try_parse_from([
            "zxs-helper",
            "initialization",
            "layout-prepare",
            "--service-user",
            "zxs",
            "--confirm-private-layout-only"
        ])
        .is_err()
    );
}

#[test]
fn configuration_installation_has_explicit_files_only_boundary_no_service_start() {
    let base = [
        "zxs-helper",
        "deployment",
        "install-files",
        "--service-user",
        "zxs",
        "--service-group",
        "zxs",
        "--request-digest",
        "exact",
        "--confirm-configuration-files-only",
        "--external-administration-stopped",
    ];
    assert!(matches!(
        Args::try_parse_from(base).unwrap().command,
        Some(Command::Deployment {
            command: deployment::Command::InstallFiles { .. }
        })
    ));
    for forbidden in ["--enable", "--start", "--force", "--destination"] {
        let mut arguments = base.to_vec();
        arguments.push(forbidden);
        assert!(Args::try_parse_from(arguments).is_err());
    }
}

#[test]
fn runtime_first_install_has_separate_preview_confirmation_and_external_ack() {
    let args = Args::try_parse_from([
        "zxs-helper",
        "initialization",
        "runtime-prepare",
        "--service-user",
        "zxs",
        "--request-digest",
        "exact",
        "--confirm-empty-runtime-only",
        "--external-administration-stopped",
    ])
    .unwrap();
    assert!(matches!(
        args.command,
        Some(Command::Initialization {
            command: initialization::Command::RuntimePrepare {
                confirm_empty_runtime_only: true,
                external_administration_stopped: true,
                ..
            }
        })
    ));
    assert!(
        Args::try_parse_from([
            "zxs-helper",
            "initialization",
            "runtime-preview",
            "--service-user",
            "zxs",
            "--path",
            "/tmp/other",
        ])
        .is_err()
    );
}

#[test]
fn same_payload_repair_is_explicit_and_accepts_no_destination_or_runtime_package() {
    let args = Args::try_parse_from([
        "zxs-helper",
        "install",
        "repair-commit",
        "--request-digest",
        "original",
        "--confirm-same-payload-program-only",
        "--external-administration-stopped",
    ])
    .unwrap();
    assert!(matches!(
        args.command,
        Some(Command::Install {
            command: installation::Command::RepairCommit {
                confirm_same_payload_program_only: true,
                external_administration_stopped: true,
                ..
            }
        })
    ));
    assert!(
        Args::try_parse_from([
            "zxs-helper",
            "install",
            "repair-preview",
            "--destination",
            "/tmp/elsewhere",
        ])
        .is_err()
    );
}

#[test]
fn embedded_material_export_requires_an_explicit_new_output_and_has_no_extraction_mode() {
    let args = Args::try_parse_from([
        "zxs-helper",
        "install",
        "materials",
        "--output",
        "/tmp/explicit-new-archive.tar.gz",
    ])
    .unwrap();
    assert!(matches!(args.command, Some(Command::Install {
        command: installation::Command::Materials { output }
    }) if output == PathBuf::from("/tmp/explicit-new-archive.tar.gz")));
    assert!(Args::try_parse_from(["zxs-helper", "install", "materials"]).is_err());
    assert!(
        Args::try_parse_from([
            "zxs-helper",
            "install",
            "materials",
            "--extract",
            "/tmp/other"
        ])
        .is_err()
    );
}

#[test]
fn original_configuration_recovery_takes_only_the_original_digest_and_explicit_acknowledgments() {
    let args = Args::try_parse_from([
        "zxs-helper",
        "deployment",
        "install-recover-files",
        "--request-digest",
        "original",
        "--confirm-original-configuration-files-only",
        "--external-administration-stopped",
    ])
    .unwrap();
    assert!(matches!(
        args.command,
        Some(Command::Deployment {
            command: deployment::Command::InstallRecoverFiles {
                confirm_original_configuration_files_only: true,
                external_administration_stopped: true,
                ..
            }
        })
    ));
    for option in ["--service-user", "--destination", "--start-service"] {
        assert!(
            Args::try_parse_from([
                "zxs-helper",
                "deployment",
                "install-recover-preview",
                "--request-digest",
                "original",
                option,
                "other",
            ])
            .is_err()
        );
    }
    assert!(Args::try_parse_from(["zxs-helper", "deployment", "install-recover-preview"]).is_err());
}
