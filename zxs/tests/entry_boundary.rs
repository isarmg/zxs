//! Negative executable boundary probe. No deployment or listener setup.
#[test]
fn legacy_argv0_cannot_select_an_old_role() {
    use std::{os::unix::process::CommandExt, process::Command};
    for name in [
        "zxs",
        "zxs:internal-smb-preflight-311",
        "zxs:internal-smb-311",
        "zxs:internal-web-1",
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_zxs"))
            .env_clear()
            .arg0(name)
            .arg("unsupported")
            .output()
            .unwrap();
        assert!(!out.status.success() && out.stdout.is_empty());
        assert!(
            String::from_utf8_lossy(&out.stderr)
                .contains("usage: zxs --listen IP:PORT (numeric IP, explicit nonzero port)")
        );
    }
}

// These operands fail before the storage or listener can be opened.
#[test]
fn listen_must_be_one_explicit_numeric_address_and_nonzero_port() {
    use std::process::Command;
    for args in [
        &[][..],
        &["--listen"],
        &["--listen", "localhost:8080"],
        &["--listen", "127.0.0.1"],
        &["--listen", "127.0.0.1:0"],
        &["--listen", "[::1]:0"],
        &["--listen", "127.0.0.1:8080", "extra"],
    ] {
        let out = Command::new(env!("CARGO_BIN_EXE_zxs"))
            .env_clear()
            .args(args)
            .output()
            .unwrap();
        assert!(!out.status.success() && out.stdout.is_empty());
        assert!(String::from_utf8_lossy(&out.stderr).contains("usage: zxs --listen IP:PORT"));
    }
}
