use super::*;

fn relative(name:&str)->TreeName{
    let bytes:Vec<_>=name.encode_utf16().flat_map(u16::to_le_bytes).collect();
    TreeName::create_relative(&bytes).unwrap()
}

#[test]
fn a_full_ordinary_rename_must_spell_only_the_original_parent() {
    let held=||["F","Reports","2026"].into_iter();
    assert_eq!(same_parent_leaf(&relative("报告.txt"),false,held()),Ok("报告.txt"));
    for rooted in [false,true]{
        assert_eq!(same_parent_leaf(&relative(r"F\Reports\2026\报告.txt"),rooted,held()),Ok("报告.txt"));
    }
    for bad in ["报告.txt",r"Reports\2026\报告.txt",r"f\Reports\2026\报告.txt",
        r"F\reports\2026\报告.txt",r"F\Reports\报告.txt",
        r"F\Reports\2026\Other\报告.txt",r"G\Reports\2026\报告.txt"]{
        assert_eq!(same_parent_leaf(&relative(bad),true,held()),Err(NameError::Field(FieldError::Unsupported)));
    }
    assert_eq!(same_parent_leaf(&relative(r"F\Reports\2026\报告.txt"),false,["F"].into_iter()),
        Err(NameError::Field(FieldError::Unsupported)));
    assert_eq!(same_parent_leaf(&relative(r"F\报告.txt"),true,["F"].into_iter()),Ok("报告.txt"));
    assert_eq!(same_parent_leaf(&relative(&"X".repeat(223)),false,held()),Err(NameError::TooLong));
}

#[test]
fn file_move_accepts_only_the_native_no_replace_standalone_form_in_the_held_folder(){
    let held=||["F","Reports"].into_iter();
    let move_to=relative(r"F\Archive\x.txt");
    let v=file_target(&move_to,false,false,true,"F",held()).unwrap();
    let FileRenameTarget::OtherParent{label,parents,name}=v else{panic!("not a two-parent selection");};
    assert_eq!((label,parents.as_slice(),name),("F",&["Archive"][..],"x.txt"));
    for (rooted,replace,standalone) in [(true,false,true),(false,true,true),(false,false,false)]{
        assert!(matches!(file_target(&move_to,rooted,replace,standalone,"F",held()),
            Err(NameError::Field(FieldError::Unsupported))));
    }
    for bad in [r"G\Archive\x.txt",r"R\Archive\x.txt",r"f\Archive\x.txt",r"Archive\x.txt"]{
        assert!(matches!(file_target(&relative(bad),false,false,true,"F",held()),
            Err(NameError::Field(FieldError::Unsupported))));
    }
    assert!(matches!(file_target(&relative(r"F\Reports\x.txt"),true,true,false,"F",held()),
        Ok(FileRenameTarget::SameParent("x.txt"))));
    assert!(matches!(file_target(&relative(r"F\x.txt"),false,false,true,"F",held()),
        Ok(FileRenameTarget::OtherParent{parents,..}) if parents.is_empty()));
    let max_depth=relative(&format!("F\\{}x.txt","D\\".repeat(MAX_PATH_DEPTH-1)));
    assert!(matches!(file_target(&max_depth,false,false,true,"F",held()),
        Ok(FileRenameTarget::OtherParent{parents,..}) if parents.len()==MAX_PATH_DEPTH-1));
    let too_deep=relative(&format!("F\\{}x.txt","D\\".repeat(MAX_PATH_DEPTH)));
    assert!(matches!(file_target(&too_deep,false,false,true,"F",held()),Err(NameError::TooLong)));
    let long_parents=format!("{}\\","D".repeat(255)).repeat(15);
    let length_fixture=|last:usize|relative(&format!("F\\{}{}{}\\x",long_parents,"中".repeat(84),"D".repeat(last-252)));
    // 15 * 256 + 254 + 1 separator + 1 terminal UTF-8 byte = 4,096.
    assert!(matches!(file_target(&length_fixture(254),false,false,true,"F",held()),
        Ok(FileRenameTarget::OtherParent{parents,..}) if parents.len()==16));
    assert!(matches!(file_target(&length_fixture(255),false,false,true,"F",held()),Err(NameError::TooLong)));
}

#[test]
fn cross_filesystem_spelling_is_explicit_bounded_and_never_approval(){
    let spelled=relative(r"G\Archive\报告.txt");
    let name=cross_filesystem_name(&spelled,false,false,"F").unwrap().unwrap();
    assert_eq!((name.label,name.parents.as_slice(),name.name),("G",&["Archive"][..],"报告.txt"));
    for (rooted,replace) in [(true,false),(false,true),(true,true)]{
        assert_eq!(cross_filesystem_name(&spelled,rooted,replace,"F"),Err(NameError::Field(FieldError::Unsupported)));
    }
    for not_cross in ["报告.txt",r"F\Archive\报告.txt"]{
        assert_eq!(cross_filesystem_name(&relative(not_cross),false,false,"F"),Ok(None));
    }
    for too_long in [format!("G\\{}","a".repeat(223)),format!("G\\{}b","D\\".repeat(MAX_PATH_DEPTH))]{
        assert_eq!(cross_filesystem_name(&relative(&too_long),false,false,"F"),Err(NameError::TooLong));
    }
}

