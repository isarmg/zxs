use super::*;

const ROLE: RecycleRole = RecycleRole {
    pool: [3; 16],
    token: [5; 32],
};
const DATA: DiskToken = [4; 32];
const OP: Mutation = Mutation {
    container: [1; 16],
    request: [2; 16],
};
fn entity(device: u64, inode: u64) -> Entity {
    Entity { device, inode }
}

#[test]
fn exact_v2_bytes_and_remount_require_the_original_role_and_inodes() {
    let record = RecycleReceipt::enrolled(OP, 9, ROLE, DATA, (31, 41), [6; 32]).unwrap();
    let exact = [
        b"RC2\0".as_slice(),
        &[1; 16],
        &[2; 16],
        &9u64.to_be_bytes(),
        &[3; 16],
        &[4; 32],
        &[5; 32],
        &31u64.to_be_bytes(),
        &41u64.to_be_bytes(),
        &[6; 32],
    ]
    .concat();
    assert_eq!(exact.len(), 172);
    assert_eq!(record.encode(), exact);
    assert_eq!(RecycleReceipt::decode(&exact).unwrap(), record);
    for device in [17, 23] {
        record.require_source(DATA, entity(device, 31)).unwrap();
        record.require_arrival(ROLE, entity(device, 41)).unwrap();
    }
    assert!(record.require_source(ROLE.token, entity(23, 31)).is_err());
    assert!(record.require_source(DATA, entity(23, 32)).is_err());
    assert!(record.require_arrival(ROLE, entity(23, 42)).is_err());
    assert!(
        record
            .require_arrival(
                RecycleRole {
                    pool: [7; 16],
                    ..ROLE
                },
                entity(23, 41)
            )
            .is_err()
    );
    assert!(
        record
            .require_arrival(
                RecycleRole {
                    token: DATA,
                    ..ROLE
                },
                entity(23, 41)
            )
            .is_err()
    );
    for (l, r) in [
        (4, 20),
        (20, 36),
        (36, 44),
        (44, 60),
        (60, 92),
        (92, 124),
        (124, 132),
        (132, 140),
    ] {
        let mut zero = exact.clone();
        zero[l..r].fill(0);
        assert!(RecycleReceipt::decode(&zero).is_err());
    }
    let mut aliases = exact.clone();
    aliases[60..92].copy_from_slice(&ROLE.token);
    assert!(RecycleReceipt::decode(&aliases).is_err());
    assert!(RecycleReceipt::decode(&exact[..171]).is_err());
    assert!(RecycleReceipt::decode(&[exact.as_slice(), b"\0"].concat()).is_err());
}

#[test]
fn rc1_cannot_be_reinterpreted_after_device_change() {
    let exact = [
        b"RC1\0".as_slice(),
        &[1; 16],
        &[2; 16],
        &9u64.to_be_bytes(),
        &17u64.to_be_bytes(),
        &31u64.to_be_bytes(),
        &23u64.to_be_bytes(),
        &41u64.to_be_bytes(),
        &[6; 32],
    ]
    .concat();
    let record = RecycleReceipt::decode(&exact).unwrap();
    assert!(!record.is_enrolled());
    assert_eq!(record.encode(), exact);
    record.require_source(DATA, entity(17, 31)).unwrap();
    record.require_arrival(ROLE, entity(23, 41)).unwrap();
    assert!(record.require_source(DATA, entity(19, 31)).is_err());
    assert!(record.require_arrival(ROLE, entity(29, 41)).is_err());
    let mut relabeled = exact;
    relabeled[2] = b'2';
    assert!(RecycleReceipt::decode(&relabeled).is_err());
}
