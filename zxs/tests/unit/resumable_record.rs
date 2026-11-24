use super::*;

fn prepared() -> Record {
    Record {
        data_token: [0x51; 32],
        intent: Intent {
            scope: Scope {
                pool: [0x11; 16],
                epoch: [0x12; 16],
            },
            mutation: Mutation {
                container: [0x21; 16],
                request: [0x22; 16],
            },
            node: [0x23; 16],
            body: Body {
                length: CHUNK + 3,
                sha256: [0x41; 32],
            },
            target: Target::Branch {
                name: "31".repeat(16),
                expected_head: [0x24; 16],
            },
        },
        phase: Phase::Preparing,
    }
}

fn ready() -> Record {
    Record {
        phase: Phase::Ready {
            directory: Birth {
                inode: 0x0102_0304_0506_0708,
                seconds: -5,
                nanos: 3,
            },
            payload: Birth {
                inode: 0x1112_1314_1516_1718,
                seconds: 123_456_789,
                nanos: 987_654_321,
            },
            offset: CHUNK,
        },
        ..prepared()
    }
}

fn exact_checkpoint(raw: &[u8]) -> String {
    hex::<32>(raw[raw.len() - 32..].try_into().unwrap())
}

fn resign(raw: &mut Vec<u8>) {
    raw.truncate(raw.len() - 32);
    let checksum = digest(raw).unwrap();
    raw.extend_from_slice(&checksum);
}

#[test]
fn ur1_proof_and_id_match_independent_byte_vectors() {
    let plan = prepared();
    let selected = ready();
    let before = plan.encode().unwrap();
    let after = selected.encode().unwrap();
    assert_eq!(before.len(), 271);
    assert_eq!(after.len(), 271);
    assert_eq!(&before[..5], b"UR1\0\0");
    assert!(before[37..85].iter().all(|&b| b == 0));
    assert_eq!(&after[..5], b"UR1\0\x01");
    assert_eq!(&after[5..37], &[0x51; 32]);
    assert_eq!(&after[37..45], &0x0102_0304_0506_0708_u64.to_be_bytes());
    assert_eq!(&after[45..53], &(-5_i64).to_be_bytes());
    assert_eq!(&after[53..57], &3_u32.to_be_bytes());
    assert_eq!(&after[77..85], &CHUNK.to_be_bytes());
    assert_eq!(
        exact_checkpoint(&before),
        "5bc0e33ec722f86b5a09290c558c6a2a3182743330d6d0bf31d44d3c0fb07725"
    );
    assert_eq!(
        exact_checkpoint(&after),
        "caa86597ad4213f0371041473bc872edf6eef77a73126fb42f80bc12ab3d0663"
    );
    assert_eq!(
        hex(selected.intent.upload_id().unwrap()),
        "73f789081ed26ead18edea06d5d5e55ee7f1a4078ab9979d22138c0a32365953"
    );
    assert_eq!(
        selected.intent.upload_id().unwrap(),
        plan.intent.upload_id().unwrap()
    );
    assert_eq!(Record::decode(&before).unwrap(), plan);
    assert_eq!(Record::decode(&after).unwrap(), selected);
}

#[test]
fn forged_metadata_with_an_updated_checksum_does_not_become_authority() {
    let canonical = ready().encode().unwrap();
    for (begin, replacement) in [
        (0, vec![b'A']),
        (4, vec![2]),
        (4, vec![0]),
        (5, vec![0; 32]),
        (37, vec![0; 8]),
        (45, vec![0; 12]),
        (53, 1_000_000_000_u32.to_be_bytes().to_vec()),
        (77, (CHUNK + 4).to_be_bytes().to_vec()),
        (77, 1_u64.to_be_bytes().to_vec()),
        (77, u64::MAX.to_be_bytes().to_vec()),
        (85, vec![0; 16]),
        (165, (MAX_BODY + 1).to_be_bytes().to_vec()),
        (205, vec![3]),
        (206, vec![7]),
    ] {
        let mut changed = canonical.clone();
        changed[begin..begin + replacement.len()].copy_from_slice(&replacement);
        resign(&mut changed);
        assert!(Record::decode(&changed).is_err(), "bytes at offset {begin}");
    }
    for length in 0..canonical.len() {
        assert!(Record::decode(&canonical[..length]).is_err());
    }
    let mut trailing = canonical.clone();
    trailing.insert(trailing.len() - 32, 0);
    resign(&mut trailing);
    assert!(Record::decode(&trailing).is_err());

    let mut changed = canonical;
    changed[154] ^= 1;
    assert!(Record::decode(&changed).is_err());
}

