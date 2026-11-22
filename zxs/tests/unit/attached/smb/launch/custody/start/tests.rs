use super::*;
#[test]
fn the_fixed_pair_votes_both_original_stops_before_any_wait_cut(){
    use std::cell::{Cell,RefCell};
    let calls=RefCell::new(Vec::new());let count=Cell::new(0);
    start_both_stops(||{
        calls.borrow_mut().push("smb");
        count.set(count.get()+1);
        if count.get()==1{Err(Fault::Handoff)}else{Ok(())}
    },||{calls.borrow_mut().push("web");Ok(())});
    calls.borrow_mut().push("reap");
    assert_eq!(calls.into_inner(),vec!["smb","web","smb","reap"]);
}
