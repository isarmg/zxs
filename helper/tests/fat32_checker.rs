//! Future acceptance: requires the dedicated non-root worker, an administrator-verified
//! fixed checker profile, permitted user namespaces, and `HELPER_TEST_DATA_PARENT`.
//! No pre-existing image, mounted filesystem or device is an operand.
mod common;
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{Read, Seek, Write},
    os::unix::fs::{FileExt, MetadataExt},
    process::Command,
};

const CLUSTERS: u32 = 65526;
const FAT: u32 = 520;
fn sector(b: &mut [u8; 512], at: usize, n: u16) {
    b[at..at + 2].copy_from_slice(&n.to_le_bytes());
}
fn word(b: &mut [u8; 512], at: usize, n: u32) {
    b[at..at + 4].copy_from_slice(&n.to_le_bytes());
}
fn image(path: &std::path::Path) -> String {
    let total = 32 + 2 * FAT + CLUSTERS;
    let mut out = File::options()
        .create_new(true)
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    out.set_len(u64::from(total) * 512).unwrap();
    let mut b = [0u8; 512];
    b[..3].copy_from_slice(&[0xeb, 0x58, 0x90]);
    b[3..11].copy_from_slice(b"MSWIN4.1");
    sector(&mut b, 11, 512);
    b[13] = 1;
    sector(&mut b, 14, 32);
    b[16] = 2;
    b[21] = 0xf8;
    sector(&mut b, 24, 63);
    sector(&mut b, 26, 255);
    word(&mut b, 32, total);
    word(&mut b, 36, FAT);
    word(&mut b, 44, 2);
    sector(&mut b, 48, 1);
    sector(&mut b, 50, 6);
    b[64] = 0x80;
    b[66] = 0x29;
    word(&mut b, 67, 0x1357_2468);
    b[71..82].copy_from_slice(b"NO NAME    ");
    b[82..90].copy_from_slice(b"FAT32   ");
    b[510..].copy_from_slice(&[0x55, 0xaa]);
    for n in [0, 6] {
        out.write_all_at(&b, n * 512).unwrap();
    }
    let mut info = [0; 512];
    word(&mut info, 0, 0x4161_5252);
    word(&mut info, 484, 0x6141_7272);
    word(&mut info, 488, CLUSTERS - 1);
    word(&mut info, 492, 3);
    word(&mut info, 508, 0xaa55_0000);
    for n in [1, 7] {
        out.write_all_at(&info, n * 512).unwrap();
    }
    let mut fat = [0; 512];
    word(&mut fat, 0, 0x0fff_fff8);
    word(&mut fat, 4, 0x0fff_ffff);
    word(&mut fat, 8, 0x0fff_ffff);
    for n in [32, 32 + FAT] {
        out.write_all_at(&fat, u64::from(n) * 512).unwrap();
    }
    out.flush().unwrap();
    out.sync_all().unwrap();
    out.rewind().unwrap();
    let mut hash = Sha256::new();
    let mut buffer = vec![0; 32_768];
    loop {
        let n = out.read(&mut buffer).unwrap();
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    format!("{:x}", hash.finalize())
}
#[test]
fn actual_fixed_checker_on_its_own_fat32_copy_never_changes_the_source() {
    let temp = common::data_directory();
    let path = temp.path().join("unique.img");
    let hash = image(&path);
    let stamp = || {
        let m = path.metadata().unwrap();
        (
            m.dev(),
            m.ino(),
            m.size(),
            m.mtime(),
            m.mtime_nsec(),
            m.ctime(),
            m.ctime_nsec(),
        )
    };
    let before = stamp();
    let check = |h: &str| {
        Command::new(env!("CARGO_BIN_EXE_helper"))
            .env_clear()
            .arg("check-fat32-copy")
            .arg(&path)
            .args(["--sha256", h])
            .output()
            .unwrap()
    };
    let result = check(&hash);
    assert_eq!(
        result.status.code(),
        Some(0),
        "controlled acceptance prerequisite or checker failed: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        String::from_utf8(result.stdout)
            .unwrap()
            .contains("result=no-findings-reported")
    );
    assert_eq!(stamp(), before);
    let mut bad = hash.into_bytes();
    bad[0] = if bad[0] == b'f' { b'0' } else { b'f' };
    assert_eq!(
        check(std::str::from_utf8(&bad).unwrap()).status.code(),
        Some(2)
    );
    assert_eq!(stamp(), before);
}
