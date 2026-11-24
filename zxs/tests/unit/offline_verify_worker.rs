use super::*;

const BINDING: [u8; 32] = [7; 32];
const SCOPE: [u8; 32] = [8; 32];
const CONTAINER: Id = [9; 16];

#[test]
fn exact_length_magic_binding_scope_and_container_are_required() {
    let mut raw = b"ZXOVF1\0\0".to_vec();
    raw.extend(BINDING);
    raw.extend(SCOPE);
    raw.extend(CONTAINER);
    assert_eq!(raw.len(), ORDER_BYTES);
    assert_eq!(order(&raw, BINDING).unwrap(), (SCOPE, CONTAINER));
    assert!(order(&raw, [0; 32]).is_err());
    assert!(order(&raw[..raw.len() - 1], BINDING).is_err());
    let mut extra = raw.clone();
    extra.push(0);
    assert!(order(&extra, BINDING).is_err());
    for field in [40..72, 72..88] {
        let mut unauthorized = raw.clone();
        unauthorized[field].fill(0);
        assert!(order(&unauthorized, BINDING).is_err());
    }
}

#[test]
fn terminal_can_report_zero_payload_bytes_but_not_missing_bodies() {
    let mut record = Verification {
        location: VerifiedLocation::Recycle,
        receipts: 3,
        bodies: 2,
        bytes: 0,
    };
    let raw = terminal(CONTAINER, record).unwrap();
    assert_eq!(raw[0], 1);
    assert_eq!(raw[1..17], CONTAINER);
    assert_eq!(raw[17..25], 3_u64.to_be_bytes());
    assert_eq!(raw[25..33], 2_u64.to_be_bytes());
    assert_eq!(raw[33..41], [0; 8]);
    assert_eq!(raw[41], 2);
    assert_eq!(raw[42..65], [0; 23]);
    record.bodies = 0;
    assert!(terminal(CONTAINER, record).is_err());
    record.bodies = 4;
    assert!(terminal(CONTAINER, record).is_err());
    assert!(terminal([0; 16], record).is_err());
}

#[test]
fn exact_live_binding_vector_covers_order_and_each_mount_identity() {
    let descriptor = [0x42; 32];
    let ordered = [
        [
            0x0102_0304_0506_0708,
            0x1112_1314_1516_1718,
            0x2122_2324_2526_2728,
        ],
        [0x31, 0x32, 0x33],
        [0x41, 0x42, 0x43],
    ];
    let expected = [
        0x2b, 0x77, 0xe2, 0x8a, 0x15, 0x92, 0xbf, 0xc7, 0xc8, 0x3e, 0x8f, 0x56, 0x18, 0xd1, 0x98,
        0xf7, 0x4c, 0x0d, 0x19, 0x92, 0xbd, 0xa7, 0x92, 0x50, 0x71, 0x63, 0xb7, 0x25, 0x41, 0xee,
        0x67, 0x8d,
    ];
    assert_eq!(live_binding(descriptor, &ordered).unwrap(), expected);
    let mut swap = ordered;
    swap.swap(0, 1);
    assert_ne!(live_binding(descriptor, &swap).unwrap(), expected);
    let mut remount = ordered;
    remount[2][2] = 0x44;
    assert_ne!(live_binding(descriptor, &remount).unwrap(), expected);
    remount[2][2] = 0;
    assert!(live_binding(descriptor, &remount).is_err());
    assert!(live_binding(descriptor, &ordered[..2]).is_err());
}
