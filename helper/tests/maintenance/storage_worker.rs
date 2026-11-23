use super::*;

const NORMAL: &[u8] = b"Name:\tx\nUid:\t1002\t1002\t1002\t1002\nGid:\t1013\t1013\t1013\t1013\nGroups:\t\nCapInh:\t0000000000000000\nCapPrm:\t0000000000000000\nCapEff:\t0000000000000000\nCapAmb:\t0000000000000000\nNoNewPrivs:\t1\n";

// Pure classifier. This does not change the calling process or establish an operator grant.
#[test]
fn only_the_complete_restricted_storage_subject_is_eligible_for_a_first_link() {
    check_status(NORMAL, 1002, 1013).unwrap();
    assert!(check_status(NORMAL, 1002, 1002).is_err()); // the configured GID is independent of the UID
    for gid in [0, 1012, u32::MAX] {
        assert!(check_status(NORMAL, 1002, gid).is_err());
    }
    for uid in [0, 1001, u32::MAX] {
        assert!(check_status(NORMAL, uid, 1013).is_err());
    }
    let input = std::str::from_utf8(NORMAL).unwrap();
    check_status(
        input.replace("Groups:\t\n", "Groups:\t1013 \n").as_bytes(),
        1002,
        1013,
    )
    .unwrap();
    for (key, wrong) in [
        ("Uid:", "0 1002 1002 1002"),
        ("Uid:", "1002 1002 0 1002"),
        ("Uid:", "1002 1002 1002 42"),
        ("Gid:", "1013 0 1013 1013"),
        ("Groups:", "1013 42"),
        ("Groups:", "0"),
        ("Groups:", "1013 1013"),
        ("Groups:", "1014"),
        ("Groups:", "1013x"),
        ("CapInh:", "0000000000000004"),
        ("CapPrm:", "0000000000000004"),
        ("CapEff:", "0000000000000004"),
        ("CapAmb:", "0000000000000004"),
        ("NoNewPrivs:", "0"),
    ] {
        let bad = input
            .lines()
            .map(|s| {
                if s.starts_with(key) {
                    format!("{key}\t{wrong}\n")
                } else {
                    format!("{s}\n")
                }
            })
            .collect::<String>();
        assert!(check_status(bad.as_bytes(), 1002, 1013).is_err());
    }
    for bad in [
        input.replace("Groups:\t\n", ""),
        format!("{input}Groups:\t1013\n"),
        input.replace("NoNewPrivs:\t1\n", ""),
        format!("{input}CapEff:\t0000000000000000\n"),
        input.trim_end().to_owned(),
        format!("Name:{}\n{input}", "x".repeat(STATUS_MAX)),
    ] {
        assert!(check_status(bad.as_bytes(), 1002, 1013).is_err());
    }
}
