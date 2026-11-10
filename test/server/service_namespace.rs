//! Shared process entry for isolated daemon and helper integration cases.
use std::process::Command;

pub(crate) fn enter(script: &str, name: &str) -> bool {
    if std::env::var("ZXS_SERVICE_STARTUP_MOUNT_NAMESPACE").as_deref() == Ok(name) {
        return true;
    }
    let status = Command::new("/usr/bin/python3")
        .arg(script)
        .arg(std::env::current_exe().unwrap())
        .arg(name)
        .status()
        .unwrap();
    assert!(status.success(), "isolated service test failed: {status}");
    false
}
