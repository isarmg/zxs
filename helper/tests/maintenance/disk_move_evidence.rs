//! Synthetic private invocation and cut vectors. No worker, IO or on-disk codec.
use crate::path_rules::Kind;
use sha2::{Digest, Sha256};

const ROLE_DOMAIN: &[u8] = b"ZXS disk move live roles v1\0";
const OPERAND_DOMAIN: &[u8] = b"ZXS disk move operand v1\0";
const MAGIC: &[u8; 8] = b"ZXDMV1\0\0";
const ORDER_BYTES: usize = 168;

type Hash = [u8; 32];
type Id = [u8; 16];

fn live_binding(descriptor: Hash, roles: &[[u64; 3]]) -> Option<Hash> {
    let count = u8::try_from(roles.len()).ok()?;
    if !(3..=64).contains(&count) {
        return None;
    }
    let mut hash = Sha256::new();
    hash.update(ROLE_DOMAIN);
    hash.update(descriptor);
    hash.update([count]);
    for &[dev, ino, mount] in roles {
        if ino == 0 || mount == 0 {
            return None;
        }
        for word in [dev, ino, mount] {
            hash.update(word.to_be_bytes());
        }
    }
    Some(hash.finalize().into())
}

// The lexical bytes are a comparison input, never a filesystem capability.
// A live move must uniquely match two different enrolled held DATA roots.
fn operand(raw: &[u8]) -> Option<Hash> {
    if raw.len() > 4095 {
        return None;
    }
    Kind::StorageRoot.check_path(raw).ok()?;
    let length = u16::try_from(raw.len()).ok()?;
    let mut hash = Sha256::new();
    hash.update(OPERAND_DOMAIN);
    hash.update(length.to_be_bytes());
    hash.update(raw);
    Some(hash.finalize().into())
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Order {
    binding: Hash,
    scope: Hash,
    cid: Id,
    op: Id,
    from: Hash,
    to: Hash,
}

impl Order {
    fn qualified(self) -> bool {
        self.scope != [0; 32] && self.cid != [0; 16] && self.op != [0; 16] && self.from != self.to
    }

    fn encode(self) -> Option<[u8; ORDER_BYTES]> {
        if !self.qualified() {
            return None;
        }
        let mut out = [0; ORDER_BYTES];
        out[..8].copy_from_slice(MAGIC);
        out[8..40].copy_from_slice(&self.binding);
        out[40..72].copy_from_slice(&self.scope);
        out[72..88].copy_from_slice(&self.cid);
        out[88..104].copy_from_slice(&self.op);
        out[104..136].copy_from_slice(&self.from);
        out[136..168].copy_from_slice(&self.to);
        Some(out)
    }

    fn decode(raw: &[u8], binding: Hash) -> Option<Self> {
        let raw: &[u8; ORDER_BYTES] = raw.try_into().ok()?;
        if &raw[..8] != MAGIC || raw[8..40] != binding {
            return None;
        }
        let decoded = Self {
            binding,
            scope: raw[40..72].try_into().ok()?,
            cid: raw[72..88].try_into().ok()?,
            op: raw[88..104].try_into().ok()?,
            from: raw[104..136].try_into().ok()?,
            to: raw[136..168].try_into().ok()?,
        };
        decoded.qualified().then_some(decoded)
    }
}

const ROLES: [[u64; 3]; 4] = [
    [
        0x0102_0304_0506_0708,
        0x1112_1314_1516_1718,
        0x2122_2324_2526_2728,
    ],
    [0x31, 0x32, 0x33],
    [0x41, 0x42, 0x43],
    [0x51, 0x52, 0x53],
];

fn hex_of(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(out, "{byte:02x}").unwrap();
    }
    out
}

fn fixture() -> Order {
    Order {
        binding: live_binding([0x42; 32], &ROLES).unwrap(),
        scope: [0x24; 32],
        cid: [0x31; 16],
        op: [0x32; 16],
        from: operand(b"/zxs/a").unwrap(),
        to: operand(b"/zxs/\xff").unwrap(),
    }
}

