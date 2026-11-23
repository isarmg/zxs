use super::*;

#[test]
fn fdinfo_proves_only_a_read_only_existing_exclusive_flock_for_the_exact_inode() {
    // SAFETY: integer-only device coordinate packing in this synthetic fixture.
    let device = unsafe { zxs_lock_device(8, 17) };
    let exact = b"pos:\t0\nflags:\t02000000\nmnt_id:\t42\nino:\t19\nlock:\t1: FLOCK  ADVISORY  WRITE 0 08:11:19 0 EOF\n";
    assert!(exclusive_fdinfo(exact, device, 19));
    assert!(!exclusive_fdinfo(exact, device, 20));
    assert!(!exclusive_fdinfo(exact, device.wrapping_add(1), 19));
    for (from, to) in [
        ("WRITE", "READ"),
        ("FLOCK", "POSIX"),
        ("02000000", "02000002"),
        ("0 EOF", "1 EOF"),
        ("0 EOF", "0 20"),
        (" 08:11:19 ", " 08:10:19 "),
        (" 08:11:19 ", " 08:11:18 "),
    ] {
        let invalid = std::str::from_utf8(exact).unwrap().replace(from, to);
        assert!(!exclusive_fdinfo(invalid.as_bytes(), device, 19));
    }
    let without_lock = exact
        .split_inclusive(|byte| *byte == b'\n')
        .take(4)
        .flatten()
        .copied()
        .collect::<Vec<_>>();
    assert!(!exclusive_fdinfo(&without_lock, device, 19));
    let duplicate = [
        exact.as_slice(),
        b"lock:\t2: FLOCK ADVISORY WRITE 0 08:11:19 0 EOF\n",
    ]
    .concat();
    assert!(!exclusive_fdinfo(&duplicate, device, 19));
}

#[test]
fn framed_names_are_exact_sorted_and_end_marked() {
    use std::collections::BTreeSet;

    let names = BTreeSet::from(["a".to_owned(), "A".to_owned()]);
    let mut frame = Vec::new();
    let binding = [0x41; 32];
    write_frame(&mut frame, binding, &names).unwrap();
    assert_eq!(
        frame,
        [
            MAGIC.as_slice(),
            &binding,
            b"\0\0\0\0\0\0\0\x02\0\x01A\0\x01aZXEND1\0\0",
        ]
        .concat()
    );
}