#[test]
fn binding_includes_original_epoch_body_and_target_but_not_live_media() {
    let candidate = ready();
    let id = candidate.intent.upload_id().unwrap();
    let mut revised = candidate.clone();
    revised.data_token = [0x52; 32];
    let Phase::Ready {
        payload,
        offset,
        mut directory,
    } = revised.phase
    else {
        unreachable!()
    };
    directory.nanos += 1;
    revised.phase = Phase::Ready {
        directory,
        payload,
        offset,
    };
    assert_eq!(revised.intent.upload_id().unwrap(), id);
    assert_ne!(revised, candidate);

    let mut intent = candidate.intent.clone();
    intent.scope.epoch = [0x13; 16];
    assert_ne!(intent.upload_id().unwrap(), id);

    let mut intent = candidate.intent.clone();
    intent.body.sha256[0] ^= 1;
    assert_ne!(intent.upload_id().unwrap(), id);

    let mut intent = candidate.intent;
    intent.target = Target::HistoricalMain {
        new_branch: "31".repeat(16),
        original: [0x24; 16],
    };
    assert_ne!(intent.upload_id().unwrap(), id);
}

#[test]
fn zero_body_and_maximum_utf8_name_require_exact_original_intent() {
    let mut bounded = prepared();
    bounded.intent.body.length = 0;
    assert!(bounded.encode().is_err());
    bounded.intent.body.sha256 = EMPTY_SHA;
    bounded.intent.target = Target::FirstMain {
        name: "文".repeat(85),
    };
    let want = bounded.encode().unwrap();
    assert_eq!(want.len(), 494);
    assert_eq!(Record::decode(&want).unwrap(), bounded);
    let Target::FirstMain { name } = &mut bounded.intent.target else {
        unreachable!()
    };
    name.push('x');
    assert!(bounded.encode().is_err());
    assert!(bounded.intent.upload_id().is_err());
}

#[test]
fn only_the_next_unconfirmed_block_may_have_a_raw_tail() {
    let total = CHUNK * 2 + 3;
    for len in [0, 1, CHUNK] {
        assert!(valid_raw_extent(total, 0, len));
    }
    assert!(!valid_raw_extent(total, 0, CHUNK + 1));
    assert!(!valid_raw_extent(total, CHUNK, CHUNK - 1));
    assert!(valid_raw_extent(total, CHUNK, CHUNK + 99));
    assert!(valid_raw_extent(total, CHUNK * 2, total));
    assert!(!valid_raw_extent(total, CHUNK, total));
    assert!(valid_raw_extent(total, total, total));
    assert!(!valid_raw_extent(total, total, total + 1));
    assert!(!valid_raw_extent(total, u64::MAX, u64::MAX));
    assert!(valid_raw_extent(0, 0, 0));
    assert!(!valid_raw_extent(0, 0, 1));
}

#[test]
fn chunks_are_exact_sequential_intervals_or_complete_older_intervals() {
    use ChunkUse::{AppendTo, VerifyPrior};

    let total = CHUNK * 2 + 3;

    assert_eq!(chunk_use(total, 0, 0, CHUNK), Some(AppendTo(CHUNK)));
    assert_eq!(chunk_use(total, 0, CHUNK, CHUNK), None);
    assert_eq!(chunk_use(total, CHUNK, 0, CHUNK), Some(VerifyPrior));
    assert_eq!(chunk_use(total, CHUNK, 0, CHUNK - 1), None);
    assert_eq!(chunk_use(total, CHUNK, CHUNK - 1, CHUNK), None);
    assert_eq!(
        chunk_use(total, CHUNK, CHUNK, CHUNK),
        Some(AppendTo(CHUNK * 2))
    );
    assert_eq!(
        chunk_use(total, CHUNK * 2, CHUNK * 2, 3),
        Some(AppendTo(total))
    );
    assert_eq!(chunk_use(total, total, CHUNK * 2, 3), Some(VerifyPrior));
    assert_eq!(chunk_use(total, total, CHUNK * 2, 2), None);
    assert_eq!(chunk_use(total, total, total, 0), None);
    assert_eq!(chunk_use(total, CHUNK, u64::MAX, CHUNK), None);
    assert_eq!(chunk_use(total, u64::MAX, 0, CHUNK), None);
    assert_eq!(chunk_use(u64::MAX, 0, 0, CHUNK), None);
    assert_eq!(chunk_use(0, 0, 0, 0), None);
}