#[test]
fn a_directory_target_uses_its_current_parent_spelling_and_its_own_component_limit(){
    let parent=||["F","Current"].into_iter();
    let name="D".repeat(255);
    let path=relative(&format!("F\\Current\\{name}"));
    assert_eq!(ordinary_parent_leaf(&path,true,parent(),255),Ok(name.as_str()));
    assert_eq!(same_parent_leaf(&path,true,parent()),Err(NameError::TooLong));
    for target in [r"F\Old\Destination",r"F\Current\Other\Destination",r"G\Current\Destination",r"Destination"]{
        assert_eq!(ordinary_parent_leaf(&relative(target),true,parent(),255),Err(NameError::Field(FieldError::Unsupported)));
    }
}

#[test]
fn a_directory_rename_turn_is_not_a_file_or_volume_root_turn(){
    use crate::{DirectoryId,VolumeId};
    use super::super::super::Vote;
    use std::num::NonZeroUsize;
    let pool=PoolId([1;16]);let directory=DirectoryId([2;16]);
    let object=Object::Directory(pool,directory);
    let owner=OpenCoordinator::new(NonZeroUsize::new(2).unwrap());
    let vote=Vote{needs:4,denies:0};
    let slot=Slot{owner:Arc::clone(&owner),object,vote,bound:true,retained:false,released:false,identity:Arc::new(())};
    {
        let mut table=owner.table.lock().unwrap();let row=table.objects.entry(object).or_default();
        row.add(vote);row.bound=1;table.live=1;
    }
    assert!(matches!(RenameTurn::take(&slot),Err(OrdinaryRenameError::AccessDenied)));
    let turn=RenameTurn::for_directory(&slot).unwrap();
    assert!(turn.represents(object));
    assert!(!turn.represents(Object::File(pool,FileId(directory.0))));
    assert!(!turn.represents(Object::OrdinaryRoot(pool,VolumeId(directory.0))));
    turn.require(&slot).unwrap();
    assert!(matches!(RenameTurn::for_directory(&slot),Err(OrdinaryRenameError::SharingViolation)));
    drop(turn);
    assert!(RenameTurn::for_directory(&slot).is_ok());
}

#[test]
fn only_the_single_bound_original_can_start_a_rename_turn(){
    let mut counts=Counts{live:1,bound:1,..Counts::default()};
    assert!(one_original(&counts).is_ok());
    counts.live=2; // even an unbound real vote could be about to open
    assert!(matches!(one_original(&counts),Err(OrdinaryRenameError::SharingViolation)));
    counts.live=1;
    counts.retained=1;
    assert!(matches!(one_original(&counts),Err(OrdinaryRenameError::SharingViolation)));
    counts.retained=0;
    let token=Arc::new(());
    counts.renaming=Some(Arc::downgrade(&token));
    assert!(matches!(one_original(&counts),Err(OrdinaryRenameError::SharingViolation)));
    counts.renaming=None;
    counts.deletion=Some(super::super::super::DeleteState::Retiring(Weak::new()));
    assert!(matches!(one_original(&counts),Err(OrdinaryRenameError::DeletePending)));
}

#[test]
fn related_retirement_holds_one_exact_empty_row_until_the_original_proof_drops(){
    use crate::{FileId,PoolId};
    use super::super::super::Vote;
    use std::num::NonZeroUsize;
    let owner=OpenCoordinator::new(NonZeroUsize::new(1).unwrap());
    let object=Object::File(PoolId([1;16]),FileId([2;16]));
    let vote=Vote{needs:4,denies:0};
    let slot=Slot{owner:Arc::clone(&owner),object,vote,bound:true,retained:false,released:false,identity:Arc::new(())};
    {
        let mut table=owner.table.lock().unwrap();
        table.live=1;
        let row=table.objects.entry(object).or_default();
        row.add(vote);
        row.bound=1;
    }
    let mut turn=RenameTurn::take(&slot).unwrap();
    assert!(turn.require_retired().is_err());
    turn.require(&slot).unwrap();
    turn.retire(slot).unwrap();
    turn.require_retired().unwrap();
    {
        let table=owner.table.lock().unwrap();
        let row=&table.objects[&object];
        assert_eq!((table.live,row.live,row.bound),(1,0,0));
        assert!(matches!(one_original(row),Err(OrdinaryRenameError::SharingViolation)));
    }
    drop(turn);
    let table=owner.table.lock().unwrap();
    assert_eq!(table.live,0);
    assert!(!table.objects.contains_key(&object));
}
