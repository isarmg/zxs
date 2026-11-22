//! Index-only source checks; no grant, OPEN, socket, account or test runtime.
use super::*;

#[test]
fn reserve_and_retire_track_the_actual_unique_persistent_entry(){
    let all=GlobalOpens::new();
    let old=all.reserve().unwrap();let neighbor=all.reserve().unwrap();
    assert_eq!(old.slot.id.get(),1);assert_eq!(neighbor.slot.id.get(),2);
    assert!(!old.slot.published.load(Ordering::Acquire));
    drop(old);
    let new=all.reserve().unwrap();assert_eq!(new.slot.id.get(),3);
    let index=all.0.lock().unwrap();assert_eq!(index.slots.len(),2);
    assert!(index.slots[0].ptr_eq(&Arc::downgrade(&neighbor.slot)));
    assert!(index.slots[1].ptr_eq(&Arc::downgrade(&new.slot)));
}

#[test]
fn open_budget_does_not_reuse_retired_or_invalid_numbers(){
    let all=GlobalOpens::new();
    let mut held=(0..PER_RUN.get()).map(|_|all.reserve().unwrap()).collect::<Vec<_>>();
    assert!(matches!(all.reserve(),Err(Fault::Resources)));
    drop(held.pop());
    assert_eq!(all.reserve().unwrap().slot.id.get(),257);
    all.0.lock().unwrap().next=u64::MAX;
    assert!(matches!(all.reserve(),Err(Fault::Exhausted)));
    assert!(matches!(next_id(0),Err(Fault::Exhausted)));
    assert_eq!(held.len(),PER_RUN.get()-1);
}

#[test]
fn a_reserved_seat_is_not_an_open_and_can_never_be_adopted_by_another_owner(){
    let global=GlobalOpens::new();
    let mut owner=OpenTable::new();
    let another=OpenTable::new();
    let seat=owner.seat(&global,7).unwrap();
    let old=WireOpenId{persistent:seat.ticket.slot.id.get(),volatile:seat.volatile.get()};
    assert!(owner.waiting_ticket(&seat.ticket));
    assert!(!another.waiting_ticket(&seat.ticket));
    assert!(owner.find(old).is_none());
    assert!(matches!(owner.seat(&global,7),Err(Fault::SessionState)));
    assert!(matches!(owner.retire_tree(7),Err(Fault::SessionState)));
    drop(seat);
    assert!(global.0.lock().unwrap().slots.is_empty());
    assert!(owner.retire_tree(7).is_ok());
    let next=owner.seat(&global,7).unwrap();
    assert!(next.ticket.slot.id.get()>old.persistent);
    assert!(next.volatile.get()>old.volatile);
}
