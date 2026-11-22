use super::*;
use crate::{DraftClaim,LengthPolicy};
fn edit(at:u64,len:u64,policy:LengthPolicy)->SpanEdit{
    SpanEdit::new(at,DraftClaim::new(len,crate::hash::digest(if len==0{b""}else{b"fixture"}).unwrap()).unwrap(),policy).unwrap()
}
#[test]
fn superseded_ranges_truncation_then_growth_and_overflow_preserve_exact_selected_origins(){
    let mut l=Layout::base(10);
    l.apply(10,edit(2,6,LengthPolicy::KeepOrGrow),10,0).unwrap();
    l.apply(10,edit(3,2,LengthPolicy::Set(6)),6,1).unwrap();
    l.apply(6,edit(0,0,LengthPolicy::Set(11)),11,2).unwrap();
    l.apply(11,edit(8,1,LengthPolicy::KeepOrGrow),11,3).unwrap();
    let mut got=Vec::new();l.visit(1,10,|s,o,n|{got.push((s,o,n));Ok(())}).unwrap();
    assert_eq!(got,vec![(From::Base,1,1),(From::Extent(0),0,1),(From::Extent(1),0,2),
        (From::Extent(0),3,1),(From::Zero,0,2),(From::Extent(3),0,1),(From::Zero,0,2)]);
    assert!(l.visit(u64::MAX,1,|_,_,_|Ok(())).is_err());assert!(l.visit(11,1,|_,_,_|Ok(())).is_err());
}
