//! These cover the public plan and the distinction between availability and diagnosis.
use super::*;

#[test]
fn all_seven_plans_have_a_typed_target_fixed_argv_and_no_repair_control() {
    use Target::*;
    let expected:[(&str,&str,&[&str],Target);7]=[
        ("ext4","e2fsck",&["-f","-n"],OfflineImage), ("ntfs","ntfsfix",&["-n"],OfflineImage),
        ("zfs","zpool",&["status"],ImportedPool), ("xfs","xfs_repair",&["-n","-f"],OfflineImage),
        ("btrfs","btrfs",&["check","--readonly"],IsolatedMembers),
        ("exfat","fsck.exfat",&["-n"],OfflineImage), ("fat32","fsck.fat",&["-n"],OfflineImage),
    ];
    for (p,(format,tool,opts,scope)) in PROFILES.iter().zip(expected) {
        assert_eq!((p.format,p.tool,p.options,p.target),(format,tool,opts,scope));
        assert!(matches!(p.directory,"bin"|"sbin") && !p.tool.contains('/'));
    }
}
#[test]
fn no_target_path_or_checker_arguments_may_be_passed(){
    let query=|words:&[&str]|Query::parse(words.iter().map(|&v|OsString::from(v)));
    assert!(query(&[]).unwrap().selected.is_none());
    assert_eq!(query(&["ntfs"]).unwrap().selected,Some(1));
    let hash="Ab".repeat(32);
    assert_eq!(query(&["zfs","--sha256",&hash]).unwrap().expected,Some([0xab;32]));
    for bad in [&["/dev/sda"][..],&["--sha256","aa"],&["ntfs","-y"],&["zfs","scrub"],
        &["fat32","--sha256","zz"],&["ext4","--sha256",&hash,"/tmp/tool"],&["btrfs","--repair"]] {
        assert!(query(bad).is_err(),"{bad:?}");
    }
}
#[test]
fn missing_unchecked_mismatch_and_unavailable_versions_are_not_device_results(){
    let report=|e,requested_hash|Report{rows:vec![Row{profile:PROFILES[2],installed:e}],requested_hash};
    assert_eq!(report(Evidence::Missing,false).exit(),0);
    assert_eq!(report(Evidence::Missing,true).exit(),2);
    assert_eq!(report(Evidence::Observed(Record::Different),true).exit(),1);
    assert_eq!(report(Evidence::DigestUnavailable,true).exit(),2);
    let positive=report(Evidence::Observed(Record::Equal),true);
    assert_eq!(positive.exit(),0);
    let s=positive.to_string();
    assert!(s.contains("checker=not-executed device=not-opened repair=not-enabled"));
    assert!(s.contains("sha256=matches-reference version=unverified signature=unverified"));
    assert!(s.contains("preview=zpool status <pool>"));
}

#[test]
fn only_supported_bundled_slots_are_private_and_detection_is_not_execution(){
    for p in PROFILES {
        let path = p.slots().join("/");
        if p.format=="fat32" || p.format=="exfat" {
            assert!(path.starts_with("usr/lib/local-ops-helper/disk-tools-v1/bin/"));
        } else {
            assert!(!p.private());assert!(!path.contains("disk-tools-v1"));
        }
    }
    assert_eq!(PROFILES[5].action(),"planned-check-not-enabled");
    assert_eq!(PROFILES[2].action(),"host-version-and-loaded-module-required-not-enabled");
}
