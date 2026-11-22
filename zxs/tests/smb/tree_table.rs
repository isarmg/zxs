//! The route table is not a registrar. No fixture creates a real RustRoot.
use super::*;
#[test]
fn a_reserved_treeid_is_not_findable_and_cannot_be_reused_or_wrap(){
    let mut tables=TreeTable::new();
    let id=tables.propose().unwrap();
    assert_eq!(id.get(),1);
    assert!(tables.root(1).is_none());
    tables.reserve(&id).unwrap();
    assert!(tables.waiting(&id));
    assert!(tables.root(1).is_none());assert_eq!(tables.count(),0);
    assert!(matches!(tables.propose(),Err(Fault::SessionState)));
    let mut last=TreeTable{next:u32::MAX-1,..TreeTable::new()};
    let edge=last.propose().unwrap();assert_eq!(edge.get(),u32::MAX-1);
    last.reserve(&edge).unwrap();
    // This fixture only checks the allocation state; a real origin with
    // a failed pending reply is discarded, not reset or assigned again.
    assert_eq!(last.next,u32::MAX);
    let exhausted=TreeTable{next:last.next,..TreeTable::new()};
    assert!(matches!(exhausted.propose(),Err(Fault::Exhausted)));
}
