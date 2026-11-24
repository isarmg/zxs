//! Immutable byte vectors only: no directories, files, workers, or syscalls.
use super::{
    proof::{Expect, Header, LedgerHash, Proof, Row, Side, Stamp},
    *,
};
use crate::container_store::{Body, Change, Mutation, Receipt, State, Target, codec, hex};

const SCOPE: Scope = Scope {
    pool: [0x11; 16],
    container: [0x21; 16],
    operation: [0x31; 16],
};
const SYSTEM: Digest = [0x51; 32];
const SOURCE: Digest = [0x52; 32];
const TARGET: Digest = [0x53; 32];

fn birth(inode: u32) -> Birth {
    Birth {
        inode: u64::from(inode),
        seconds: 1_700_000_000 + i64::from(inode),
        nanos: inode * 11,
    }
}

fn receipts() -> [Receipt; 2] {
    [
        Receipt {
            mutation: Mutation {
                container: SCOPE.container,
                request: [0x41; 16],
            },
            sequence: 1,
            change: Change::Save {
                target: Target::FirstMain {
                    name: String::from("alpha"),
                },
                node: [0x71; 16],
                body: Body {
                    length: 0,
                    sha256: digest(b"").unwrap(),
                },
            },
        },
        Receipt {
            mutation: Mutation {
                container: SCOPE.container,
                request: [0x42; 16],
            },
            sequence: 2,
            change: Change::Rename {
                expected_sequence: 1,
                name: String::from("beta"),
            },
        },
    ]
}

fn ledger() -> Digest {
    let mut proof = LedgerHash::new(SCOPE.container, 2).unwrap();
    let mut state = State::default();
    for row in receipts() {
        state.recover(row.clone()).unwrap();
        proof.push(&row).unwrap();
    }
    proof.finish().unwrap()
}

fn stamp(inode: u64, len: u64) -> Stamp {
    Stamp {
        inode,
        len,
        mtime: (-2, 111),
        ctime: (1_700_000_010, 333),
    }
}

fn proof_bytes(side: Side, token: Digest, root: u64, ledger: Digest) -> Vec<u8> {
    let [first, second] = receipts();
    let one = Row {
        sequence: 1,
        operation: first.mutation.request,
        dir_inode: root + 1,
        receipt: stamp(root + 2, codec::encode(&first).unwrap().len() as u64),
        payload: Some(stamp(root + 3, 0)),
    };
    let two = Row {
        sequence: 2,
        operation: second.mutation.request,
        dir_inode: root + 4,
        receipt: stamp(root + 5, codec::encode(&second).unwrap().len() as u64),
        payload: None,
    };
    let header = Header::new(side, SCOPE, token, root, 2, ledger).unwrap();
    let mut raw = header.encode().unwrap().to_vec();
    raw.extend_from_slice(&one.encode().unwrap());
    raw.extend_from_slice(&two.encode().unwrap());
    raw
}

struct Fixture {
    intent: [u8; INTENT_BYTES],
    guard: [u8; GUARD_BYTES],
    ready: [u8; READY_BYTES],
    switched: [u8; CUT_BYTES],
    done: [u8; CUT_BYTES],
    source_proof: Vec<u8>,
    target_proof: Vec<u8>,
}

impl Fixture {
    fn form() -> Self {
        let ledger = ledger();
        let source_proof = proof_bytes(Side::Source, SOURCE, 0x100, ledger);
        let target_proof = proof_bytes(Side::Target, TARGET, 0x200, ledger);
        let intent = Intent {
            scope: SCOPE,
            system: SYSTEM,
            source: SOURCE,
            target: TARGET,
            source_operand: [0x61; 32],
            target_operand: [0x62; 32],
            state_parent: birth(11),
            routes_parent: birth(12),
            journal: birth(13),
            old_route: route_record(SCOPE.container, SOURCE).unwrap(),
            old_birth: birth(14),
            source_inode: 0x100,
            rows: 2,
            source_proof: digest(&source_proof).unwrap(),
            ledger,
        };
        let intent = intent.encode().unwrap();
        let i = Intent::decode(&intent).unwrap();
        let guard = Guard { birth: birth(15) }.encode(&i).unwrap();
        let ready = Ready {
            guard: birth(15),
            target_inode: 0x200,
            new_route: route_record(SCOPE.container, TARGET).unwrap(),
            new_birth: birth(16),
            target_proof: digest(&target_proof).unwrap(),
        }
        .encode(&i)
        .unwrap();
        let r = Ready::decode(&ready, &i).unwrap();
        let switched = Switched::encode(&r).unwrap();
        let done = encode_done(&Switched::decode(&switched, &r).unwrap()).unwrap();
        Self {
            intent,
            guard,
            ready,
            switched,
            done,
            source_proof,
            target_proof,
        }
    }

