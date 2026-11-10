use super::*;
#[test]
fn target_is_debian_13_only_without_duplicate_identity() {
    assert!(debian_13("ID=debian\nVERSION_ID=\"13\"\n"));
    for text in [
        "ID=ubuntu\nVERSION_ID=13\n",
        "ID=debian\nVERSION_ID=12\n",
        "ID=debian\nID=debian\nVERSION_ID=13\n",
    ] {
        assert!(!debian_13(text));
    }
}
#[test]
fn package_records_distinguish_missing_exact_partial_and_foreign() {
    let version = version().unwrap();
    assert_eq!(package_rows("", &version).unwrap(), [false, false, false]);
    let row = |name: &str| format!("{name}\tinstall ok installed\t{version}\tamd64\n");
    assert_eq!(
        package_rows(&row("samba-common-bin"), &version).unwrap(),
        [false, true, false]
    );
    let exact = row("samba") + &row("samba-common-bin") + &row("systemd");
    assert_eq!(package_rows(&exact, &version).unwrap(), [true, true, true]);
    for text in [
        row("other"),
        row("samba") + &row("samba"),
        exact.replace("installed", "unpacked"),
        exact.replace("amd64", "arm64"),
        exact.replace(&version, "old-version"),
    ] {
        assert!(package_rows(&text, &version).is_err());
    }
}
#[test]
fn native_install_uses_same_fixed_plan_and_preserves_configuration() {
    let version = version().unwrap();
    let execution = apt_args(&version, false);
    let simulation = apt_args(&version, true)
        .into_iter()
        .filter(|arg| arg != "--simulate")
        .collect::<Vec<_>>();
    assert_eq!(simulation, execution);
    assert!(execution.ends_with(&[
        "install".to_owned(),
        format!("samba={version}"),
        format!("samba-common-bin={version}"),
        "systemd".to_owned(),
    ]));
    for argument in [
        "--no-remove",
        "--no-upgrade",
        "--no-install-recommends",
        "APT::Get::AllowUnauthenticated=false",
        "Acquire::AllowInsecureRepositories=false",
        "DPkg::Options::=--force-confold",
    ] {
        assert!(execution.iter().any(|actual| actual == argument));
    }
    assert!(!execution.iter().any(|arg| matches!(
        arg.as_str(),
        "upgrade" | "dist-upgrade" | "--allow-unauthenticated" | "--allow-downgrades"
    )));
}
#[test]
fn confirmations_reject_before_host_inspection() {
    let request = || Commit {
        request_digest: "a".repeat(64),
        confirmation: "wrong".into(),
        external_administration_stopped: true,
    };
    assert!(install(request()).is_err());
    assert!(finish(request()).is_err());
    assert!(
        refresh(Refresh {
            confirmation: "wrong".into(),
            external_administration_stopped: true
        })
        .is_err()
    );
}
#[test]
fn temporary_policy_is_specific_to_original_and_denies_service_start() {
    let first = policy::contents(&"a".repeat(64));
    let second = policy::contents(&"b".repeat(64));
    assert_ne!(first, second);
    assert!(first.starts_with(b"#!/bin/sh\n# zxs-helper system packages "));
    assert!(first.ends_with(b"\nexit 101\n"));
}