#[test]
fn invocation_bytes_match_independently_hashed_scope_and_operands() {
    let order = fixture();
    assert_eq!(
        hex_of(&order.binding),
        "9e9677fc4c09673a235b3bc2a88197afc6c588a1c4086ba7750c0ad8aedff98c"
    );
    assert_eq!(
        hex_of(&order.from),
        "782d96da9a7c4516df1b1a72dc4982faaa05befc9c5dd71469047253ebb48434"
    );
    assert_eq!(
        hex_of(&order.to),
        "f545a79a125aaa6c1c9d8f4bc81a0d7ff3ae8d521b9a726a1a179d73bf1ee0a6"
    );
    let raw = order.encode().unwrap();
    assert_eq!(
        hex_of(&Sha256::digest(raw)),
        "bdbd276eb6e0aee8b13a896d9e490d847ba61177e51bc9e321387e443eb589d6"
    );
    assert_eq!(Order::decode(&raw, order.binding), Some(order));
}

#[test]
fn invocation_rejects_wrong_binding_frames_and_unqualified_identifiers() {
    let order = fixture();
    let raw = order.encode().unwrap();
    let mut appended = raw.to_vec();
    appended.push(0);
    for invalid in [&raw[..ORDER_BYTES - 1], &appended] {
        assert!(Order::decode(invalid, order.binding).is_none());
    }
    assert!(Order::decode(&raw, [0x44; 32]).is_none());
    for section in [40..72, 72..88, 88..104] {
        let mut wrong = raw;
        wrong[section].fill(0);
        assert!(Order::decode(&wrong, order.binding).is_none());
    }
    let mut wrong = raw;
    wrong[..8].copy_from_slice(b"ZXOVF1\0\0");
    assert!(Order::decode(&wrong, order.binding).is_none());
    let mut wrong = raw;
    wrong[136..168].copy_from_slice(&order.from);
    assert!(Order::decode(&wrong, order.binding).is_none());
    assert!(
        Order {
            to: order.from,
            ..order
        }
        .encode()
        .is_none()
    );
}

#[test]
fn held_role_order_and_exact_native_operand_spellings_are_bound() {
    let order = fixture();
    let mut changed = ROLES;
    changed.swap(0, 1);
    assert_ne!(live_binding([0x42; 32], &changed), Some(order.binding));
    let mut changed = ROLES;
    changed[3][2] ^= 1;
    assert_ne!(live_binding([0x42; 32], &changed), Some(order.binding));
    assert!(live_binding([0x42; 32], &ROLES[..3]).is_some()); // current roster on DONE
    assert!(live_binding([0x42; 32], &ROLES[..2]).is_none());
    assert!(live_binding([0x42; 32], &[[1; 3]; 65]).is_none());
    for field in 1..3 {
        let mut changed = ROLES;
        changed[0][field] = 0;
        assert!(live_binding([0x42; 32], &changed).is_none());
    }
    for bad in [
        &b""[..],
        b"/",
        b"relative",
        b"/x/.",
        b"/x/../y",
        b"/x//y",
        b"/x/",
        b"/x/\0",
    ] {
        assert!(operand(bad).is_none());
    }
    assert_ne!(operand(b"/zxs/a"), operand(b"/zxs/A"));
    let mut oversized = vec![b'a'; 4096];
    oversized[0] = b'/';
    assert!(operand(&oversized).is_none());
}