    fn complete(&self) -> JournalBytes<'_> {
        JournalBytes {
            intent: &self.intent,
            guard: Some(&self.guard),
            ready: Some(&self.ready),
            switched: Some(&self.switched),
            done: Some(&self.done),
        }
    }
}

fn checksum(raw: &mut [u8]) {
    let len = raw.len();
    let (fields, footer) = raw.split_at_mut(len - 32);
    footer.copy_from_slice(&digest(fields).unwrap());
}

#[test]
fn immutable_records_and_native_sidecars_have_independent_known_digests() {
    let f = Fixture::form();
    let selected = Journal::decode(f.complete()).unwrap();
    let i = selected.intent();
    let g = selected.guard().unwrap();
    let r = selected.ready().unwrap();
    assert_eq!(i.value().scope, SCOPE);
    assert_eq!(i.value().old_route[..4], *b"CR1\0");
    assert_eq!(r.value().new_route[20..], TARGET);
    assert_eq!(g.value().birth, birth(15));
    assert_eq!(
        hex(i.id()),
        "f2cab6327971feba0720cd15348f6c0735c0705655e7c2983b820d07a48d9d22"
    );
    assert_eq!(
        hex(g.id()),
        "cdcdbaead199adb10668619b7d34b39d5bebbc6673371deceb202d373a79e3f9"
    );
    assert_eq!(
        hex(r.id()),
        "302b430431f761e7fde17e8decb1b6153ca66625577085d0b25473a2f7526886"
    );
    assert_eq!(
        hex(i.value().ledger),
        "bc077e6684d2268ccbb8f9fe1f19d2e1d8300063c43c277591a5c5cec1d81269"
    );
    assert_eq!(
        hex(digest(&f.source_proof).unwrap()),
        "1d822352bc03a7d2a09ba991954263b8a5cc584f6140e58026e0af383be101ac"
    );
    assert_eq!(
        hex(digest(&f.target_proof).unwrap()),
        "72603f515ff8fc21942b488966ebbe558338d911129ebd415499284616fb812e"
    );
    if let Phase::Done(id) = selected.phase() {
        assert_eq!(
            hex(*id),
            "37cb04576600da71bca6a5b75d90049c470b19d2d2b419adaafca068da004793"
        );
    } else {
        panic!("complete exact record chain did not reach historical DONE");
    }
    assert_eq!(
        hex(Switched::decode(&f.switched, r).unwrap().0),
        "b390a433956dc8b2bb213ee08e37d9b75ea267b4f869357e4ac988f4f50b581d"
    );
}

#[test]
fn incomplete_or_foreign_guard_and_gapped_phases_are_not_recovered() {
    let f = Fixture::form();
    let mut raw = f.complete();
    raw.guard = None;
    assert!(matches!(
        Journal::decode(raw).unwrap().phase(),
        Phase::Done(_)
    ));

    let raw = JournalBytes {
        guard: None,
        done: None,
        ..f.complete()
    };
    assert!(Journal::decode(raw).is_err());
    let raw = JournalBytes {
        ready: None,
        ..f.complete()
    };
    assert!(Journal::decode(raw).is_err());
    let raw = JournalBytes {
        switched: None,
        ..f.complete()
    };
    assert!(Journal::decode(raw).is_err());

    let i = Intent::decode(&f.intent).unwrap();
    let replacement = Guard { birth: birth(19) }.encode(&i).unwrap();
    let raw = JournalBytes {
        guard: Some(&replacement),
        ..f.complete()
    };
    assert!(Journal::decode(raw).is_err());
    let raw = JournalBytes {
        guard: Some(&f.guard[..1]),
        ..f.complete()
    };
    assert!(Journal::decode(raw).is_err());

    let raw = JournalBytes {
        done: None,
        ..f.complete()
    };
    assert!(
        matches!(Journal::decode(raw).unwrap().phase(), Phase::Switched(s) if s.0 == f.switched[40..])
    );
    let raw = JournalBytes {
        switched: None,
        done: None,
        ..f.complete()
    };
    assert!(matches!(
        Journal::decode(raw).unwrap().phase(),
        Phase::Ready
    ));
    let raw = JournalBytes {
        ready: None,
        switched: None,
        done: None,
        ..f.complete()
    };
    assert!(matches!(
        Journal::decode(raw).unwrap().phase(),
        Phase::Guarded
    ));
    let raw = JournalBytes {
        guard: None,
        ready: None,
        switched: None,
        done: None,
        ..f.complete()
    };
    assert!(matches!(
        Journal::decode(raw).unwrap().phase(),
        Phase::Intent
    ));
}

