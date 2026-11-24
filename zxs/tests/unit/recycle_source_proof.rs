use super::*;

fn stamped(dev: u64) -> [u8; OPAQUE_STAMP_BYTES] {
    let mut row = [0; OPAQUE_STAMP_BYTES];
    for (word, value) in row
        .as_chunks_mut::<8>()
        .0
        .iter_mut()
        .zip([dev, 3, 4, 5, 6, 7, 8])
    {
        *word = value.to_be_bytes();
    }
    row
}

#[test]
fn durable_row_ignores_only_device_and_retains_live_mount_fence() {
    let directory = |device| Entity { device, inode: 43 };
    let make = |device, file_stamp| {
        let mut row = SourceRow::enrolled(directory(device), &file_stamp);
        row.bind_body(&file_stamp);
        row
    };
    let before = make(17, stamped(17));
    assert_eq!(before.bytes().len(), 104);
    assert_eq!(before.bytes(), make(19, stamped(19)).bytes());
    assert_ne!(stamped(17), stamped(19)); // The full live stamp was not weakened.
    let witness = SourceRow::read(&mut before.bytes(), ProofFormat::Enrolled, true).unwrap();
    witness.require_directory(directory(19), true).unwrap();
    assert!(witness.require_directory(directory(19), false).is_err());
    assert!(
        witness
            .require_directory(
                Entity {
                    device: 19,
                    inode: 42
                },
                true
            )
            .is_err()
    );
    assert!(SourceRow::read(&mut before.bytes(), ProofFormat::Enrolled, false).is_err());
    assert!(SourceRow::read(&mut before.bytes(), ProofFormat::Legacy, true).is_err());
    for byte in 8..56 {
        let mut changed = stamped(19);
        changed[byte] ^= 1;
        assert_ne!(before.bytes(), make(19, changed).bytes());
    }
}
