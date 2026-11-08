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
