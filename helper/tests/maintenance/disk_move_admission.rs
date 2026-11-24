//! Fresh-candidate oracle in the test-only move module. Neither this gate nor
//! its input fixtures open a path, parse store records, or authorize a write.

use super::{Evidence, Hash, Id, Order, ROLES, fixture, live_binding, operand};

#[derive(Clone, Copy, Eq, PartialEq)]
enum Role {
    System,
    Recycle,
    InitializedData,
    MarkerOnlyData,
}

#[derive(Clone, Copy)]
struct HeldRole<'a> {
    role: Role,
    path: &'a [u8],
    token: Hash,
    live: [u64; 3],
}

#[derive(Clone, Copy)]
struct Held<'a> {
    // Exact means original offline EX or protected original ELF respectively,
    // already proved from actual held objects, not from a passed-in name.
    lease: Evidence<()>,
    executable: Evidence<()>,
    descriptor_sha: Hash,
    roster: &'a [HeldRole<'a>],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Pair {
    source: Hash,
    target: Hash,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Refusal {
    Custody,
    Roster,
    Binding,
    Operands,
    Route,
    Occupied,
    Retirement,
    Upload,
    Recycle,
}

fn pair(order: Order, held: Held<'_>) -> Result<Pair, Refusal> {
    if held.lease != Evidence::Exact(()) || held.executable != Evidence::Exact(()) {
        return Err(Refusal::Custody);
    }
    if !(4..=64).contains(&held.roster.len()) {
        return Err(Refusal::Roster);
    }
    let mut live = [[0; 3]; 64];
    let (mut source, mut target) = (None, None);
    for (index, role) in held.roster.iter().enumerate() {
        if !matches!(
            (index, role.role),
            (0, Role::System)
                | (1, Role::Recycle)
                | (2.., Role::InitializedData | Role::MarkerOnlyData)
        ) || role.token == [0; 32]
            || held.roster[..index].iter().any(|r| r.token == role.token)
        {
            return Err(Refusal::Roster);
        }
        live[index] = role.live;
        let name = operand(role.path).ok_or(Refusal::Roster)?;
        if index >= 2 {
            let matched = if name == order.from {
                &mut source
            } else if name == order.to {
                &mut target
            } else {
                continue;
            };
            if matched.replace(role).is_some() || role.role != Role::InitializedData {
                return Err(Refusal::Operands);
            }
        }
    }
    if !order.qualified()
        || live_binding(held.descriptor_sha, &live[..held.roster.len()]) != Some(order.binding)
    {
        return Err(Refusal::Binding);
    }
    let (source, target) = source.zip(target).ok_or(Refusal::Operands)?;
    if source.live[0] == target.live[0] || source.live[2] == target.live[2] {
        return Err(Refusal::Operands);
    }
    Ok(Pair {
        source: source.token,
        target: target.token,
    })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct SelectedRoute {
    cid: Id,
    data_token: Hash,
}

#[derive(Clone, Copy)]
struct CoreFacts {
    // Exact requires Core to reprove the original EX, held roster and this
    // request across its observation. This is not a returned frame layout.
    cut: Evidence<Order>,
    route: Evidence<SelectedRoute>,
    // Active interlock, same-op journal, candidate CR1, destination data and
    // destination staging. This is fresh admission only, not same-OP recovery.
    vacancies: [Evidence<()>; 5],
    // Each selected DATA token: DT1 final and .token.incoming.
    dt1: [[Evidence<()>; 2]; 2],
    // Exact CID control record, then slots -00.incoming through -0f.incoming.
    ur1: [Evidence<()>; 17],
    // Same CID selected, arrived (including unselected), and pending R.
    // Pending is absent only after a qualified metadata-only absence proof.
    recycle: [Evidence<()>; 3],
}

fn absent<'a>(items: impl IntoIterator<Item = &'a Evidence<()>>) -> bool {
    items.into_iter().all(|seen| *seen == Evidence::Absent)
}

fn admit(
    order: Order,
    held: Held<'_>,
    core: impl FnOnce(Order, Pair) -> CoreFacts,
) -> Result<Pair, Refusal> {
    let moved = pair(order, held)?;
    // Represents the independently held Core role/metadata cut. Keep this
    // downstream of Helper's local roster gate; no borrowed or raw names.
    let facts = core(order, moved);
    if facts.cut != Evidence::Exact(order) {
        return Err(Refusal::Binding);
    }
    if facts.route
        != Evidence::Exact(SelectedRoute {
            cid: order.cid,
            data_token: moved.source,
        })
    {
        return Err(Refusal::Route);
    }
    if !absent(&facts.vacancies) {
        return Err(Refusal::Occupied);
    }
    if !absent(facts.dt1.iter().flatten()) {
        return Err(Refusal::Retirement);
    }
    if !absent(&facts.ur1) {
        return Err(Refusal::Upload);
    }
    if !absent(&facts.recycle) {
        return Err(Refusal::Recycle);
    }
    Ok(moved)
}

const CHOSEN: Pair = Pair {
    source: [3; 32],
    target: [4; 32],
};
const ROSTER: [HeldRole<'static>; 4] = [
    HeldRole {
        role: Role::System,
        path: b"/zxs/sys",
        token: [1; 32],
        live: ROLES[0],
    },
    HeldRole {
        role: Role::Recycle,
        path: b"/zxs/r",
        token: [2; 32],
        live: ROLES[1],
    },
    HeldRole {
        role: Role::InitializedData,
        path: b"/zxs/a",
        token: CHOSEN.source,
        live: ROLES[2],
    },
    HeldRole {
        role: Role::InitializedData,
        path: b"/zxs/\xff",
        token: CHOSEN.target,
        live: ROLES[3],
    },
];
const HELD: Held<'static> = Held {
    lease: Evidence::Exact(()),
    executable: Evidence::Exact(()),
    descriptor_sha: [0x42; 32],
    roster: &ROSTER,
};
fn cleared(order: Order) -> CoreFacts {
    CoreFacts {
        cut: Evidence::Exact(order),
        route: Evidence::Exact(SelectedRoute {
            cid: order.cid,
            data_token: CHOSEN.source,
        }),
        vacancies: [Evidence::Absent; 5],
        dt1: [[Evidence::Absent; 2]; 2],
        ur1: [Evidence::Absent; 17],
        recycle: [Evidence::Absent; 3],
    }
}

