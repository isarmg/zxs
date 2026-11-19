//! Future isolated Debian 13 acceptance. Use two **saved regular test-volume
//! copies**, with independent SHA-256s from the fixture custodian. Never use
//! devices or mounted host filesystems as images. Nothing is formatted here.
use std::{env,fs,path::PathBuf,process::Command,os::unix::fs::MetadataExt};
#[test]
fn fixed_exfat_checker_on_known_clean_and_uncorrected_copies(){
    for (kind,code,expected) in [("CLEAN",0,"no-findings-reported"),("ERRORS",1,"checker-reported-unresolved-errors")]{
        let image=PathBuf::from(env::var_os(format!("HELPER_TEST_EXFAT_{kind}_IMAGE")).expect("dedicated fixture required"));
        let hash=env::var(format!("HELPER_TEST_EXFAT_{kind}_SHA256")).expect("independent fixture SHA-256 required");
        assert!(image.is_absolute()&&hash.len()==64&&hash.bytes().all(|b|b.is_ascii_hexdigit()));
        let stamp=||{let m=fs::symlink_metadata(&image).unwrap();assert!(m.is_file()&&m.nlink()==1);
            (m.dev(),m.ino(),m.size(),m.mtime(),m.mtime_nsec(),m.ctime(),m.ctime_nsec())};
        let before=stamp();
        let check=|h:&str|Command::new(env!("CARGO_BIN_EXE_helper")).env_clear()
            .arg("check-exfat-copy").arg(&image).args(["--sha256",h]).output().unwrap();
        let r=check(&hash);assert_eq!(r.status.code(),Some(code),"qualified original and worker required");
        assert!(String::from_utf8(r.stdout).unwrap().contains(&format!("result={expected} ")));
        assert_eq!(stamp(),before);
        let mut wrong=hash.into_bytes();wrong[0]=if wrong[0]==b'0'{b'1'}else{b'0'};
        assert_eq!(check(std::str::from_utf8(&wrong).unwrap()).status.code(),Some(2));assert_eq!(before,stamp());
    }
}
