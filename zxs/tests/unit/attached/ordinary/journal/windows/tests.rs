use super::*;
#[test]
fn bounded_ordinals_do_not_cross_a_parent_or_consume_an_unconfirmed_child(){
    let work=[1,4,7,8,11,13];assert_eq!(positions(&work,2,6),&[4]);
    assert_eq!(positions(&work,6,10),&[7,8]);assert_eq!(positions(&work,10,13),&[11]);
    assert!(positions(&work,10,11).is_empty());
    let file=FileId::new().unwrap();let mut rows=BTreeMap::from([(file,vec![6,10])]);
    assert!(hide_last(&mut rows,file,6).is_err());assert_eq!(rows[&file],vec![6,10]);
    hide_last(&mut rows,file,10).unwrap();assert_eq!(rows[&file],vec![6]);
    hide_last(&mut rows,file,6).unwrap();assert!(!rows.contains_key(&file));
}
