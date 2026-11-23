//! Actual CLI source fixture, restricted to fresh, test-owned directories; does not mount or change any service.
#![cfg(all(
    target_os = "linux",
    target_arch = "x86_64",
    target_pointer_width = "64"
))]
use rustix::fs::{AtFlags, StatVfsMountFlags, StatxFlags, fstatvfs, statx};
use std::{
    fs::{self, File},
    os::unix::fs::{MetadataExt, symlink},
    path::Path,
    process::{Command, Output},
};

fn status(system: &Path, recycle: &Path, data: &Path) -> Output {
    Command::new(env!("CARGO_BIN_EXE_helper"))
        .arg("storage-status")
        .arg("--recycle")
        .arg(recycle)
        .arg("--data")
        .arg(data)
        .arg("--system")
        .arg(system)
        .output()
        .unwrap()
}
fn field<'a>(row: &'a str, key: &str) -> &'a str {
    row.split_ascii_whitespace()
        .filter_map(|part| part.split_once('='))
        .find(|(k, _)| *k == key)
        .unwrap_or_else(|| panic!("missing field {key} in {row}"))
        .1
}

#[test]
fn requested_roles_report_actual_mount_and_capacity_without_claiming_or_registering_a_volume() {
    let area = tempfile::tempdir().unwrap();
    let (s, r, d) = (
        area.path().join("s-secret"),
        area.path().join("r-secret"),
        area.path().join("d-secret"),
    );
    for p in [&s, &r, &d] {
        fs::create_dir(p).unwrap();
    }
    fs::write(d.join("original"), b"unchanged").unwrap();
    let dir = File::open(&d).unwrap();
    let expected=statx(&dir,c"",AtFlags::EMPTY_PATH,StatxFlags::MNT_ID)
        .expect("fixture requires kernel statx and mount IDs; otherwise it has not verified the positive case");
    assert_ne!(
        expected.stx_mask & StatxFlags::MNT_ID.bits(),
        0,
        "fixture requires an actual mount ID"
    );
    let v = fstatvfs(&dir).unwrap();
    let meta = dir.metadata().unwrap();
    let result = status(&s, &r, &d);
    assert!(result.status.success(), "{result:?}");
    let text = String::from_utf8(result.stdout).unwrap();
    let lines: Vec<_> = text.lines().collect();
    assert_eq!(lines.len(), 4);
    assert!(lines[0].contains("registration=not-checked"));
    assert!(lines[0].contains("business_health=not-observed"));
    assert!(lines[0].contains("sample_unix_ms="));
    for (line, role) in lines[1..].iter().zip(["system", "recycle", "data[1]"]) {
        assert_eq!(field(line, "role"), role);
    }
    let data = lines[3];
    assert_eq!(field(data, "ino").parse::<u64>().unwrap(), meta.ino());
    assert_eq!(
        field(data, "mount_id").parse::<u64>().unwrap(),
        expected.stx_mnt_id
    );
    assert_eq!(
        field(data, "mount_ro_flag"),
        if v.f_flag.contains(StatVfsMountFlags::RDONLY) {
            "set"
        } else {
            "clear"
        }
    );
    assert_eq!(
        field(data, "total_bytes").parse::<u128>().unwrap(),
        u128::from(v.f_blocks) * u128::from(v.f_frsize)
    );
    assert!(
        field(data, "available_bytes").parse::<u128>().unwrap()
            <= field(data, "free_bytes").parse::<u128>().unwrap()
    );
    assert!(!text.contains("-secret"));
    assert!(!text.contains("original"));
    assert_eq!(fs::read(d.join("original")).unwrap(), b"unchanged");
    assert_eq!(fs::read_dir(&s).unwrap().count(), 0);
    assert_eq!(fs::read_dir(&r).unwrap().count(), 0);
}

#[test]
fn one_missing_or_aliased_root_cannot_hide_other_observations_or_impersonate_the_requested_volume()
{
    let area = tempfile::tempdir().unwrap();
    for n in ["s", "r", "underlying"] {
        fs::create_dir(area.path().join(n)).unwrap();
    }
    let d = area.path().join("no-data");
    for alias in [false, true] {
        if alias {
            symlink(area.path().join("underlying"), &d).unwrap();
        }
        let out = status(&area.path().join("s"), &area.path().join("r"), &d);
        assert_eq!(out.status.code(), Some(2));
        let text = String::from_utf8(out.stdout).unwrap();
        assert!(text.contains("role=system path=observed"));
        assert!(text.contains("role=recycle path=observed"));
        let line = text
            .lines()
            .find(|s| s.starts_with("role=data[1]"))
            .unwrap();
        assert!(line.contains("path=unverified"));
        assert!(!line.contains("mount_id="));
        assert!(!line.contains("available_bytes="));
        if !alias {
            assert_eq!(field(line, "reason"), "not-found");
        }
        assert!(!area.path().join("registration").exists());
    }
}
