//! Vectors for the actual Helper parser, sealed order and terminal bridge.
use super::super::{is_other_stage, live_role_binding};
use super::*;
use std::os::unix::ffi::OsStringExt;

const LIVE: [[u64; 3]; 4] = [
    [
        0x0102_0304_0506_0708,
        0x1112_1314_1516_1718,
        0x2122_2324_2526_2728,
    ],
    [0x31, 0x32, 0x33],
    [0x41, 0x42, 0x43],
    [0x51, 0x52, 0x53],
];

fn args() -> Vec<OsString> {
    vec![
        hex(&[0x31; 16]).into(),
        hex(&[0x32; 16]).into(),
        "/zxs/a".into(),
        OsString::from_vec(b"/zxs/\xff".to_vec()),
        "/zxs/sys".into(),
        "/zxs/r".into(),
        "/zxs/current".into(),
    ]
}

fn request() -> Request {
    DiskMove::parse(args().into_iter()).unwrap().request
}

#[test]
fn order_matches_exact_native_operand_and_live_roster_vectors() {
    let selected = request();
    let binding = live_role_binding(ROLE_DOMAIN, &[0x42; 32], &LIVE).unwrap();
    assert_eq!(
        hex(&binding),
        "9e9677fc4c09673a235b3bc2a88197afc6c588a1c4086ba7750c0ad8aedff98c"
    );
    assert_eq!(
        hex(&selected.source),
        "782d96da9a7c4516df1b1a72dc4982faaa05befc9c5dd71469047253ebb48434"
    );
    assert_eq!(
        hex(&selected.target),
        "f545a79a125aaa6c1c9d8f4bc81a0d7ff3ae8d521b9a726a1a179d73bf1ee0a6"
    );
    let raw = selected.encode(&binding, &[0x24; 32]).unwrap();
    assert_eq!(raw.len(), 168);
    assert_eq!(&raw[..8], b"ZXDMV1\0\0");
    assert_eq!(raw[72..88], selected.container);
    assert_eq!(raw[88..104], selected.operation);
    assert_eq!(
        hex(&Sha256::digest(raw)),
        "bdbd276eb6e0aee8b13a896d9e490d847ba61177e51bc9e321387e443eb589d6"
    );
    assert!(selected.encode(&binding, &[0; 32]).is_err());
    let invalid = Request {
        source: [0; 32],
        ..selected
    };
    assert!(invalid.encode(&binding, &[0x24; 32]).is_err());
    let mut changed = LIVE;
    changed.swap(0, 1);
    assert_ne!(
        live_role_binding(ROLE_DOMAIN, &[0x42; 32], &changed).unwrap(),
        binding
    );
    changed[3][2] = 0;
    assert!(live_role_binding(ROLE_DOMAIN, &[0x42; 32], &changed).is_err());
}

#[test]
fn actual_cli_never_requires_historical_operands_to_be_current_paths() {
    let parsed = DiskMove::parse(args().into_iter()).unwrap();
    assert_eq!(
        parsed.roots,
        ["/zxs/sys", "/zxs/r", "/zxs/current"].map(PathBuf::from)
    );
    assert!(
        !parsed
            .request
            .current_pair(parsed.roots.iter().skip(2).map(PathBuf::as_path))
            .unwrap()
    );
    let live_target = OsString::from_vec(b"/zxs/\xff".to_vec());
    assert!(
        parsed
            .request
            .current_pair([Path::new("/zxs/a"), Path::new(&live_target)].into_iter())
            .unwrap()
    );

    let values = [OsString::from("disk"), "move".into()]
        .into_iter()
        .chain(args());
    assert!(matches!(
        crate::parse(values),
        Ok(crate::Command::DiskMove(_))
    ));
    let values = [OsString::from("disk"), "force".into()]
        .into_iter()
        .chain(args());
    assert!(crate::parse(values).is_err());
    for (at, bad) in [
        (0, "0".repeat(32)),
        (1, "AB".repeat(16)),
        (2, "/".into()),
        (3, "/zxs/a".into()),
    ] {
        let mut input = args();
        input[at] = bad.into();
        assert!(DiskMove::parse(input.into_iter()).is_err());
    }
    for bad in [&b"relative"[..], b"/x/..", b"/x//y", b"/x/", b"/x/\0"] {
        assert!(operand(bad).is_err());
    }
    let mut oversized = vec![b'a'; 4096];
    oversized[0] = b'/';
    assert!(operand(&oversized).is_err());
}

#[test]
fn only_the_exact_same_operation_guard_is_allowed_and_not_after_success() {
    let request = request();
    let own = request.guard_name();
    assert_eq!(
        own,
        format!(
            ".server-stage-move-{}-{}",
            hex(&request.container),
            hex(&request.operation)
        )
    );
    assert!(!is_other_stage(own.as_bytes(), Some(&own)));
    for other in [".server-stage-retire-data", ".server-stage-move-unproved"] {
        assert!(is_other_stage(other.as_bytes(), Some(&own)));
    }
    let different = Request {
        operation: [0x33; 16],
        ..request
    }
    .guard_name();
    assert!(is_other_stage(different.as_bytes(), Some(&own)));
    assert!(is_other_stage(own.as_bytes(), None));
    // An inactive, unproved draft is not an active ordinary-startup interlock.
    assert!(!is_other_stage(b".move-guard-draft-private", None));
}

#[test]
fn only_exact_history_or_current_completion_can_be_reported() {
    let selected = request();
    let mut raw = [0x53; 65];
    raw[0] = 1;
    raw[1..17].copy_from_slice(&selected.container);
    raw[17..33].copy_from_slice(&selected.operation);
    assert_eq!(
        Completion::decode(raw, selected, true).unwrap(),
        Completion::Completed([0x53; 32])
    );
    assert!(Completion::decode(raw, selected, false).is_err());
    raw[0] = 2;
    let previous = Completion::decode(raw, selected, false).unwrap();
    assert_eq!(previous, Completion::Historical([0x53; 32]));
    let mut result = Vec::new();
    previous.write(&mut result, selected).unwrap();
    let result = String::from_utf8(result).unwrap();
    assert!(result.starts_with("historical move completion for "));
    assert!(result.ends_with("current location not checked\n"));

    for slot in [0, 1, 17] {
        let mut invalid = raw;
        invalid[slot] ^= 0x7f;
        assert!(Completion::decode(invalid, selected, true).is_err());
    }
}