// This in-memory oracle names proof dependencies for the authority cut; it
// does not read or authorize filesystem actions, nor fix a durable layout.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Route {
    raw: [u8; 52],
    inode: u64,
    birth: (i64, u32),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Evidence<T> {
    Exact(T),
    Absent,
    Unknown,
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Scope {
    Unconfirmed,
    CurrentPool,
    LiveSelectedPair,
}
#[derive(Clone, Copy)]
struct Observed {
    scope: Scope,
    complete_source: bool,
    ready_target: bool,
    guard: Evidence<()>,
    done: Evidence<()>,
    switched: Evidence<()>,
    route: Evidence<Route>,
    candidate: Evidence<Route>,
}

#[derive(Debug, Eq, PartialEq)]
enum Cut {
    Refused,
    ReproveBeforeReplace,
    VisibleNeedsDurability,
    ReproveSurvivingSource,
    HistoricDone,
}

fn classify(old: Route, new: Route, held: Observed) -> Cut {
    if held.scope == Scope::Unconfirmed {
        return Cut::Refused;
    }
    match held.done {
        Evidence::Exact(()) if held.guard != Evidence::Unknown => return Cut::HistoricDone,
        Evidence::Exact(()) | Evidence::Unknown => return Cut::Refused,
        Evidence::Absent => {}
    }
    if held.scope != Scope::LiveSelectedPair
        || !held.ready_target
        || held.guard != Evidence::Exact(())
    {
        return Cut::Refused;
    }
    if held.route == Evidence::Exact(old)
        && held.candidate == Evidence::Exact(new)
        && held.switched == Evidence::Absent
        && held.complete_source
    {
        return Cut::ReproveBeforeReplace;
    }
    if held.route == Evidence::Exact(new) && held.candidate == Evidence::Absent {
        match held.switched {
            Evidence::Exact(()) => return Cut::ReproveSurvivingSource,
            Evidence::Absent if held.complete_source => return Cut::VisibleNeedsDurability,
            Evidence::Absent | Evidence::Unknown => {}
        }
    }
    Cut::Refused
}

fn route(token: Hash, inode: u64) -> Route {
    let mut raw = [0; 52];
    raw[..4].copy_from_slice(b"CR1\0");
    raw[4..20].copy_from_slice(&[0x31; 16]);
    raw[20..].copy_from_slice(&token);
    Route {
        raw,
        inode,
        birth: (23, 8),
    }
}

fn before() -> (Route, Route, Observed) {
    let old = route([1; 32], 11);
    let new = route([2; 32], 12);
    (
        old,
        new,
        Observed {
            scope: Scope::LiveSelectedPair,
            complete_source: true,
            ready_target: true,
            guard: Evidence::Exact(()),
            done: Evidence::Absent,
            switched: Evidence::Absent,
            route: Evidence::Exact(old),
            candidate: Evidence::Exact(new),
        },
    )
}

#[test]
fn route_visibility_is_not_a_durable_switch_or_evidence_to_retire() {
    let (old, new, pre) = before();
    assert_eq!(classify(old, new, pre), Cut::ReproveBeforeReplace);
    for invalid in [
        Observed {
            scope: Scope::CurrentPool,
            ..pre
        },
        Observed {
            switched: Evidence::Unknown,
            ..pre
        },
        Observed {
            guard: Evidence::Unknown,
            ..pre
        },
    ] {
        assert_eq!(classify(old, new, invalid), Cut::Refused);
    }
    let visible = Observed {
        route: Evidence::Exact(new),
        candidate: Evidence::Absent,
        ..pre
    };
    assert_eq!(classify(old, new, visible), Cut::VisibleNeedsDurability);
    assert_eq!(
        classify(
            old,
            new,
            Observed {
                complete_source: false,
                ..visible
            }
        ),
        Cut::Refused
    );
    let switched = Observed {
        switched: Evidence::Exact(()),
        complete_source: false,
        ..visible
    };
    assert_eq!(classify(old, new, switched), Cut::ReproveSurvivingSource);
    for invalid in [
        Observed {
            ready_target: false,
            ..switched
        },
        Observed {
            candidate: Evidence::Unknown,
            ..switched
        },
        Observed {
            route: Evidence::Exact(old),
            ..switched
        },
        Observed {
            route: Evidence::Exact(Route {
                birth: (24, 8),
                ..new
            }),
            ..switched
        },
    ] {
        assert_eq!(classify(old, new, invalid), Cut::Refused);
    }
}

#[test]
fn historic_done_never_requires_the_former_roster_or_target_bytes() {
    let (old, new, pre) = before();
    let historical = Observed {
        done: Evidence::Exact(()),
        guard: Evidence::Absent,
        scope: Scope::CurrentPool,
        complete_source: false,
        ready_target: false,
        route: Evidence::Unknown,
        candidate: Evidence::Unknown,
        ..pre
    };
    assert_eq!(classify(old, new, historical), Cut::HistoricDone);
    assert_eq!(
        classify(
            old,
            new,
            Observed {
                guard: Evidence::Exact(()),
                ..historical
            }
        ),
        Cut::HistoricDone
    );
    for invalid in [
        Observed {
            guard: Evidence::Unknown,
            ..historical
        },
        Observed {
            scope: Scope::Unconfirmed,
            ..historical
        },
        Observed {
            done: Evidence::Unknown,
            ..historical
        },
        Observed {
            done: Evidence::Absent,
            ..historical
        },
    ] {
        assert_eq!(classify(old, new, invalid), Cut::Refused);
    }
}
