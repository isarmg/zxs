use super::*;
fn sb() -> [u8; HEADER] {
    // Non-power-of-two AG length; the packed log FSB is deliberately
    // greater than the linear data-block count near the last group's end.
    let mut raw = [0; HEADER];
    raw[..4].copy_from_slice(b"XFSB");
    set(&mut raw, 4, 4096);
    wide(&mut raw, 8, 3 * 50_000 + 18_000);
    wide(&mut raw, 48, (3 << 16) + 1000);
    set(&mut raw, 84, 50_000);
    set(&mut raw, 88, 4);
    set(&mut raw, 96, 800);
    raw[100..102].copy_from_slice(&5u16.to_be_bytes());
    raw[102..104].copy_from_slice(&512u16.to_be_bytes());
    raw[120] = 12;
    raw[121] = 9;
    raw[124] = 16;
    raw
}
fn wide(s: &mut [u8; HEADER], i: usize, n: u64) {
    s[i..i + 8].copy_from_slice(&n.to_be_bytes());
}
fn set(s: &mut [u8; HEADER], i: usize, n: u32) {
    s[i..i + 4].copy_from_slice(&n.to_be_bytes());
}
const BYTES: u64 = 168_000 * 4096;
const _: () = assert!(BYTES > 128 << 20);
#[test]
fn only_a_full_single_volume_with_consistent_internal_log_and_no_rt_is_admitted() {
    let full = sb();
    assert!(frame(&full, BYTES).is_ok());
    let bigger = (1u64 << 44) + 4096;
    assert_eq!(frame(&full, bigger).unwrap().filesystem_bytes, BYTES);
    assert!(frame(&full, BYTES - 1).is_err());
    let mut wrong = full;
    wrong[3] = b'G';
    assert!(frame(&wrong, BYTES).is_err());
    for (i, n) in [
        (16, 1),
        (24, 1),
        (48, 0),
        (48, 4 << 16),
        (48, (3 << 16) + 17_999),
        (48, (2 << 16) + 49_999),
    ] {
        let mut raw = full;
        wide(&mut raw, i, n);
        assert!(frame(&raw, BYTES).is_err(), "{i} {n}");
    }
    for (i, n) in [(4, 4095), (88, 0), (88, 5), (92, 1), (96, 0), (216, 0x10)] {
        let mut raw = full;
        set(&mut raw, i, n);
        assert!(frame(&raw, BYTES).is_err(), "{i} {n}");
    }
    let mut incomplete = full;
    incomplete[126] = 1;
    assert!(frame(&incomplete, BYTES).is_err());
}
fn transcript(geometry: bool, ag3: &[u32], ag4: &[u32]) -> Vec<u8> {
    let mut lines = Vec::<Vec<u8>>::new();
    let add = |rows: &mut Vec<Vec<u8>>, extras: &[&[u8]]| {
        rows.extend(extras.iter().map(|line| line.to_vec()));
    };
    let ags = |rows: &mut Vec<Vec<u8>>, ids: &[u32]| {
        rows.extend(
            ids.iter()
                .map(|id| format!("        - agno = {id}").into_bytes()),
        );
    };
    if geometry {
        add(&mut lines, &BOUNDARY);
    }
    add(&mut lines, &[PHASE1]);
    if geometry {
        add(&mut lines, &BOUNDARY);
    }
    add(&mut lines, &BEFORE_AG3);
    ags(&mut lines, ag3);
    add(&mut lines, &BEFORE_AG4);
    ags(&mut lines, ag4);
    add(&mut lines, &END);
    [lines.join(&b'\n'), b"\n".to_vec()].concat()
}
#[test]
fn only_exact_no_modify_completion_clears_the_positive_gate() {
    let e = frame(&sb(), BYTES).unwrap();
    let good = [0, 1, 2, 3];
    let reversed = [3, 2, 1, 0];
    let valid = transcript(true, &good, &reversed);
    assert_eq!(classify(0, SKIP5, &valid, e).exit(), 0);
    assert_eq!(
        classify(0, SKIP5, &transcript(false, &good, &good), e).exit(),
        0
    );
    for n in [1, 2, 4, 64, 127] {
        assert_eq!(classify(n, SKIP5, &valid, e).exit(), 2);
    }
    for out in [
        b"".as_slice(),
        b"done\n",
        b"No modify flag set, skipping phase 5",
    ] {
        assert_eq!(classify(0, out, &valid, e).exit(), 2);
    }
    let insert_after = |part: &str, warning: &str| {
        String::from_utf8(valid.clone())
            .unwrap()
            .replace(part, &format!("{part}{warning}"))
            .into_bytes()
    };
    let log = b"        - zero log...\n";
    let dirty = insert_after(
        std::str::from_utf8(log).unwrap(),
        "ALERT: The filesystem has valuable metadata changes in a log which is being\nignored because the -n option was used.  Expect spurious inconsistencies\nwhich may be resolved by first mounting the filesystem to replay the log.\n",
    );
    let unexpected = insert_after(
        std::str::from_utf8(log).unwrap(),
        "zero_log: cannot find log head/tail (xlog_find_tail=-5)\n",
    );
    assert_eq!(classify(0, SKIP5, &valid[..valid.len() - 1], e).exit(), 2);
    for err in [
        dirty,
        unexpected,
        transcript(true, &good, &[0, 1, 1, 3]),
        transcript(false, &[0, 1, 2], &good),
        transcript(false, &good, &[0, 1, 2, 4]),
        [valid.clone(), b"done\n".to_vec()].concat(),
    ] {
        assert_eq!(classify(0, SKIP5, &err, e).exit(), 2);
    }
}
#[test]
fn parent_receipt_requires_the_already_known_image_and_actual_matching_exit_not_an_ext_or_partial_result()
 {
    let bytes = 1u64 << 42;
    let e = frame(&sb(), bytes).unwrap();
    let sha = [0x61; 32];
    let good = [0, 1, 2, 3];
    let exact = transcript(false, &good, &good);
    let packet = classify(0, SKIP5, &exact, e).packet(&sha);
    assert_eq!(
        Report::from_packet(&packet, Some(0), bytes, &sha)
            .unwrap()
            .filesystem_bytes,
        BYTES
    );
    assert!(Report::from_packet(&packet, Some(0), bytes - 1, &sha).is_err());
    assert!(Report::from_packet(&packet, Some(2), bytes, &sha).is_err());
    assert!(Report::from_packet(&packet, Some(0), bytes, &[0; 32]).is_err());
    assert!(Report::from_packet(&packet[..56], Some(0), bytes, &sha).is_err());
    let mut bad = packet;
    bad[..4].copy_from_slice(b"XCR1");
    assert!(Report::from_packet(&bad, Some(0), bytes, &sha).is_err());
    let incomplete = classify(1, b"", b"whatever failed\n", e).packet(&sha);
    assert_eq!(
        Report::from_packet(&incomplete, Some(2), bytes, &sha)
            .unwrap()
            .exit(),
        2
    );
    assert!(Report::from_packet(&incomplete, Some(3), bytes, &sha).is_err());
}
