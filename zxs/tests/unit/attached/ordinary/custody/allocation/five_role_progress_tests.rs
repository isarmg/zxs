use super::Progress as P;
#[test]
fn only_the_frozen_role_sequence_can_reach_verified_empty(){
    assert_eq!(P::read(7).unwrap(),P::EMPTY);
    assert_eq!(P::read(8).unwrap(),P::leaf(4).unwrap());
    assert!(P::read(9).is_err());assert!(P::leaf(5).is_err());
    for (count,steps) in [(3,&[2,3,4,5,7][..]),(4,&[2,3,4,5,6,7][..]),(5,&[2,3,4,5,6,8,7][..])]{
        for from in 0..=8 {for to in 0..=8{
            let a=P::read(from).unwrap();let b=P::read(to).unwrap();
            assert_eq!(a.allows_next(count,b),steps.windows(2).any(|p|p[0]==from&&p[1]==to),"{count}: {from} => {to}");
        }}
    }
    assert_eq!(P::EMPTY.authorized(),None);assert_eq!(P::leaf(4).unwrap().authorized(),Some(4));
    assert!(P::EMPTY.past_leaf(4));assert!(!P::leaf(4).unwrap().past_leaf(4));
    assert!(P::leaf(4).unwrap().past_leaf(3));
}
