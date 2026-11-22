use super::*;
#[test]
fn staged_normalized_name_rejects_another_writer_revision_or_placement_even_with_the_same_spelling(){
    let owner=Rc::new(());let other=Rc::new(());
    let state=|r,own:&Rc<()>|RustStagedState{open:Rc::downgrade(own),revision:r};
    let original=OperationId([0x73;16]);
    let name=RustStagedName{state:state(4,&owner),position:(original,2),
        path:RustFileName::format(["Data-é","Archive🚀","Final.TXt"].into_iter()).unwrap()};
    assert_eq!(name.path(),r"\Data-é\Archive🚀\Final.TXt");
    assert!(name.require_identity(&state(4,&owner),(original,2)).is_ok());
    for (view,position) in [
        (state(4,&other),(original,2)),
        (state(5,&owner),(original,2)),
        (state(4,&owner),(original,3)),
        (state(4,&owner),(OperationId([0x74;16]),2)),
    ]{
        assert_eq!(name.require_identity(&view,position).unwrap_err().kind(),io::ErrorKind::InvalidInput);
    }
}
