use super::*;
use crate::files::lifecycle::tests::{id, page, path_event, target};
use storage_contract::file_lifecycle::{Interval, IntervalNode, NodeLink};

fn n(i: u64) -> FileNodeId { id("ver",i).parse().unwrap() }

#[test]
fn missing_borrowed_or_replaced_event_relations_cannot_become_an_anchor() {
    let target = target();
    let req = target.request(None).unwrap();
    let mut page = page(&target,1,1,false);
    let mut event = path_event(2,7);
    event.references = References::Complete { links: vec![
        NodeLink { role: LinkRole::Before, node: None },
        NodeLink { role: LinkRole::After, node: Some(n(2)) },
    ]};
    let proof = Proof::Event { event: event.event_key(), role: LinkRole::After, node:n(2) };
    page.events = vec![event.clone()];
    let mut model = Model::default(); model.set_target(Some(target.clone()));
    let g = model.begin(); model.complete(&target,g,&req,Ok(page.clone()));
    let original = Origin::capture(&model).unwrap();
    assert!(proof.present(&page) && original.readback(&page, &proof).is_ok());
    assert!(!Proof::Event {event:event.event_key(),role:LinkRole::Before,node:n(2)}.present(&page));
    let mut changed = page.clone();
    changed.events[0].principal = "different original".parse().unwrap();
    assert!(original.readback(&changed,&proof).is_err());
    changed = page.clone(); changed.observation_sequence += 1;
    assert!(original.readback(&changed,&proof).is_err());
    changed = page.clone(); changed.events[0] = path_event(2,8); // Same operation, different member.
    assert!(!proof.present(&changed));
    changed = page.clone(); changed.events[0].references = References::Open {};
    assert!(!proof.present(&changed));
    let g = model.begin(); model.complete(&target,g,&req,Ok(page.clone()));
    assert!(original.current(&model)); // Same installed card and source after an identical refresh.
    model.close();
    let g = model.begin(); model.complete(&target,g,&req,Ok(page));
    assert!(!original.current(&model)); // Dismiss/reopen cannot reacquire old UI ownership.
}

#[test]
fn a_section_jump_cannot_borrow_a_node_from_another_interval_or_observation() {
    let mut p = page(&target(),1,3,false);
    let older = p.events[0].event_key();
    let newer = p.events[1].event_key();
    let more = p.events[2].event_key();
    let proof = Proof::Section { older:older.clone(), newer:newer.clone(), node:n(8) };
    assert!(!proof.present(&p));
    p.intervals = vec![Interval { older:newer.clone(), newer:more.clone(),
        body:IntervalBody::Complete { nodes:vec![IntervalNode {node:n(8),parents:vec![]}], branches:vec![] } }];
    assert!(!proof.present(&p));
    p.intervals[0].older=older; p.intervals[0].newer=newer;
    assert!(proof.present(&p));
    p.events.remove(0);
    assert!(!proof.present(&p));
}
