//! Same place does not mean same transfer result. Exercise the real
//! kind-12 SQL projection with disposable in-memory rows, not media or wire.
use super::*;
use crate::{BranchId,EntryId,FileId,HistoryId,PoolId,ordinary::{working_round::Record,working_sql::Spec},
    system::reader::{check,zxs_sql_template}};
use std::ptr;

fn memory()->Connection{
    // Only the tables read/issued by this receipt check. Not a replacement
    // for the real schema's triggers, filesystem or original session tests.
    let ddl=c"CREATE TABLE operation(op_id BLOB PRIMARY KEY,kind INTEGER,actor_id BLOB,request_sha BLOB,state INTEGER) STRICT;
        CREATE TABLE file_transfer(
          transfer_op BLOB PRIMARY KEY,file_id BLOB,direction INTEGER,source_op BLOB,source_gen INTEGER,
          source_id BLOB,source_name BLOB,target_id BLOB,target_name BLOB,share_id BLOB,setting_op BLOB,
          auth_op BLOB,save_op BLOB,manual_op BLOB,source_parent BLOB,target_parent BLOB,source_backing BLOB,
          source_terminal BLOB,target_setting_op BLOB) STRICT;";
    let mut raw=ptr::null_mut();
    check(unsafe{zxs_sql_template(ddl.as_ptr(),&mut raw)}).unwrap();
    assert!(!raw.is_null());
    Connection(raw)
}

fn op(i:u8)->OperationId{OperationId([i;16])}
fn route(kind:Direction)->Route{
    let f=VolumeId([10;16]);let r=VolumeId([11;16]);let file=FileId([12;16]);
    let birth=Birth{pool:PoolId([13;16]),volume:f,operation:op(14),entry:EntryId([15;16]),
        file,history:HistoryId([16;16]),branch:BranchId([17;16]),name:"Original.txt".into(),ticket:op(18),slot:1};
    let (here,to,name)=match kind{
        Direction::Delete=>(Place::origin(&birth,DirectoryParent::Root),r,birth.name.clone()),
        Direction::Restore=>(Place{file,op:op(19),generation:2,volume:r,role:Role::Recycle,
            name:birth.name.clone(),parent:DirectoryParent::Root,backing:Some(op(19))},f,"Restored.txt".into()),
        Direction::Ordinary=>unreachable!(),
    };
    Route{from:Placed{birth,here},kind,to,parent:DirectoryParent::Root,name,work:None,head:None,
        share:ShareId([20;16]),setting:op(21),auth:op(22),actor:AccountId([23;16]),epoch:1,destination:None}
}

fn selected(db:&Connection,plan:&Route,id:OperationId)->Moved{
    assert!(plan.confirmed_at(db,id).unwrap().is_none());
    plan.issue(db,id).unwrap();
    assert!(plan.pending_at(db,id).unwrap());
    assert!(plan.confirmed_at(db,id).is_err());
    db.fixed(c"UPDATE main.operation SET state=1").unwrap();
    let chosen=plan.confirmed_at(db,id).unwrap().unwrap();
    assert_eq!(plan.confirmed_at(db,id).unwrap().as_ref(),Some(&chosen));
    chosen
}

#[test]
fn delete_and_restore_receipts_keep_original_save_and_history_at_the_same_places(){
    for kind in [Direction::Delete,Direction::Restore]{
        let id=op(30);
        let mut plan=route(kind);
        let birth=&plan.from.birth;
        let work=|id|ReadRecord{record:Record::first(Spec::checked(birth.pool,birth.file,id,1,[8;32],[9;32]).unwrap()).unwrap(),issue:1};
        plan.work=Some(work(op(31)));plan.head=Some(op(32));
        let mut different_save=plan.clone();different_save.work=Some(work(op(33)));
        let mut different_history=plan.clone();different_history.head=Some(op(34));
        let db=memory();
        let first=selected(&db,&plan,id);
        // Distinct internally consistent memory cuts with the same public
        // operation/place coordinates must not compare as equal receipts.
        for other in [different_save,different_history]{
            assert!(other.confirmed_at(&db,id).is_err());
            let changed=selected(&memory(),&other,id);
            assert_eq!((changed.op,&changed.from,&changed.to),(first.op,&first.from,&first.to));
            assert_ne!(changed,first);
        }
    }
}
