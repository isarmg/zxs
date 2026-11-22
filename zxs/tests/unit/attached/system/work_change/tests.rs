use super::*;
use crate::{PoolId,VolumeId,identity::ShareId,ordinary::{working_round::Map,working_sql::Spec}};

fn proposed_edit(save:OperationId)->Edit{
    let key=Key{pool:PoolId([2;16]),file:FileId([3;16]),round:save,save,revision:1};
    let spec=Spec{pool:key.pool,file:key.file,op:save,bytes:0,content_sha:[4;32],directory_sha:[5;32]};
    let map=Map{at:spec.table_len().unwrap(),bytes:115,sha:[6;32]};
    let record=Record::edit(key,End::ZERO,End{data:0,checks:map.at+map.bytes},0,spec.content_sha,spec.directory_sha,map).unwrap();
    Edit{record,actor:AccountId([7;16]),credential:OperationId([8;16]),generation:1,share:ShareId([9;16]),
        setting:OperationId([10;16]),position:OperationId([11;16]),volume:VolumeId([12;16]),nonce:OperationId([13;16]),
        issue:1,predecessor:None,parent:None,previous_pending:None,retained:0}
}

#[test]
fn sqlite_full_changes_category_only_after_known_absent_reservation(){
    let proposed=proposed_edit(OperationId([1;16]));
    let full=check(SqliteFailure::FULL).unwrap_err();
    assert_eq!(full.kind(),io::ErrorKind::InvalidData);
    let original=full.to_string();
    let definite=resolve_failed_reservation(full,proposed,Ok(None)).unwrap_err();
    assert_eq!(definite.kind(),io::ErrorKind::StorageFull);
    assert_eq!(definite.to_string(),original);
    for (code,kind) in [(5,io::ErrorKind::WouldBlock),(262,io::ErrorKind::WouldBlock),
        (776,io::ErrorKind::PermissionDenied),(778,io::ErrorKind::InvalidData),(11,io::ErrorKind::InvalidData),
        (26,io::ErrorKind::InvalidData)]{
        let error=check(code).unwrap_err();
        let message=error.to_string();
        let unchanged=resolve_failed_reservation(error,proposed,Ok(None)).unwrap_err();
        assert_eq!(unchanged.kind(),kind);
        assert_eq!(unchanged.to_string(),message);
    }
    let plain=io::Error::new(io::ErrorKind::InvalidData,original);
    assert_eq!(resolve_failed_reservation(plain,proposed,Ok(None)).unwrap_err().kind(),io::ErrorKind::InvalidData);
}

#[test]
fn sqlite_full_never_replaces_an_exact_or_uncertain_save(){
    let proposed=proposed_edit(OperationId([1;16]));
    let full=||check(SqliteFailure::FULL).unwrap_err();
    let recorded=Some(Issued{edit:proposed,confirmed:false});
    assert!(resolve_failed_reservation(full(),proposed,Ok(recorded)).is_ok());
    let different=Some(Issued{edit:Edit{issue:2,..proposed},confirmed:false});
    for observed in [Ok(different),Err(io::ErrorKind::WouldBlock.into())]{
        let error=resolve_failed_reservation(full(),proposed,observed).unwrap_err();
        assert!(error.get_ref().is_some_and(|cause|cause.is::<UnconfirmedSave>()));
    }
}

#[test]
fn exact_body_lookup_prevents_definite_full_when_a_reservation_is_incomplete(){
    let mut raw=std::ptr::null_mut();
    // SAFETY: fixed in-memory test DDL and one initialized connection slot.
    let ddl=c"CREATE TABLE work_edit(edit_op BLOB NOT NULL PRIMARY KEY) WITHOUT ROWID;
        INSERT INTO work_edit VALUES(X'01010101010101010101010101010101');";
    check(unsafe{crate::system::reader::zxs_sql_template(ddl.as_ptr(),&raw mut raw)}).unwrap();
    assert!(!raw.is_null());
    let db=Connection(raw);
    let proposed=proposed_edit(OperationId([1;16]));
    let full=||check(SqliteFailure::FULL).unwrap_err();

    let exact=require_absent_edit(&db,proposed.record.key().save).map(|()|None);
    let failure=resolve_failed_reservation(full(),proposed,exact).unwrap_err();
    assert!(failure.get_ref().is_some_and(|cause|cause.is::<UnconfirmedSave>()));
    let unrecorded=proposed_edit(OperationId([0x21;16]));
    let absent=require_absent_edit(&db,unrecorded.record.key().save).map(|()|None);
    assert_eq!(resolve_failed_reservation(full(),unrecorded,absent).unwrap_err().kind(),io::ErrorKind::StorageFull);
}
