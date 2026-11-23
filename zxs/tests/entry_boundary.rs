//! Negative executable boundary probe. No deployment or listener setup.
#[test]
fn legacy_argv0_cannot_select_an_old_role() {
    use std::{os::unix::process::CommandExt, process::Command};
    for name in ["zxs", "zxs:internal-smb-preflight-311", "zxs:internal-smb-311", "zxs:internal-web-1"] {
        let out = Command::new(env!("CARGO_BIN_EXE_zxs")).env_clear().arg0(name).arg("unsupported").output().unwrap();
        assert!(!out.status.success() && out.stdout.is_empty());
        assert!(String::from_utf8_lossy(&out.stderr).contains("zxs takes no arguments"));
    }
}
