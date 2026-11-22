//! Unrun source tests of the conservative H04 observation. All data is
//! synthetic; no retirement worker, real S/F, product or service is invoked.
use super::*;
use crate::sqlite_install::stage;
use rusqlite::Connection;

#[test]
fn current_schema_has_exactly_the_audited_table_set(){
    let mut db=Connection::open_in_memory().unwrap();
    engine::limits(&db).unwrap();
    db.execute_batch(stage::SCHEMA).unwrap();
    let tx=db.transaction().unwrap();
    template().unwrap().require(&tx).unwrap();
    let tables=tx.prepare("SELECT name FROM main.sqlite_schema WHERE type='table'").unwrap()
        .query_map([],|r|r.get::<_,String>(0)).unwrap().collect::<Result<Vec<_>,_>>().unwrap();
    let observed=tables.iter().map(String::as_str).collect::<std::collections::BTreeSet<_>>();
    assert_eq!(observed,AUDITED_TABLES.iter().copied().collect());
    complete_small_cut(&tx).unwrap();
    assert!(!responsibilities(&tx,&[9;16]).unwrap().any());
}

fn synthetic_columns(db:&Connection){
    // Shape is intentionally weak to exercise adverse/potentially malformed
    // historical facts, not a substitute for template() or the product DDL.
    db.execute_batch("CREATE TABLE root_share(volume_id,publish_op);
        CREATE TABLE share_mode(change_op);
        CREATE TABLE file_location(volume_id);
        CREATE TABLE directory_entry(volume_id,create_op);
        CREATE TABLE root_entry(volume_id,create_op);
        CREATE TABLE file_rename(volume_id,rename_op);
        CREATE TABLE file_transfer(source_id,target_id,transfer_op,pending);
        CREATE TABLE file_transfer_source(transfer_op,status);
        CREATE TABLE operation(op_id,kind);
        CREATE TABLE first_work(save_op);
        CREATE TABLE work_edit(edit_op);
        CREATE TABLE manual_node(commit_op);
        CREATE TABLE directory_rename(rename_op);
        CREATE VIEW pending_transfer AS SELECT source_id,target_id FROM file_transfer WHERE pending=1;").unwrap();
}

#[test]
fn historic_origins_never_clear_a_currently_empty_volume(){
    let id=[9u8;16];let other=[8u8;16];let op=[7u8;16];
    let mut db=Connection::open_in_memory().unwrap();synthetic_columns(&db);
    let t=db.transaction().unwrap();
    let r=||responsibilities(&t,&id).unwrap();
    assert!(!r().any());
    t.execute("INSERT INTO root_share VALUES(?1,?2)",params![&id[..],&op[..]]).unwrap();
    assert!(r().publication); // independent of live SMB setting; none exists here
    t.execute("DELETE FROM root_share",[]).unwrap();
    t.execute("INSERT INTO file_transfer VALUES(?1,?2,?3,1)",params![&other[..],&id[..],&op[..]]).unwrap();
    assert!(r().pending_transfer);
    t.execute("UPDATE file_transfer SET pending=0",[]).unwrap();
    for status in [0,1,2] {
        t.execute("DELETE FROM file_transfer_source",[]).unwrap();
        t.execute("INSERT INTO file_transfer_source VALUES(?1,?2)",params![&op[..],status]).unwrap();
        let now=r();
        assert!(now.transfer_history&&now.any()&&!now.pending_transfer);
        assert_eq!((now.armed_cleanup,now.unproved_cleanup),(status==0,status==2));
    }
    t.execute("DELETE FROM file_transfer",[]).unwrap();
    t.execute("INSERT INTO root_entry VALUES(?1,?2)",params![&id[..],&op[..]]).unwrap();
    assert!(r().file_birth&&!r().selected_file);
    t.execute("INSERT INTO directory_entry VALUES(?1,?2)",params![&id[..],&op[..]]).unwrap();
    assert!(r().directory_origin);
    t.execute("INSERT INTO operation VALUES(?1,14)",[&op[..]]).unwrap();
    let missing=||t.query_row(include_str!("../../src/sqlite_install/pair/retire_ordinary/unbound-pending.sql"),[],|r|r.get::<_,bool>(0)).unwrap();
    assert!(missing());
    t.execute("INSERT INTO file_rename VALUES(?1,?2)",params![&id[..],&op[..]]).unwrap();
    assert!(r().file_rename_history&&!missing());
}

#[test]
fn an_unexamined_tail_never_counts_as_complete(){
    let mut db=Connection::open_in_memory().unwrap();
    db.execute_batch("CREATE TABLE operation (i INTEGER NOT NULL);").unwrap();
    let t=db.transaction().unwrap();
    t.execute("WITH RECURSIVE n(i) AS (SELECT 1 UNION ALL SELECT i+1 FROM n WHERE i<?1)
        INSERT INTO operation SELECT i FROM n",[ROW_CEILING+1]).unwrap();
    let err=complete_small_cut(&t).unwrap_err();
    assert!(err.to_string().contains("exceeds the bounded"));
}

#[test]
#[ignore="source-only example; requires explicitly provisioned disposable parents on separate filesystems"]
fn the_actual_original_is_clear_only_with_another_ordinary_and_no_extra_entry(){
    use crate::sqlite_install::{originals::Originals,request::Request};
    use std::{fs,os::unix::fs::PermissionsExt};
    let area=|key|{
        let p=tempfile::Builder::new().prefix("helper-retire-preflight-")
            .tempdir_in(std::env::var_os(key).expect("explicit disposable fixture parent")).unwrap();
        fs::set_permissions(p.path(),fs::Permissions::from_mode(0o700)).unwrap();p
    };
    for second_f in [false,true]{
        let f=area("HELPER_TEST_PORTABLE_DATA_PARENT");let r=area("HELPER_TEST_PORTABLE_RECYCLE_PARENT");
        let mut paths=vec![f.path().join("S"),r.path().join("R"),f.path().join("F1")];
        if second_f{paths.push(f.path().join("F2"));}
        for p in &paths{fs::create_dir(p).unwrap();fs::set_permissions(p,fs::Permissions::from_mode(0o700)).unwrap();}
        let uid=rustix::process::getuid().as_raw();assert!(uid>0);
        let init=Request::new([1;16],[2;16],[3;16],uid,paths).unwrap();
        let (pool,ids)={let group=Originals::pin(&init,File::open("/").unwrap()).unwrap().lock(uid).unwrap();
            stage::seed_retirement_fixture(&init,&group).unwrap()};
        let locked=LockedSystem::from_passive_fixture(&init.paths()[0]).unwrap();
        let scope=|id,n:usize|Scope::new(pool,init.op(),id,init.paths()[0].clone(),init.paths()[n].clone()).unwrap();
        let selected=scope(ids[2],2);
        let first=inspect(&locked,&selected).unwrap();
        assert_eq!((first.ordinal,first.volume,first.original_init),(2,ids[2],init.op()));
        assert_eq!(first.only_ordinary,!second_f);
        assert_eq!(first.no_observed_blockers(),second_f);
        // Even an admissible F pathname cannot stand in for S by VolumeId:
        // exact role rejection precedes any attempt to open the target F.
        for forbidden in [scope(ids[0],2),scope(ids[1],1)]{
            let reason=inspect(&locked,&forbidden).unwrap_err();
            assert_eq!(reason.to_string(),"SYSTEM and the sole recycle volume cannot be retired");
        }
        if second_f{assert!(inspect(&locked,&scope(ids[2],3)).is_err());}
        fs::create_dir(init.paths()[2].join(".extra")).unwrap();
        let changed=inspect(&locked,&selected).unwrap();
        assert!(changed.extra_entry&&!changed.no_observed_blockers());
    }
}
