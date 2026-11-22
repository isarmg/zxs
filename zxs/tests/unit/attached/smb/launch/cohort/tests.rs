use super::*;
#[test]
fn paired_offer_only_fixed_shape_never_interprets_reserved_modes(){
    let v=fixture();
    assert_eq!(PairOffer::decode(v.encode()),Ok(v));
    for (start,len) in [(0,32),(32,32),(72,8),(80,32),(112,8)]{
        let mut b=v.encode();b[start..start+len].fill(0);
        assert!(PairOffer::decode(b).is_err());
    }
}
#[test]
fn paired_epoch_digest_changes_for_source_scope_and_authenticated_generation(){
    let mut e=PreflightEpoch{account:[9;16],op:[10;16],record:[11;16],sid:[12;28],generation:1,login:"fixture".to_owned()};
    let source=Entity{device:22,inode:23};
    let old=digest([24;32],[25;32],source,&e).unwrap();
    assert_ne!(digest([24;32],[26;32],source,&e).unwrap(),old);
    assert_ne!(digest([24;32],[25;32],Entity{inode:28,..source},&e).unwrap(),old);
    e.generation=2;
    assert_ne!(digest([24;32],[25;32],source,&e).unwrap(),old);
}
