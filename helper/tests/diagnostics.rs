//! CLI acceptance source. All writable fixtures belong to a freshly created temporary directory.
#![cfg(all(
    target_os = "linux",
    target_arch = "x86_64",
    target_pointer_width = "64"
))]
mod common;
use std::{
    fs::{self, File, FileTimes},
    os::unix::fs::{MetadataExt, symlink},
    path::Path,
    process::{Command, Output},
    time::{Duration, UNIX_EPOCH},
};

fn run(cwd: &Path, args: &[&Path]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_helper"))
        .current_dir(cwd)
        .args(args)
        .output()
        .unwrap()
}
fn stamp(p: &Path) -> (u64, u64, u64, u32, u64, i64, i64, i64, i64, i64, i64) {
    let m = fs::symlink_metadata(p).unwrap();
    (
        m.dev(),
        m.ino(),
        m.len(),
        m.mode(),
        m.nlink(),
        m.atime(),
        m.atime_nsec(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
    )
}
fn refusal(result: Output) {
    assert_eq!(result.status.code(), Some(2));
    assert!(result.stdout.is_empty());
}

#[test]
fn usage_and_unknown_commands_do_not_bootstrap_anything() {
    let tmp = tempfile::tempdir().unwrap();
    refusal(run(tmp.path(), &[]));
    assert!(run(tmp.path(), &[Path::new("--help")]).status.success());
    assert!(run(tmp.path(), &[Path::new("--version")]).status.success());
    refusal(run(tmp.path(), &[Path::new("unknown")]));
    assert_eq!(fs::read_dir(tmp.path()).unwrap().count(), 0);
}

#[test]
fn directory_observations_report_identity_visible_ancestry_and_device_separately() {
    let tmp = tempfile::tempdir().unwrap();
    let (a, b) = (tmp.path().join("alpha"), tmp.path().join("bravo"));
    let child = a.join("inside");
    for dir in [&a, &b, &child] {
        fs::create_dir(dir).unwrap();
    }
    let alias = tmp.path().join("alias");
    symlink(&a, &alias).unwrap();
    let op = Path::new("inspect-dirs");
    let before = stamp(&a);
    let result = run(tmp.path(), &[op, &a, &b, &child, &a]);
    assert_eq!(result.status.code(), Some(0));
    let text = String::from_utf8(result.stdout).unwrap();
    assert!(text.starts_with("directories=4\n"));
    assert!(text.lines().any(|s| s == "pair[0,1] same_entity=false same_st_dev=true first_visible_ancestor=false second_visible_ancestor=false"));
    assert!(text.lines().any(|s| s == "pair[0,2] same_entity=false same_st_dev=true first_visible_ancestor=true second_visible_ancestor=false"));
    assert!(text.lines().any(|s| s == "pair[0,3] same_entity=true same_st_dev=true first_visible_ancestor=false second_visible_ancestor=false"));
    assert!(text.lines().any(|s| s == "pair[2,3] same_entity=false same_st_dev=true first_visible_ancestor=false second_visible_ancestor=true"));
    assert_eq!(before, stamp(&a));
    for path in [
        alias.clone(),
        alias.join("inside"),
        a.join("missing"),
        a.join(".."),
        "relative".into(),
    ] {
        refusal(run(tmp.path(), &[op, &a, &path]));
    }
    let mut maximum: Vec<&Path> = vec![op];
    maximum.extend(std::iter::repeat_n(a.as_path(), 16));
    let result = run(tmp.path(), &maximum);
    assert_eq!(result.status.code(), Some(0));
    assert!(result.stdout.len() <= 32 * 1024);
    assert_eq!(
        String::from_utf8(result.stdout)
            .unwrap()
            .lines()
            .filter(|line| line.starts_with("pair["))
            .count(),
        120
    );
    maximum.push(&a);
    refusal(run(tmp.path(), &maximum));
}

#[test]
fn ordinary_byte_comparison_is_bounded_and_does_not_touch_observed_files() {
    use rustix::fs::{CWD, Mode, mkfifoat};
    let tmp = common::data_directory();
    let (a, b) = (tmp.path().join("reference"), tmp.path().join("candidate"));
    let op = Path::new("compare-files");
    let mut bytes = vec![0x63; 65_537];
    for p in [&a, &b] {
        fs::write(p, &bytes).unwrap();
        File::open(p)
            .unwrap()
            .set_times(FileTimes::new().set_accessed(UNIX_EPOCH + Duration::from_secs(1_000)))
            .unwrap();
    }
    let original = (stamp(&a), stamp(&b));
    assert_eq!(run(tmp.path(), &[op, &a, &b]).status.code(), Some(0));
    assert_eq!(original, (stamp(&a), stamp(&b))); // includes nanosecond access times
    bytes[65_536] = 42;
    fs::write(&b, &bytes).unwrap();
    assert_eq!(run(tmp.path(), &[op, &a, &b]).status.code(), Some(1));
    fs::write(&b, &bytes[..12]).unwrap();
    assert_eq!(run(tmp.path(), &[op, &a, &b]).status.code(), Some(1));
    let alias = tmp.path().join("alias");
    symlink(&a, &alias).unwrap();
    refusal(run(tmp.path(), &[op, &alias, &b]));
    refusal(run(tmp.path(), &[op, &a, &a]));
    refusal(run(tmp.path(), &[op, tmp.path(), &b]));
    let pipe = tmp.path().join("fifo");
    mkfifoat(CWD, pipe.as_path(), Mode::RUSR | Mode::WUSR).unwrap();
    refusal(run(tmp.path(), &[op, &pipe, &b]));
    assert_eq!(original.0, stamp(&a));
    let hard = tmp.path().join("hard");
    fs::hard_link(&a, &hard).unwrap();
    refusal(run(tmp.path(), &[op, &hard, &b]));
    let large = tmp.path().join("large");
    File::create(&large)
        .unwrap()
        .set_len((256 << 20) + 1)
        .unwrap();
    refusal(run(tmp.path(), &[op, &large, &b]));
}
