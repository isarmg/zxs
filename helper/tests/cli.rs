//! CLI acceptance source. Its writable fixture is a freshly created temporary directory.
#![cfg(all(
    target_os = "linux",
    target_arch = "x86_64",
    target_pointer_width = "64"
))]

use std::{
    fs,
    path::Path,
    process::{Command, Output},
};

fn run(cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_helper"))
        .current_dir(cwd)
        .args(args)
        .output()
        .unwrap()
}

fn refusal(result: &Output) {
    assert_eq!(result.status.code(), Some(2));
    assert_eq!(result.stdout, [] as [u8; 0]);
}

#[test]
fn usage_and_unknown_commands_do_not_bootstrap_anything() {
    let tmp = tempfile::tempdir().unwrap();
    refusal(&run(tmp.path(), &[]));
    assert!(run(tmp.path(), &["--help"]).status.success());
    assert!(run(tmp.path(), &["--version"]).status.success());
    refusal(&run(tmp.path(), &["unknown"]));
    assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 0);
}
