#![cfg(target_os = "linux")]

mod support;

use std::{
    fs::{self, File, FileTimes, OpenOptions},
    io::{BufRead, BufReader, Read, Write},
    os::unix::fs::PermissionsExt,
    path::Path,
    process::{Command, Output, Stdio},
    time::{Duration, UNIX_EPOCH},
};
use zxs_core::{session::Principal, workspace::Workspace};

fn cli(root: &Path, state: &Path) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_zxs-client"));
    cmd.current_dir(root).env_clear().arg(state);
    cmd
}

fn synthetic_tree(root: &Path, reverse: bool) {
    fs::create_dir(root).unwrap();
    let mut dirs = ["nested", "empty", ".git", "target"];
    let mut files = [
        (".zxsignore", &b"custom.tmp\n"[..]),
        ("a.txt", &b""[..]),
        ("nested/中.txt", &b"exact synthetic\x00binary\n"[..]),
        ("z.txt", &b"deflate deflate deflate\n"[..]),
        (".git/config", &b"must not ship"[..]),
        ("target/excluded", &b"must not ship"[..]),
        ("workspace.sqlite", &b"not actual state"[..]),
    ];
    if reverse {
        dirs.reverse();
        files.reverse();
    }
    for leaf in dirs {
        fs::create_dir(root.join(leaf)).unwrap();
    }
    for (leaf, bytes) in files {
        let file = File::create_new(root.join(leaf)).unwrap();
        (&file).write_all(bytes).unwrap();
        let mode = if reverse { 0o600 } else { 0o644 };
        file.set_permissions(fs::Permissions::from_mode(mode))
            .unwrap();
        let age = if reverse { 100_000 } else { 900_000 };
        file.set_times(FileTimes::new().set_modified(UNIX_EPOCH + Duration::from_secs(age)))
            .unwrap();
    }
    fs::write(
        root.join("custom.tmp"),
        if reverse { b"ignored B" } else { b"ignored A" },
    )
    .unwrap();
}

fn pack(
    root: &Path,
    state: &Path,
    tree: &Path,
    zip: &Path,
    after_preview: impl FnOnce(),
) -> Output {
    let mut child = cli(root, state)
        .arg("bundle-pack")
        .arg(tree)
        .arg(zip)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut out = String::new();
    loop {
        let mut line = String::new();
        if stdout.read_line(&mut line).unwrap() == 0 {
            drop(stdin);
            panic!("did not reach ZIP review: {:?}", child.wait_with_output());
        }
        out.push_str(&line);
        // Destination::new has finished; CLI has not accepted confirmation yet.
        if line.starts_with("仅生成普通本机 ZIP：") {
            break;
        }
    }
    after_preview();
    stdin.write_all(b"pack\n").unwrap();
    drop(stdin);
    stdout.read_to_string(&mut out).unwrap();
    let mut result = child.wait_with_output().unwrap();
    result.stdout = out.into_bytes();
    result
}

#[test]
#[ignore = "requires newly allocated unprivileged ext4 scratch and independent Python/unzip"]
fn deterministic_native_zip() {
    let root = support::fixture("zip");
    let state = root.join("private");
    fs::create_dir(&state).unwrap();
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    let principal: Principal = serde_json::from_str(
        r#"{"origin":"https://no-contact.invalid:443","account":"fixture","trust":"synthetic"}"#,
    )
    .unwrap();
    // Seed only local state. No transport or existing data is used.
    drop(Workspace::initialize(&state, &principal).unwrap());
    let a = root.join("a");
    let b = root.join("b");
    synthetic_tree(&a, false);
    synthetic_tree(&b, true);
    let zip_a = root.join("a.zip");
    let zip_b = root.join("b.zip");
    for (tree, file) in [(&a, &zip_a), (&b, &zip_b)] {
        let built = pack(&root, &state, tree, file, || ());
        assert!(built.status.success(), "{built:?}");
        let check = Command::new("unzip")
            .arg("-tqq")
            .arg(file)
            .status()
            .unwrap();
        assert!(check.success(), "independent unzip failed");
    }
    let oracle = Command::new("python3")
        .args(["-I", "-B", "-c", include_str!("zip_oracle.py")])
        .arg(&zip_a)
        .arg(&zip_b)
        .status()
        .unwrap();
    assert!(
        oracle.success(),
        "independent ZIP metadata/content checks failed"
    );

    let occupied = root.join("late.zip");
    let collision = pack(&root, &state, &a, &occupied, || {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&occupied)
            .unwrap();
        file.write_all(b"retain the exact late occupant").unwrap();
        file.sync_all().unwrap();
    });
    assert!(!collision.status.success());
    assert!(String::from_utf8_lossy(&collision.stderr).contains("local name already exists"));
    assert_eq!(
        fs::read(&occupied).unwrap(),
        b"retain the exact late occupant"
    );

    let stale = root.join("stale.zip");
    let result = pack(&root, &state, &a, &stale, || {
        fs::write(
            a.join("nested/中.txt"),
            b"changed after confirmation prompt",
        )
        .unwrap();
    });
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("previously reviewed bundle"));
    assert!(!stale.exists());

    fs::write(a.join("added.txt"), b"new").unwrap();
    fs::remove_file(a.join("z.txt")).unwrap();
    let diff = cli(&root, &state)
        .arg("bundle-diff")
        .arg(&a)
        .output()
        .unwrap();
    assert!(diff.status.success(), "{diff:?}");
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&diff.stdout).unwrap(),
        serde_json::json!({"added":["added.txt"],"changed":["nested/中.txt"],"removed":["z.txt"]})
    );
}
