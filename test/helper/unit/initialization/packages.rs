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
    let target = Scope::Samba.target().unwrap();
    assert_eq!(
        package_rows("", &target).unwrap(),
        PackageState {
            complete: false,
            new_samba: true
        }
    );
    let row = |name: &str| format!("{name}\tinstall ok installed\t{version}\tamd64\n");
    assert_eq!(
        package_rows(&row("samba-common-bin"), &target).unwrap(),
        PackageState {
            complete: false,
            new_samba: true
        }
    );
    let exact = row("samba") + &row("samba-common-bin") + &row("systemd");
    assert_eq!(
        package_rows(&exact, &target).unwrap(),
        PackageState {
            complete: true,
            new_samba: false
        }
    );
    for text in [
        row("other"),
        row("samba") + &row("samba"),
        exact.replace("installed", "unpacked"),
        exact.replace("amd64", "arm64"),
        exact.replace(&version, "old-version"),
    ] {
        assert!(package_rows(&text, &target).is_err());
    }
}
#[test]
fn native_install_uses_same_fixed_plan_and_preserves_configuration() {
    let version = version().unwrap();
    let target = Scope::Samba.target().unwrap();
    let execution = apt_args(&target, false);
    let simulation = apt_args(&target, true)
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
        scope: Scope::Systemd,
        request_digest: "a".repeat(64),
        confirmation: "wrong".into(),
        external_administration_stopped: true,
    };
    assert!(install(request()).is_err());
    assert!(finish(request()).is_err());
    assert!(
        refresh(Refresh {
            scope: Scope::Systemd,
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

#[test]
fn systemd_only_installs_tools_without_a_samba_target() {
    let target = Scope::Systemd.target().unwrap();
    assert!(matches!(target, Target::Systemd));
    assert_eq!(Scope::Systemd.names(), ["systemd"]);
    assert_eq!(
        package_rows("", &target).unwrap(),
        PackageState {
            complete: false,
            new_samba: false
        }
    );
    let row = "systemd\tinstall ok installed\t257.8-1\tamd64\n";
    assert_eq!(
        package_rows(row, &target).unwrap(),
        PackageState {
            complete: true,
            new_samba: false
        }
    );
    assert!(package_rows(&row.replace("systemd", "samba"), &target).is_err());
    let args = apt_args(&target, false);
    assert!(args.ends_with(&["install".to_owned(), "systemd".to_owned()]));
    assert!(!args.iter().any(|argument| argument.starts_with("samba")));
    assert_ne!(Scope::Systemd.records(), Scope::Samba.records());
}

#[test]
fn dependency_scope_is_explicit_at_the_http_boundary() {
    let input: Selection = json_codec::from_str(r#"{"scope":"systemd"}"#).unwrap();
    assert_eq!(input.scope, Scope::Systemd);
    for text in [
        r#"{}"#,
        r#"{"scope":"other"}"#,
        r#"{"scope":"systemd","upgrade":true}"#,
        r#"{"scope":"systemd","scope":"samba"}"#,
    ] {
        assert!(json_codec::from_str::<Selection>(text).is_err());
    }
}
