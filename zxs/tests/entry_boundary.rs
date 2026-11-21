//! Only negative public argv probes against the built binary. No default
//! runtime start, device metadata, listener, pool, or user-machine access.
#[test]
fn historical_commands_are_not_a_delivered_entry(){
    use std::{os::unix::process::CommandExt,process::Command};
    for args in [&["new-id"][..],&["init","unused","unused-system","unused-recycle","unused-file"],
        &["file-commit-receipt","unused-system","unused-operation"],&["dir-list","unused-system"]]
    {
        let out=Command::new(env!("CARGO_BIN_EXE_zxs")).env_clear().args(args).output().unwrap();
        assert!(!out.status.success()&&out.stdout.is_empty());
        assert!(String::from_utf8_lossy(&out.stderr).contains("ZXS starts without arguments"));
    }
    // A recognized inherited-role spelling with a suffix must reject at
    // its private entry rather than fall through to any public mode.
    for role in ["zxs:internal-smb-preflight-311","zxs:internal-smb-311","zxs:internal-web-1"]{
        let out=Command::new(env!("CARGO_BIN_EXE_zxs")).env_clear().arg0(role).arg("unsupported").output().unwrap();
        assert!(!out.status.success()&&out.stdout.is_empty());
        let err=String::from_utf8_lossy(&out.stderr);
        assert!(err.starts_with("zxs: ")&&!err.contains("ZXS starts without arguments"));
    }
}
