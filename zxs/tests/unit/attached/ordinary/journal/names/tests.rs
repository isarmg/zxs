use super::*;
#[test]
fn occupation_cannot_authorize_withdrawal_and_equal_names_are_scoped_to_their_real_parent(){
    let a=VolumeId([1;16]);let b=VolumeId([2;16]);let e=EntryId([3;16]);let f=FileId([4;16]);
    let g=DirectoryId([5;16]);let h=DirectoryId([6;16]);let other=EntryId([7;16]);
    let file=Member::File{entry:e,file:f};let dir=Member::Directory{entry:other,directory:g};
    let(ea,eb)=(End::Ordinary(Parent::Root(a)),End::Ordinary(Parent::Root(b)));
    let mut n=Names::default();n.enter(Parent::Root(a),"report",dir).unwrap();
    assert!(n.occupied_root(a,"report"));assert!(n.endpoints(ea,eb,"report",file).is_err());
    assert!(n.create_file(Parent::Root(a),"report",e,f).is_err());assert!(n.hide_created(Parent::Root(a),"report",e,f).is_err());
    n.create_file(Parent::Child(a,g),"report",e,f).unwrap();n.enter(Parent::Child(a,h),"report",dir).unwrap();
    assert_eq!(n.get(Parent::Child(a,g),"report"),Some(file));assert_eq!(n.get(Parent::Child(a,h),"report"),Some(dir));
    assert_eq!(n.get(Parent::Child(b,g),"report"),None);assert_eq!(n.root(a,"report"),Some(dir));
    assert!(n.hide_created(Parent::Child(a,h),"report",e,f).is_err());
    n.hide_created(Parent::Child(a,g),"report",e,f).unwrap();
    assert_eq!(n.get(Parent::Child(a,g),"report"),None);
    assert_eq!(n.get(Parent::Child(a,h),"report"),Some(dir));assert_eq!(n.root(a,"report"),Some(dir));
    n.withdraw(Parent::Root(a),"report",dir).unwrap();n.create_file(Parent::Root(a),"report",e,f).unwrap();
    assert!(n.endpoints(ea,eb,"report",Member::File{entry:other,file:f}).is_err());
    assert!(n.endpoints(ea,eb,"report",Member::File{entry:e,file:FileId([8;16])}).is_err());
    n.endpoints(ea,eb,"report",file).unwrap();n.enter(Parent::Root(b),"report",dir).unwrap();
    assert!(n.endpoints(ea,eb,"report",file).is_err());assert_eq!(n.root(b,"report"),Some(dir));
}
#[test]
fn both_directions_change_only_the_exact_direct_parents_never_an_ordinary_recycle_row(){
    let a=VolumeId([1;16]);let b=VolumeId([2;16]);let r=VolumeId([3;16]);
    let left=Parent::Child(b,DirectoryId([4;16]));let right=Parent::Child(b,DirectoryId([5;16]));
    let here=Parent::Root(a);let root=End::Ordinary(here);let to_left=End::Ordinary(left);let to_right=End::Ordinary(right);let bin=End::Recycle(r);
    let f=Member::File{entry:EntryId([6;16]),file:FileId([7;16])};
    let sibling=Member::File{entry:EntryId([8;16]),file:FileId([9;16])};
    let mut names=Names::default();names.enter(here,"report",f).unwrap();names.enter(right,"report",sibling).unwrap();
    names.apply_pair(root,to_left,"report",f).unwrap();assert_eq!(names.get(left,"report"),Some(f));
    names.apply_pair(to_left,root,"report",f).unwrap();assert_eq!(names.get(here,"report"),Some(f));
    names.apply_pair(root,to_left,"report",f).unwrap();names.apply_pair(to_left,bin,"report",f).unwrap();
    assert_eq!(names.get(left,"report"),None);assert!(!names.occupied_root(r,"report"));
    assert!(names.apply_pair(bin,to_right,"report",f).is_err());assert_eq!(names.get(right,"report"),Some(sibling));
    names.apply_pair(bin,root,"report",f).unwrap();assert!(!names.occupied_root(r,"report"));
    names.apply_pair(root,bin,"report",f).unwrap();names.apply_pair(bin,to_left,"report",f).unwrap();
    assert_eq!(names.get(left,"report"),Some(f));assert_eq!(names.get(right,"report"),Some(sibling));
    assert_eq!(names.get(Parent::Root(b),"report"),None);assert!(!names.occupied_root(r,"report"));
}
#[test]
fn native_collision_in_one_child_cannot_withdraw_from_a_different_parent_on_failed_handoff_or_undo(){
    let a=VolumeId([1;16]);let b=VolumeId([2;16]);let id=FileId([3;16]);
    let file=Member::File{entry:EntryId([4;16]),file:id};
    let blocker=Member::Directory{entry:EntryId([5;16]),directory:DirectoryId([6;16])};
    let src=Parent::Child(a,DirectoryId([7;16]));let dst=Parent::Child(b,DirectoryId([8;16]));
    let display="Report.xlsx";let key=String::from_utf8(file.leaf(display).unwrap()).unwrap();
    let mut n=Names::default();n.enter(src,display,file).unwrap();n.enter(dst,&key,blocker).unwrap();
    assert_eq!(n.get(dst,display),None);assert!(n.apply_pair(End::Ordinary(src),End::Ordinary(dst),display,file).is_err());
    assert_eq!(n.get(src,display),Some(file));assert_eq!(n.get(dst,&key),Some(blocker));
    n.withdraw(dst,&key,blocker).unwrap();n.apply_pair(End::Ordinary(src),End::Ordinary(dst),display,file).unwrap();
    n.enter(src,&key,blocker).unwrap();
    assert!(n.apply_pair(End::Ordinary(dst),End::Ordinary(src),display,file).is_err());
    assert_eq!(n.get(dst,display),Some(file));assert_eq!(n.get(src,&key),Some(blocker));
}
#[test]
fn exact_display_names_share_one_typed_namespace_without_case_or_unicode_aliases(){
    use crate::ordinary::storage_name::{DirectoryName,FileName};
    let v=VolumeId([1;16]);let p=Parent::Root(v);let e=EntryId([2;16]);let f=FileId([3;16]);
    let e2=EntryId([4;16]);let f2=FileId([5;16]);let dir=Member::Directory{entry:EntryId([6;16]),directory:DirectoryId([7;16])};
    let upper=FileName::parse(b"Report.xlsx").unwrap();let lower=FileName::parse(b"report.xlsx").unwrap();
    let mut n=Names::default();n.create_file(p,upper.display(),e,f).unwrap();n.create_file(p,lower.display(),e2,f2).unwrap();
    assert_eq!(n.get(p,"Report.xlsx"),Some(Member::File{entry:e,file:f}));assert_eq!(n.get(p,"report.xlsx"),Some(Member::File{entry:e2,file:f2}));
    assert_eq!(n.get(p,"REPORT.xlsx"),None);
    let same=DirectoryName::parse(b"Report.xlsx").unwrap();assert!(n.enter(p,same.display(),dir).is_err());
    let third=DirectoryName::parse(b"REPORT.xlsx").unwrap();n.enter(p,third.display(),dir).unwrap();assert_eq!(n.get(p,third.display()),Some(dir));
    n.hide_created(p,upper.display(),e,f).unwrap();assert_eq!(n.get(p,lower.display()),Some(Member::File{entry:e2,file:f2}));
    let same_directory=Member::Directory{entry:EntryId([8;16]),directory:DirectoryId([9;16])};
    n.enter(p,same.display(),same_directory).unwrap();assert!(n.create_file(p,upper.display(),e,f).is_err());
    n.withdraw(p,same.display(),same_directory).unwrap();n.create_file(p,upper.display(),e,f).unwrap();
    assert_eq!(n.get(p,upper.display()),Some(Member::File{entry:e,file:f}));
    let composed="Café";let decomposed="Cafe\u{301}";
    n.create_file(p,FileName::parse(composed.as_bytes()).unwrap().display(),EntryId([9;16]),FileId([10;16])).unwrap();
    assert_eq!(n.get(p,decomposed),None);
    let other_directory=Member::Directory{entry:EntryId([11;16]),directory:DirectoryId([12;16])};
    n.enter(p,DirectoryName::parse(decomposed.as_bytes()).unwrap().display(),other_directory).unwrap();
}