#[test]
fn only_bound_offline_selection_may_consult_storage_evidence() {
    let order = fixture();
    assert_eq!(
        admit(order, HELD, |request, pair| {
            assert_eq!(request, order);
            assert_eq!(pair, CHOSEN);
            cleared(order)
        }),
        Ok(CHOSEN)
    );
    for invalid in [
        Held {
            lease: Evidence::Unknown,
            ..HELD
        },
        Held {
            executable: Evidence::Unknown,
            ..HELD
        },
        Held {
            descriptor_sha: [0x45; 32],
            ..HELD
        },
        Held {
            roster: &ROSTER[..3],
            ..HELD
        },
    ] {
        assert!(
            admit(order, invalid, |_, _| panic!(
                "unproved scope consulted Core"
            ))
            .is_err()
        );
    }
    let wrong = Order {
        from: operand(ROSTER[1].path).unwrap(),
        ..order
    };
    assert_eq!(
        admit(wrong, HELD, |_, _| panic!("RECYCLE operand consulted Core")),
        Err(Refusal::Operands)
    );
    let mut roles = ROSTER;
    roles.swap(0, 1);
    assert_eq!(
        pair(
            order,
            Held {
                roster: &roles,
                ..HELD
            }
        ),
        Err(Refusal::Roster)
    );
    let mut roles = ROSTER;
    roles[2].role = Role::MarkerOnlyData;
    assert_eq!(
        pair(
            order,
            Held {
                roster: &roles,
                ..HELD
            }
        ),
        Err(Refusal::Operands)
    );
    let mut roles = ROSTER;
    roles[3].path = roles[2].path;
    assert_eq!(
        pair(
            order,
            Held {
                roster: &roles,
                ..HELD
            }
        ),
        Err(Refusal::Operands)
    );
    let mut roles = ROSTER;
    roles[3].live[0] = roles[2].live[0];
    let mut coordinates = ROLES;
    coordinates[3] = roles[3].live;
    let new_order = Order {
        binding: live_binding(HELD.descriptor_sha, &coordinates).unwrap(),
        ..order
    };
    assert_eq!(
        pair(
            new_order,
            Held {
                roster: &roles,
                ..HELD
            }
        ),
        Err(Refusal::Operands)
    );
}

#[test]
fn missing_alternate_route_or_unproved_occupancy_is_not_fresh_admission() {
    let order = fixture();
    let ok = cleared(order);
    for cut in [
        Evidence::Unknown,
        Evidence::Exact(Order {
            scope: [0x99; 32],
            ..order
        }),
        Evidence::Exact(Order {
            op: [0x44; 16],
            ..order
        }),
    ] {
        assert_eq!(
            admit(order, HELD, |_, _| CoreFacts { cut, ..ok }),
            Err(Refusal::Binding)
        );
    }
    for route in [
        Evidence::Absent,
        Evidence::Unknown,
        Evidence::Exact(SelectedRoute {
            cid: order.cid,
            data_token: CHOSEN.target,
        }),
        Evidence::Exact(SelectedRoute {
            cid: [9; 16],
            data_token: CHOSEN.source,
        }),
    ] {
        assert_eq!(
            admit(order, HELD, |_, _| CoreFacts { route, ..ok }),
            Err(Refusal::Route)
        );
    }
    for presence in [Evidence::Exact(()), Evidence::Unknown] {
        for slot in 0..ok.vacancies.len() {
            let mut stopped = ok;
            stopped.vacancies[slot] = presence;
            assert_eq!(admit(order, HELD, |_, _| stopped), Err(Refusal::Occupied));
        }
    }
}

#[test]
fn every_scoped_pending_or_unreadable_operation_blocks_admission() {
    let order = fixture();
    let ok = cleared(order);
    for presence in [Evidence::Exact(()), Evidence::Unknown] {
        for disk in 0..2 {
            for slot in 0..2 {
                let mut stopped = ok;
                stopped.dt1[disk][slot] = presence;
                assert_eq!(admit(order, HELD, |_, _| stopped), Err(Refusal::Retirement));
            }
        }
        for slot in 0..ok.ur1.len() {
            let mut stopped = ok;
            stopped.ur1[slot] = presence;
            assert_eq!(admit(order, HELD, |_, _| stopped), Err(Refusal::Upload));
        }
        for slot in 0..ok.recycle.len() {
            let mut stopped = ok;
            stopped.recycle[slot] = presence;
            assert_eq!(admit(order, HELD, |_, _| stopped), Err(Refusal::Recycle));
        }
    }
}
