//! Exact systemd state classes; no external systemd process or live unit.
use super::*;

const STOPPED: &str = "LoadState=loaded\nActiveState=inactive\nSubState=dead\nMainPID=0\nControlPID=0\nControlGroup=\nJob=\nNeedDaemonReload=no\nTransient=no\nFragmentPath=/etc/systemd/system/zxs.service\nDropInPaths=\nTriggeredBy=\n";

#[test]
fn no_process_job_reload_override_or_activation_source_is_required() {
    assert!(admit(ZXS, Some(0), STOPPED.as_bytes()).is_ok());
    for (before, after) in [
        ("ActiveState=inactive", "ActiveState=activating"),
        ("SubState=dead", "SubState=running"), ("MainPID=0", "MainPID=22"),
        ("ControlPID=0", "ControlPID=9"), ("ControlGroup=\n", "ControlGroup=/lingering\n"),
        ("Job=\n", "Job=7\n"), ("NeedDaemonReload=no", "NeedDaemonReload=yes"),
        ("Transient=no", "Transient=yes"),
        ("DropInPaths=\n", "DropInPaths=/private-name\n"),
        ("TriggeredBy=\n", "TriggeredBy=unexpected.socket\n"),
        ("/etc/systemd/system/zxs.service", "/some/other/zxs.service"),
        ("Job=\n", ""), ("Job=\n", "Job=\nJob=\n"), ("Job=\n", "Secret=not for output\n"),
    ] {
        let error = admit(ZXS, Some(0), STOPPED.replace(before, after).as_bytes()).err().unwrap();
        let diagnostic = error.to_string();
        assert!(!diagnostic.contains(after) || after.is_empty());
        assert!(!diagnostic.contains("private-name") && !diagnostic.contains("not for output"));
    }
    assert!(admit(ZXS, Some(1), STOPPED.as_bytes()).is_err());
    assert!(admit(ZXS, None, STOPPED.as_bytes()).is_err());
}

#[test]
fn absence_is_never_used_for_the_existing_required_zxs_unit() {
    let absent = STOPPED.replace("LoadState=loaded", "LoadState=not-found")
        .replace("/etc/systemd/system/zxs.service", "");
    assert!(admit(ZXS, Some(4), absent.as_bytes()).is_err());
    assert!(admit("smbd.service", Some(4), absent.as_bytes()).is_ok());
    assert!(admit("smbd.service", Some(2), absent.as_bytes()).is_err());
    assert!(admit("smbd.service", Some(4), absent.replace("MainPID=0","MainPID=2").as_bytes()).is_err());
}