#[test]
fn exact_extent_and_authority_are_checked_even_with_a_consistent_checksum() {
    let f = Fixture::form();
    assert!(Intent::decode(&f.intent[..INTENT_BYTES - 1]).is_err());
    let mut trailing = f.intent.to_vec();
    trailing.push(0);
    assert!(Intent::decode(&trailing).is_err());
    let mut bad = f.intent;
    bad[296] ^= 1; // old CR1 DATA token
    checksum(&mut bad);
    assert!(Intent::decode(&bad).is_err());

    let mut bad = f.intent;
    bad[356..364].copy_from_slice(&(MAX_ROWS + 1).to_be_bytes());
    checksum(&mut bad);
    assert!(Intent::decode(&bad).is_err());

    let i = Intent::decode(&f.intent).unwrap();
    let mut alias = f.ready;
    alias[152..160].copy_from_slice(&i.value().old_birth.inode.to_be_bytes());
    checksum(&mut alias);
    assert!(Ready::decode(&alias, &i).is_err());

    let mut wrong_guard = f.ready;
    wrong_guard[60] ^= 1;
    checksum(&mut wrong_guard);
    assert!(Ready::decode(&wrong_guard, &i).is_err());

    let mut wrong_done = f.done;
    wrong_done[8] ^= 1;
    checksum(&mut wrong_done);
    let raw = JournalBytes {
        done: Some(&wrong_done),
        ..f.complete()
    };
    assert!(Journal::decode(raw).is_err());
}

#[test]
fn both_proofs_require_whole_extent_exact_header_and_matching_ledger_row() {
    let fixture = Fixture::form();
    let journal = Journal::decode(fixture.complete()).unwrap();
    let source = Expect::source(journal.intent());
    let target = Expect::target(&journal).unwrap();
    assert_eq!(source.extent().unwrap(), 400);
    assert_eq!(target.extent().unwrap(), 400);
    let sp = Proof::decode(&fixture.source_proof, &source).unwrap();
    let tp = Proof::decode(&fixture.target_proof, &target).unwrap();
    let [mut first, second] = receipts();
    assert_eq!(
        sp.for_receipt(&first).unwrap().payload,
        Some(stamp(0x103, 0))
    );
    assert_eq!(tp.for_receipt(&first).unwrap().dir_inode, 0x201);
    assert_eq!(sp.for_receipt(&second).unwrap().payload, None);
    first.mutation.request = [0x43; 16];
    assert!(sp.for_receipt(&first).is_err());
    assert!(Proof::decode(&fixture.target_proof, &source).is_err());
    assert!(Proof::decode(&fixture.source_proof[..399], &source).is_err());
    let mut longer = fixture.source_proof.clone();
    longer.push(0);
    assert!(Proof::decode(&longer, &source).is_err());
    let mut structurally_invalid = fixture.source_proof;
    structurally_invalid[144..152].copy_from_slice(&2_u64.to_be_bytes());
    let mut intent = journal.intent().value().clone();
    intent.source_proof = digest(&structurally_invalid).unwrap();
    let source = Expect::source(&Intent::decode(&intent.encode().unwrap()).unwrap());
    assert!(Proof::decode(&structurally_invalid, &source).is_err());
}

#[test]
fn ledger_scope_order_and_count_are_bounded_without_accepting_a_prefix() {
    let [first, second] = receipts();
    assert!(LedgerHash::new(SCOPE.container, 0).is_err());
    assert!(LedgerHash::new(SCOPE.container, MAX_ROWS + 1).is_err());
    let mut partial = LedgerHash::new(SCOPE.container, 2).unwrap();
    assert!(partial.push(&second).is_err());
    partial.push(&first).unwrap();
    assert!(partial.finish().is_err());

    let mut different = LedgerHash::new([0x68; 16], 2).unwrap();
    assert!(different.push(&first).is_err());

    let f = Fixture::form();
    let mut i = Intent::decode(&f.intent).unwrap().value().clone();
    i.rows = MAX_ROWS;
    let cap = Expect::source(&Intent::decode(&i.encode().unwrap()).unwrap());
    assert_eq!(cap.extent().unwrap(), 8_388_752);
}
