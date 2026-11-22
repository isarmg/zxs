use super::*;

#[test]
fn directory_name_uses_the_linked_label_and_every_selected_ancestor(){
    let open=Rc::new(());
    let label=RustDirectoryName::capture(&open,None,"Data-é",std::iter::empty()).unwrap();
    assert_eq!(label.path(),r"\Data-é");
    let named=RustDirectoryName::capture(&open,Some((OperationId([1;16]),1)),
        "Data-é",["Ärchive","报告🚀"].into_iter()).unwrap();
    assert_eq!(named.path(),r"\Data-é\Ärchive\报告🚀");
    assert!(RustDirectoryName::capture(&open,None,"Data-é",["..","Leaf"].into_iter()).is_err());
}

#[test]
fn directory_name_witness_rejects_an_equal_open_or_reused_spelling_after_relabel(){
    let open=Rc::new(());
    let old=Some((OperationId([1;16]),1));
    let later=Some((OperationId([3;16]),3));
    let name=RustDirectoryName::capture(&open,old,"Data",["Reports"].into_iter()).unwrap();
    let renamed_back=RustDirectoryName::capture(&open,later,"Data",["Reports"].into_iter()).unwrap();
    assert_eq!(name.path(),renamed_back.path());
    assert!(name.ties(&open,old));
    assert!(!name.ties(&Rc::new(()),old));
    assert!(!name.ties(&open,None));
    assert!(!name.ties(&open,later));
}
