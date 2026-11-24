use super::*;

const FIRST: Id = [0xaf; 16];
const FIRST_HEX: &str = "afafafafafafafafafafafafafafafaf";
const ROLES: [[u64; 3]; 3] = [
    [
        0x0102_0304_0506_0708,
        0x1112_1314_1516_1718,
        0x2122_2324_2526_2728,
    ],
    [0x31, 0x32, 0x33],
    [0x41, 0x42, 0x43],
];
// Independently computed SHA-256 of the exact v1 transcript, not a codec round trip.
const BINDING: [u8; 32] = [
    0x2b, 0x77, 0xe2, 0x8a, 0x15, 0x92, 0xbf, 0xc7, 0xc8, 0x3e, 0x8f, 0x56, 0x18, 0xd1, 0x98, 0xf7,
    0x4c, 0x0d, 0x19, 0x92, 0xbd, 0xa7, 0x92, 0x50, 0x71, 0x63, 0xb7, 0x25, 0x41, 0xee, 0x67, 0x8d,
];

fn parse(values: &[&str]) -> Result<OfflineVerify, &'static str> {
    OfflineVerify::parse(values.iter().map(OsString::from))
}

#[test]
fn verify_selects_one_exact_nonzero_id_and_original_roles() {
    let request = parse(&[FIRST_HEX, "/system", "/recycle", "/one", "/two"]).unwrap();
    assert_eq!(request.container, FIRST);
    assert_eq!(
        request.roots,
        ["/system", "/recycle", "/one", "/two"].map(PathBuf::from)
    );
    for bad in [
        "",
        "00000000000000000000000000000000",
        "AFafafafafafafafafafafafafafafaf",
        "x0000000000000000000000000000000",
        "afafafafafafafafafafafafafafafaf ",
        "--all",
    ] {
        assert!(parse(&[bad, "/system", "/recycle", "/one"]).is_err());
    }
    assert!(parse(&[FIRST_HEX, "/system", "/recycle"]).is_err());
    assert!(parse(&[FIRST_HEX, "/system", "/recycle", "/one", "--repair"]).is_err());
}

#[test]
fn verify_live_binding_is_ordered_and_fails_closed() {
    let descriptor = [0x42; 32];
    assert_eq!(live_role_binding(&descriptor, &ROLES).unwrap(), BINDING);
    assert_ne!(live_role_binding(&[0x43; 32], &ROLES).unwrap(), BINDING);

    let mut other = ROLES;
    other.swap(0, 1);
    assert_ne!(live_role_binding(&descriptor, &other).unwrap(), BINDING);
    let mut other = ROLES;
    other[2][2] += 1; // same device and inode, different live mount
    assert_ne!(live_role_binding(&descriptor, &other).unwrap(), BINDING);

    assert!(live_role_binding(&descriptor, &ROLES[..2]).is_err());
    assert!(live_role_binding(&descriptor, &[[1; 3]; 65]).is_err());
    for field in 1..3 {
        let mut other = ROLES;
        other[2][field] = 0;
        assert!(live_role_binding(&descriptor, &other).is_err());
    }
    let mut other = ROLES;
    other[0][0] = 0; // the held-root proof, not this codec, decides eligible devices
    assert_ne!(live_role_binding(&descriptor, &other).unwrap(), BINDING);
}

#[test]
fn verify_sealed_order_has_exact_live_binding_scope_and_selected_id() {
    let raw = order(&BINDING, &[4; 32], &FIRST).unwrap();
    assert_eq!(raw.len(), 88);
    assert_eq!(&raw[..8], b"ZXOVF1\0\0");
    assert_eq!(raw[8..40], BINDING);
    assert_eq!(raw[40..72], [4; 32]);
    assert_eq!(raw[72..88], FIRST);
    assert!(order(&BINDING, &[0; 32], &FIRST).is_err());
    assert!(order(&BINDING, &[4; 32], &[0; 16]).is_err());
}

fn result(receipts: u64, bodies: u64, bytes: u64, area: u8) -> Payload {
    let mut out = [0; 65];
    out[0] = 1;
    out[1..17].copy_from_slice(&FIRST);
    out[17..25].copy_from_slice(&receipts.to_be_bytes());
    out[25..33].copy_from_slice(&bodies.to_be_bytes());
    out[33..41].copy_from_slice(&bytes.to_be_bytes());
    out[41] = area;
    out
}

#[test]
fn exact_terminal_report_not_any_partial_or_selected_other_id() {
    let record = result(1, 1, 0, 2);
    let verified = Verification::decode(record, &FIRST).unwrap();
    assert_eq!(verified.area, Area::Recycle);
    let mut output = Vec::new();
    verified.write(&mut output, &FIRST).unwrap();
    assert_eq!(
        String::from_utf8(output).unwrap(),
        format!("verified container {FIRST_HEX}: area=RECYCLE receipts=1 payloads=1 raw_bytes=0\n")
    );

    assert!(Verification::decode(record, &[0xef; 16]).is_err());
    assert!(Verification::decode(result(0, 1, 0, 1), &FIRST).is_err());
    assert!(Verification::decode(result(1, 0, 0, 1), &FIRST).is_err());
    assert!(Verification::decode(result(1, 2, 0, 1), &FIRST).is_err());
    assert!(Verification::decode(result(1, 1, 42, 3), &FIRST).is_err());

    for pos in std::iter::once(0).chain(42..65) {
        let mut changed = record;
        changed[pos] ^= 1;
        assert!(Verification::decode(changed, &FIRST).is_err());
    }
}
