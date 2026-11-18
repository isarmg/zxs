use super::*;
use crate::files::lifecycle::tests::{id, page, target};
use storage_contract::file_lifecycle::Interval;

fn n(i: u64) -> FileNodeId { id("ver",i).parse().unwrap() }
fn node(i: u64, parents: &[u64]) -> IntervalNode {
    IntervalNode { node: n(i), parents: parents.iter().copied().map(n).collect() }
}
fn tip(i: u64, at: u64) -> BranchTip {
    BranchTip { branch: id("br",i).parse().unwrap(), node: n(at) }
}

#[test]
fn a_shared_ancestor_a_short_branch_and_a_real_merge_keep_their_longest_parent_ranks() {
    // 91 is a *proven outside parent*, not a new card in this section.
    let nodes = [node(5,&[3,4]), node(3,&[2]), node(4,&[1]), node(1,&[91]), node(2,&[1])];
    let tips = [tip(1,5), tip(2,4), tip(3,4)];
    let g = ranked(&nodes, &tips).unwrap();
    assert_eq!(g.rows, vec![vec![n(5)], vec![n(3)], vec![n(2),n(4)], vec![n(1)]]);
    assert_eq!(g.edges, vec![(n(1),n(2)),(n(1),n(4)),(n(2),n(3)),(n(3),n(5)),(n(4),n(5))]);
    assert_eq!(g.tips[&n(4)].len(), 2);
    assert_eq!(g.outside[&n(1)], vec![n(91)]);
    assert!(!g.edges.iter().any(|e|e.0 == n(91)));
    let mut shuffled = nodes.to_vec(); shuffled.reverse();
    assert_eq!(g, ranked(&shuffled, &tips).unwrap());
    let unequal = ranked(&[node(1,&[]),node(2,&[1]),node(3,&[2]),node(8,&[])],
                         &[tip(1,3),tip(2,8)]).unwrap();
    assert_eq!(unequal.rows, vec![vec![n(3)],vec![n(2)],vec![n(1),n(8)]]);
}

#[test]
fn only_two_adjacent_original_events_can_close_even_an_empty_interval() {
    let mut p = page(&target(),1,3,false);
    let a = p.events[0].event_key();
    let b = p.events[1].event_key();
    let c = p.events[2].event_key();
    p.intervals = vec![Interval { older: a.clone(), newer: c.clone(),
        body: IntervalBody::Complete { nodes: vec![node(1,&[])], branches: vec![tip(1,1)] } }];
    assert_eq!(below_in(&p,0), Section::OutsidePage);
    assert_eq!(below_in(&p,1), Section::Open);
    assert_eq!(below_in(&p,2), Section::Open);
    p.intervals.push(Interval { older: b.clone(), newer: c, body: IntervalBody::Complete { nodes: vec![], branches: vec![] } });
    assert!(matches!(below_in(&p,2), Section::Complete { graph, .. } if graph.rows.is_empty() && graph.edges.is_empty()));
    let valid = p.intervals.last().unwrap().clone();
    p.intervals.push(valid);
    assert_eq!(below_in(&p,2), Section::Open);
    p.events.remove(0);
    assert_eq!(below_in(&p,0), Section::OutsidePage);
    assert!(ranked(&[node(1,&[2]),node(2,&[1])], &[]).is_none());
    assert!(ranked(&[node(1,&[]),node(1,&[])], &[]).is_none());
    assert!(ranked(&[node(1,&[9,9])], &[]).is_none());
    assert!(ranked(&[node(1,&[])], &[tip(1,9)]).is_none());
    assert!(ranked(&[node(1,&[])], &[tip(1,1),tip(1,1)]).is_none());
}
