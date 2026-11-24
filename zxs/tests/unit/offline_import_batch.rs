use super::*;

const BINDING: [u8; 32] = [0x51; 32];
const SCOPE: [u8; 32] = [0x72; 32];

fn field(out: &mut Vec<u8>, raw: &[u8]) {
    out.extend(u16::try_from(raw.len()).unwrap().to_be_bytes());
    out.extend_from_slice(raw);
}

fn ticket(seed: u8, relative: &[u8], name: &[u8]) -> Vec<u8> {
    let mut raw = b"ZXIMP1\0\0".to_vec();
    raw.extend([seed; 16]);
    raw.extend([1; 16]);
    raw.extend([2; 32]);
    for part in [b"/fixture/target".as_slice(), b"/fixture/source", relative] {
        field(&mut raw, part);
    }
    raw.extend(3_u64.to_be_bytes()); // device
    raw.extend(4_u64.to_be_bytes()); // inode
    raw.extend(2_310_u32.to_be_bytes()); // uid
    raw.extend(0o100_600_u32.to_be_bytes());
    raw.extend(1_u64.to_be_bytes()); // links
    for number in [5_i64, 6, 7, 8] {
        raw.extend(number.to_be_bytes());
    }
    raw.extend(6_u64.to_be_bytes()); // payload length and digest
    raw.extend([9; 32]);
    field(&mut raw, name);
    raw
}

fn order(tickets: &[&[u8]]) -> Vec<u8> {
    let mut frame = b"ZXBATCH1".to_vec();
    frame.extend(u64::try_from(tickets.len()).unwrap().to_be_bytes());
    for raw in tickets {
        frame.extend(u32::try_from(raw.len()).unwrap().to_be_bytes());
        frame.extend_from_slice(raw);
    }
    sealed_frame(&frame)
}

fn sealed_frame(frame: &[u8]) -> Vec<u8> {
    let mut raw = b"ZXIBC1\0\0".to_vec();
    raw.extend(BINDING);
    raw.extend(SCOPE);
    raw.extend_from_slice(frame);
    raw.extend(digest(frame).unwrap());
    raw
}

#[test]
fn plan_keeps_exact_order_and_independent_ticket_and_envelope_digests() {
    let a = ticket(8, b"a.bin", b"a.bin");
    let b = ticket(3, b"folder/b.bin", b"b.bin");
    let raw = order(&[&a, &b]);
    let plan = parse_plan(&raw, BINDING).unwrap();
    assert_eq!(plan.scope, SCOPE);
    assert_eq!(plan.digest, digest(&raw).unwrap());
    assert_eq!(plan.rows.len(), 2);
    assert_eq!(plan.rows[0].intent.seed, [8; 16]);
    assert_eq!(plan.rows[1].intent.seed, [3; 16]);
    assert_eq!(plan.rows[0].digest, digest(&a).unwrap());
    assert_eq!(plan.rows[1].digest, digest(&b).unwrap());
}

#[test]
fn malformed_or_trailing_end_and_late_bad_ticket_refuse_the_whole_plan() {
    let a = ticket(8, b"a.bin", b"a.bin");
    let b = ticket(3, b"folder/b.bin", b"b.bin");
    let raw = order(&[&a, &b]);
    assert!(parse_plan(&raw, [0; 32]).is_err());
    assert!(parse_plan(&raw[..raw.len() - 1], BINDING).is_err());
    let mut bad_digest = raw.clone();
    *bad_digest.last_mut().unwrap() ^= 1;
    assert!(parse_plan(&bad_digest, BINDING).is_err());

    let mut bad = raw.clone();
    bad[40..72].fill(0);
    assert!(parse_plan(&bad, BINDING).is_err());
    let mut frame = raw[72..raw.len() - 32].to_vec();
    frame.push(0);
    assert!(parse_plan(&sealed_frame(&frame), BINDING).is_err());
    let mut late_bad_ticket = b;
    late_bad_ticket[0] ^= 1;
    assert!(parse_plan(&order(&[&a, &late_bad_ticket]), BINDING).is_err());
}

#[test]
fn duplicates_mixed_selection_and_oversized_counts_are_not_canonical() {
    let a = ticket(8, b"a.bin", b"a.bin");
    for b in [
        ticket(8, b"second", b"second"),
        ticket(3, b"a.bin", b"second"),
        ticket(3, b"second", b"a.bin"),
    ] {
        assert!(parse_plan(&order(&[&a, &b]), BINDING).is_err());
    }
    let mut other_pool = ticket(3, b"second", b"second");
    other_pool[24] ^= 1;
    assert!(parse_plan(&order(&[&a, &other_pool]), BINDING).is_err());
    assert!(parse_plan(&order(&[]), BINDING).is_err());
    let raw = order(&[&a]);
    let mut frame = raw[72..raw.len() - 32].to_vec();
    frame[8..16].copy_from_slice(&65_537_u64.to_be_bytes());
    assert!(parse_plan(&sealed_frame(&frame), BINDING).is_err());
}

#[test]
fn only_the_confirmed_row_index_and_ids_with_no_fd_can_acknowledge_it() {
    let mut confirmed = numbered(step(6, 1), 68);
    confirmed[9..25].fill(9);
    confirmed[25..41].fill(7);
    confirmed[41..57].fill(5);
    assert_eq!(&confirmed[57..65], &68_u64.to_be_bytes());
    let mut ack = confirmed;
    ack[0] = 7;
    assert!(check_ack(confirmed, ack, false).is_ok());
    assert!(check_ack(confirmed, ack, true).is_err());
    assert!(check_ack(confirmed, numbered(ack, 69), false).is_err());
    ack[25] ^= 1;
    assert!(check_ack(confirmed, ack, false).is_err());

    let done = step(8, 68);
    assert_eq!(&done[1..9], &68_u64.to_be_bytes());
    assert_eq!(&done[9..65], &[0; 56]);
}
