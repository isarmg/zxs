use super::*;

#[test]
fn log_filter_errors_retain_typed_sources_without_displaying_input() {
    use std::{error::Error, ffi::OsString, os::unix::ffi::OsStringExt};
    let parse_error =
        EnvFilter::try_new("zxs[request{token=redaction-test-only(}]=info").unwrap_err();
    let error = LogFilterError::InvalidDirective(parse_error);
    assert_eq!(
        error.to_string(),
        "RUST_LOG contains an invalid filter directive"
    );
    let source = error
        .source()
        .unwrap()
        .downcast_ref::<tracing::collector::filter::ParseError>()
        .unwrap();
    assert!(
        source.source().is_some(),
        "regex parser cause must be retained"
    );

    let input = OsString::from_vec(b"redaction-test-only\xff".to_vec());
    let error = LogFilterError::NotUnicode(std::env::VarError::NotUnicode(input.clone()));
    assert_eq!(error.to_string(), "RUST_LOG must contain valid Unicode");
    assert_eq!(
        error.source().unwrap().downcast_ref::<std::env::VarError>(),
        Some(&std::env::VarError::NotUnicode(input))
    );
}

#[test]
fn setup_maps_validate_syntax_without_accessing_missing_files() {
    assert!(
        parse_setup_map(
            vec!["preset=/definitely/missing/config.json".into()],
            "--setup-config"
        )
        .is_ok()
    );
    for values in [
        vec!["preset=relative".into()],
        vec!["=/absolute".into()],
        vec!["same=/a".into(), "same=/b".into()],
    ] {
        assert!(parse_setup_map(values, "--setup-config").is_err());
    }
}

#[test]
fn setup_maps_share_the_runtime_identifier_policy_for_both_flags() {
    for flag in ["--setup-disk", "--setup-config"] {
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
            let paths =
                parse_setup_map(vec![format!("{id}=/definitely/missing/path")], flag).unwrap();
            assert!(paths.contains_key(id));
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
            let error = parse_setup_map(vec![format!("{id}=/absolute/path")], flag)
                .unwrap_err()
                .to_string();
            assert_eq!(
                error,
                format!("{flag} ID must contain 1–64 ASCII letters, digits, '_' or '-'")
            );
        }
    }
}

#[test]
fn trusted_proxy_requires_origin_and_exact_ip() {
    assert!(Args::try_parse_from(["zxs", "--trusted-proxy-ip", "127.0.0.1"]).is_err());
    assert!(
        Args::try_parse_from([
            "zxs",
            "--external-origin",
            "https://storage.example.org",
            "--trusted-proxy-ip",
            "127.0.0.0/8"
        ])
        .is_err()
    );
    let args = Args::try_parse_from([
        "zxs",
        "--external-origin",
        "https://storage.example.org",
        "--trusted-proxy-ip",
        "127.0.0.1",
        "--trusted-proxy-ip",
        "::1",
    ])
    .unwrap();
    assert_eq!(args.trusted_proxy_ips.len(), 2);
}

const WORKSPACE_LIMIT_FLAGS: [&str; 4] = [
    "--max-retained-workspaces",
    "--max-retained-workspaces-per-principal",
    "--max-retained-workspace-metadata-bytes",
    "--max-retained-workspace-metadata-bytes-per-principal",
];

#[test]
fn http_transfer_slots_use_one_limit_and_reject_old_flags() {
    assert_eq!(
        Args::try_parse_from(["zxs"]).unwrap().http_transfer_slots,
        2
    );
    assert_eq!(
        Args::try_parse_from(["zxs", "--http-transfer-slots", "3"])
            .unwrap()
            .http_transfer_slots,
        3
    );
    for old in [
        "--http-transfer-slots-total",
        "--http-transfer-slots-per-account",
    ] {
        assert!(Args::try_parse_from(["zxs", old, "2"]).is_err(), "{old}");
    }
}

#[test]
fn retained_workspace_flags_require_all_positive_values_or_none() {
    let base = vec!["zxs"];
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
fn smb_requires_one_exact_authenticated_account_without_mapping_syntax() {
    let args = [
        "zxs",
        "--smb-socket",
        "/run/zxs/smb.sock",
        "--smb-peer-uid",
        "0",
        "--smb-user",
        "samba-owner",
    ];
    assert_eq!(
        Args::try_parse_from(args).unwrap().smb_user.as_deref(),
        Some("samba-owner")
    );
    assert!(Args::try_parse_from(["zxs", "--smb-socket", "/run/zxs/smb.sock"]).is_err());
    assert!(Args::try_parse_from(["zxs", "--smb-user", "samba-owner"]).is_err());
    let mut repeated = args.to_vec();
    repeated.extend(["--smb-user", "other"]);
    assert!(Args::try_parse_from(repeated).is_err());
    for invalid in ["", "samba=owner", " guest", "guest ", "line\nbreak"] {
        let mut args = args;
        args[6] = invalid;
        assert!(Args::try_parse_from(args).is_err());
    }
}

#[test]
fn scrub_host_policy_flags_are_positive_and_daemon_defaults_are_periodic() {
    let args = Args::try_parse_from(["zxs"]).unwrap();
    assert_eq!(args.scrub_interval_seconds.get(), 86_400);
    assert_eq!(args.scrub_max_age_seconds.get(), 172_800);
    assert_eq!(args.scrub_bytes_per_second.get(), 67_108_864);
    for flag in [
        "--scrub-interval-seconds",
        "--scrub-max-age-seconds",
        "--scrub-bytes-per-second",
    ] {
        assert!(Args::try_parse_from(["zxs", flag, "0"]).is_err());
    }
}

#[test]
fn payload_proof_budget_is_optional_and_parsed_as_nonzero() {
    assert!(
        Args::try_parse_from(["zxs"])
            .unwrap()
            .max_held_payload_files
            .is_none()
    );
    for value in ["1", "3", "128", "512"] {
        assert_eq!(
            Args::try_parse_from(["zxs", "--max-held-payload-files", value])
                .unwrap()
                .max_held_payload_files
                .unwrap()
                .get(),
            value.parse::<usize>().unwrap()
        );
    }
    for value in ["0", "-1", "invalid", "18446744073709551616"] {
        assert!(Args::try_parse_from(["zxs", "--max-held-payload-files", value]).is_err());
    }
}
