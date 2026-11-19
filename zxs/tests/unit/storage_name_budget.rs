//! Source cases: native component maximum and recorded path geometry are
//! different constraints. A numeric budget never grants access to the root.
use super::*;
use crate::{fs::{self,ComponentLimit,HeldDir},test_support::PrivateTemp};
use std::{os::unix::ffi::OsStrExt,path::{Path,PathBuf}};

fn spelling(display:&str)->ResidentNames{
    ResidentNames::for_file(FileName::parse(display.as_bytes()).unwrap(),FileId([0x81;16]),OperationId([0x92;16])).unwrap()
}
fn parent_width(width:usize)->PathBuf{
    let mut p=PathBuf::from("/anchor");
    while p.as_os_str().as_bytes().len()<width{
        let remaining=width-p.as_os_str().as_bytes().len();assert!(remaining>1);
        let mut k=(remaining-1).min(199);if remaining-k-1==1{k-=1;}
        p.push("a".repeat(k));
    }
    assert_eq!(p.as_os_str().as_bytes().len(),width);p
}
fn budget(path:&Path,limit:u8)->NameBudget{
    NameBudget::from_original(fs::path_budget(path).unwrap(),ComponentLimit::from_original(limit).unwrap())
}
#[test]
fn selected_ordinary_and_offline_recycle_must_both_fit_this_same_physical_spelling(){
    let short=budget(Path::new("/ordinary"),255);
    let s=spelling("abcdefghijklmnopqrst");assert_eq!(s.terminal().name().to_bytes().len(),53);
    let edge=budget(&parent_width(4042),255);let over=budget(&parent_width(4043),255);
    assert!(s.new_spelling(short,edge).is_ok());assert!(s.new_spelling(edge,short).is_ok());
    assert!(s.new_spelling(short,over).is_err());assert!(s.new_spelling(over,short).is_err());
    let little=spelling("a");assert_eq!(little.terminal().name().to_bytes().len(),34);
    // The final FILE and every 43-byte staging slot fit in both cases;
    // the 46-byte isolation leaf is exactly 4096 in the first.
    assert!(little.new_spelling(short,budget(&parent_width(4049),255)).is_ok());
    assert!(little.new_spelling(short,budget(&parent_width(4050),255)).is_err());
}
#[test]
fn depth_and_smaller_recorded_component_limits_apply_to_each_real_leaf(){
    let mut deep=PathBuf::from("/");for _ in 0..255{deep.push("d");}
    let small=spelling("x");assert!(small.at(budget(&deep,46)).is_ok());
    deep.push("e");assert!(small.at(budget(&deep,255)).is_err());
    let root=Path::new("/ordinary");
    assert!(small.at(budget(root,45)).is_err());assert!(small.at(budget(root,46)).is_ok());
    let large=spelling("abcdefghijklmnopqrst");
    assert!(large.new_spelling(budget(root,255),budget(root,52)).is_err());
    assert!(large.new_spelling(budget(root,255),budget(root,53)).is_ok());
    assert!(ComponentLimit::from_original(0).is_err());assert_eq!(ComponentLimit::from_original(1).unwrap().encoded(),1);
}
#[test]
fn a_live_component_limit_belongs_to_the_selected_directory_not_its_path_replacement(){
    let t=PrivateTemp::under(&std::env::temp_dir());let original=t.child("original");
    let held=HeldDir::open(&original).unwrap();let observed=ComponentLimit::held(&held).unwrap();
    assert!(observed.encoded()>0);let shape=NameBudget::held(&held).unwrap();
    assert_eq!(shape,NameBudget::from_original(fs::path_budget(&original).unwrap(),observed));
    std::fs::rename(&original,t.0.join("detached")).unwrap();let replacement=t.child("original");
    assert_ne!(held.entity,HeldDir::open(&replacement).unwrap().entity);
    assert!(ComponentLimit::held(&held).is_err());assert!(NameBudget::held(&held).is_err());
    // The stored number is an offline constraint only, not an I/O capability.
    assert_eq!(shape.child(c"x").unwrap(),fs::path_budget(&original.join("x")).unwrap());
}
