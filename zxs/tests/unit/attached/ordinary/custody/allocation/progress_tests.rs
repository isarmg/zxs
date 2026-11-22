use super::Progress as P;
#[test]
fn only_the_original_final_leaf_can_precede_the_checked_empty_quarantine(){
    let (raw3,incr4)=(P::leaf(2).unwrap(),P::leaf(3).unwrap());
    assert!(raw3.allows_next(3,P::EMPTY));assert!(!raw3.allows_next(3,incr4));
    assert!(raw3.allows_next(4,incr4));assert!(!raw3.allows_next(4,P::EMPTY));
    assert!(!incr4.allows_next(3,P::EMPTY));assert!(incr4.allows_next(4,P::EMPTY));
    assert!(!P::ARMED.allows_next(3,P::EMPTY));assert!(!P::EMPTY.allows_next(3,P::EMPTY));
}
