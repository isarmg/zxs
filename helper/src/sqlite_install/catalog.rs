//! Reselect the exact operation once from the original database. An admitted
//! SQL row describes its claim; it never supplies an unexamined storage path.
use super::{bad,originals::Group,request::{Id,Request,role},stage::SCHEMA};
use rusqlite::{Connection,params,types::ValueRef};
use std::{ffi::OsString,io,os::unix::ffi::{OsStrExt,OsStringExt},path::PathBuf};

fn sql(_:rusqlite::Error)->io::Error {bad("original SQLite catalog is unavailable or is not this exact format")}
#[derive(Eq,PartialEq)]
pub(super) struct Catalog {pub(super) pool:Id,pub(super) committed:bool,pub(super) markers:Vec<[u8;101]>}
pub(super) const SCHEMA_VERSION:i64=19;
pub(super) const MAX_OBJECTS:i32=178; // 146 declared descriptions and 32 implicit UNIQUE indexes
const SHAPE:&str="SELECT type,name,tbl_name,sql FROM main.sqlite_schema ORDER BY type COLLATE BINARY,name COLLATE BINARY LIMIT ?1";
type Shape=Vec<[Option<Vec<u8>>;4]>;
fn shape(c:&Connection)->io::Result<Shape>{
    let mut st=c.prepare(SHAPE).map_err(sql)?;let mut rows=st.query([MAX_OBJECTS+1]).map_err(sql)?;
    let mut result=Vec::new();let mut budget=256*1024usize;
    while let Some(row)=rows.next().map_err(sql)?{
        if result.len()>=MAX_OBJECTS as usize{return Err(bad("unexpected schema object count"));}
        let mut cols=[None,None,None,None];
        for (i,col) in cols.iter_mut().enumerate(){
            match row.get_ref(i).map_err(sql)?{
                ValueRef::Null=>{},ValueRef::Text(b) if b.len()<=65536&&b.len()<=budget=>{
                    budget-=b.len();*col=Some(b.to_vec());
                },_=>return Err(bad("invalid schema description")),
            }
        }
        result.push(cols);
    }
    Ok(result)
}
pub(super) struct Canonical(Shape, i64);
impl Canonical{
    pub(super) fn new()->io::Result<Self>{ Self::from_source(SCHEMA, SCHEMA_VERSION) }
    fn from_source(schema:&str,version:i64)->io::Result<Self>{
        // Only a private in-memory template. No file fallback, ATTACH or second
        // source of product state. Use the same upstream parsing/normalization.
        let db=Connection::open_in_memory().map_err(sql)?;
        super::engine::limits(&db)?;
        db.execute_batch(schema).map_err(sql)?;
        let s=shape(&db)?;db.close().map_err(|_|bad("schema template close failed"))?;Ok(Self(s,version))
    }
    pub(super) fn require(&self,tx:&rusqlite::Transaction<'_>)->io::Result<()>{
        let app:i64=tx.query_row("PRAGMA main.application_id",[],|row|row.get(0)).map_err(sql)?;
        let v:i64=tx.query_row("PRAGMA main.user_version",[],|row|row.get(0)).map_err(sql)?;
        if (app,v)!=(0x5a58_5331,self.1)||shape(tx)?!=self.0{return Err(bad("not the exact released database schema"));}
        Ok(())
    }
}
/// Fixed external ordinary marker for an already-validated SQL claim row.
pub(super) fn claimed_marker(role:u8,pool:&Id,volume:&Id,op:&Id,token:&[u8;32])->[u8;101] {
    let mut b=[0;101];b[..20].copy_from_slice(b"ZXS-SQLITE-1/VOLUME\0");b[20]=role;
    b[21..37].copy_from_slice(pool);b[37..53].copy_from_slice(volume);
    b[53..69].copy_from_slice(op);b[69..].copy_from_slice(token);b
}
pub(super) fn read(tx:&rusqlite::Transaction<'_>,template:&Canonical,group:&Group,r:&Request)->io::Result<Catalog>{
    if group.uid()!=r.uid()||!group.matches(r){return Err(bad("different approved group"));}
    read_rows(tx,template,r,|i,limit|group.limits().get(i).is_some_and(|n|i64::from(*n)==limit))
}

/// The non-S claimed limits are recorded facts, not observed filesystem
/// properties. Only the held S's actual limit is compared to the original.
pub(super) fn initial_receipt(tx:&rusqlite::Transaction<'_>,template:&Canonical,system:super::originals::HeldRoot<'_>,r:&Request)
    ->io::Result<Catalog>{
    if system.uid()!=r.uid()||system.path()!=r.paths()[0].as_path(){
        return Err(bad("different SYSTEM for the original INIT receipt"));
    }
    read_rows(tx,template,r,|i,limit|(super::MARKER.len() as i64..=255).contains(&limit)
        &&(i!=0||i64::from(system.byte_limit())==limit))
}
/// Canonical committed INIT metadata. Its selected raw R/F path bytes
/// are data, never authorizations to open them. Rebuild the original request
/// from the immutable recorded choices and audit against its request SHA.
/// A changed path alongside a still-valid VolumeId/marker is not adopted.
#[derive(Clone)]
pub(super) struct Initial { pub(super) path:PathBuf, pub(super) marker:[u8;101] }
/// Only a result from `initial_metadata` is accepted by the paired workers.
/// This never interprets an arbitrary path, nor selects an ordinary claim as R.
pub(super) fn initial_recycle(claims:&[Initial])->io::Result<&Initial>{
    let recycle=claims.get(1).filter(|c|c.marker[20]==1)
        .ok_or_else(||bad("no original recycle claim at the unique INIT ordinal"))?;
    if claims.iter().enumerate().any(|(i,c)|i!=1&&c.marker[20]==1){
        return Err(bad("ambiguous original recycle claim"));
    }
    Ok(recycle)
}
pub(super) fn initial_metadata(tx:&rusqlite::Transaction<'_>,template:&Canonical,system:super::originals::HeldRoot<'_>,
    pool:&Id,init:&Id)->io::Result<Vec<Initial>>{
    template.require(tx)?;
    let (r,recorded)={
        let mut s=tx.prepare(include_str!("initial-catalog.sql")).map_err(sql)?;
        let mut rows=s.query([&pool[..]]).map_err(sql)?;
        let first=rows.next().map_err(sql)?.ok_or_else(||bad("no original pool claims"))?;
        let n=|i|first.get::<_,i64>(i).map_err(sql);
        let b=|i|first.get_ref(i).map_err(sql)?.as_blob().map_err(|_|bad("invalid INIT receipt field"));
        let count=n(7)?;
        if !(3..=64).contains(&count)||n(4)?!=1||b(0)?!=pool||b(1)?!=init||
            n(6)?!=i64::from(system.uid())||b(11)?!=system.path().as_os_str().as_bytes(){
            return Err(bad("not the selected original committed INIT"));
        }
        let actor:Id=b(2)?.try_into().map_err(|_|bad("invalid original installer identity"))?;
        let approval:Id=b(5)?.try_into().map_err(|_|bad("invalid original installer approval"))?;
        let sha:[u8;32]=b(3)?.try_into().map_err(|_|bad("invalid original INIT request digest"))?;
        let mut paths=Vec::with_capacity(count as usize);
        paths.push(system.path().to_path_buf());
        for _ in 1..count{
            let row=rows.next().map_err(sql)?.ok_or_else(||bad("incomplete original claim listing"))?;
            let bytes=row.get_ref(11).map_err(sql)?.as_blob().map_err(|_|bad("invalid original root path bytes"))?;
            crate::path_rules::Kind::StorageRoot.check_path(bytes).map_err(bad)?;
            paths.push(PathBuf::from(OsString::from_vec(bytes.to_vec())));
        }
        if rows.next().map_err(sql)?.is_some(){return Err(bad("extra original claim rows"));}
        (Request::new(*init,actor,approval,system.uid(),paths)?,sha)
    };
    if r.sha()!=recorded{return Err(bad("claim paths no longer match the original INIT request"));}
    let c=initial_receipt(tx,template,system,&r)?;
    if !c.committed||c.pool!=*pool{return Err(bad("not the selected committed original INIT catalogue"));}
    Ok(r.paths().iter().cloned().zip(c.markers).map(|(path,marker)|Initial{path,marker}).collect())
}
fn read_rows(tx:&rusqlite::Transaction<'_>,template:&Canonical,r:&Request,limit_ok:impl Fn(usize,i64)->bool)->io::Result<Catalog>{
    template.require(tx)?;
    let (pool,state)=tx.query_row(include_str!("original-init.sql"),params![&r.op()[..],&r.actor()[..],&r.sha()[..]],|row|
        Ok((row.get::<_,Vec<u8>>(0)?,row.get::<_,i64>(1)?))).map_err(sql)?;
    let pool:Id=pool.try_into().map_err(|_|bad("invalid pool id"))?;
    if pool==[0;16]||!(0..=1).contains(&state){return Err(bad("invalid original INIT state"));}
    let mut st=tx.prepare(include_str!("initial-catalog.sql")).map_err(sql)?;
    let mut rows=st.query([&pool[..]]).map_err(sql)?;let mut markers=Vec::new();
    for (i,path) in r.paths().iter().enumerate(){
        let row=rows.next().map_err(sql)?.ok_or_else(||bad("missing original claim row"))?;
        let blob=|c|row.get_ref(c).map_err(sql)?.as_blob().map_err(|_|bad("invalid claim blob"));
        let num=|c|row.get::<_,i64>(c).map_err(sql);
        if blob(0)?!=pool||blob(1)?!=r.op()||blob(2)?!=r.actor()||blob(3)?!=r.sha()||num(4)?!=state
          ||blob(5)?!=r.approval()||num(6)?!=i64::from(r.uid())||num(7)?!=r.paths().len() as i64
          ||num(8)?!=i as i64||num(9)?!=i64::from(role(i))||blob(11)?!=path.as_os_str().as_bytes()
          ||!limit_ok(i,num(12)?){return Err(bad("catalog is not the complete original operation and selected roots"));}
        let volume:Id=blob(10)?.try_into().map_err(|_|bad("invalid volume id"))?;
        let token:[u8;32]=blob(13)?.try_into().map_err(|_|bad("invalid original claim token"))?;
        if volume==[0;16]||volume==pool||token==[0;32]||markers.iter().any(|m:&[u8;101]|&m[37..53]==volume.as_slice()||&m[69..]==token.as_slice()){
            return Err(bad("missing or ambiguous issued identity"));
        }
        markers.push(claimed_marker(role(i),&pool,&volume,&r.op(),&token));
    }
    if rows.next().map_err(sql)?.is_some(){return Err(bad("extra original claim rows"));}
    Ok(Catalog{pool,committed:state==1,markers})
}

#[cfg(test)]
mod schema_tests {
    use super::*;
    #[test]
    fn recycle_is_the_single_original_ordinal_not_an_ordinary_claim() {
        let row=|role,i|Initial{path:format!("/claim/{i}").into(),marker:claimed_marker(role,&[1;16],&[i;16],&[2;16],&[3;32])};
        let valid=[row(0,4),row(1,5),row(2,6)];
        assert_eq!(initial_recycle(&valid).unwrap().path,PathBuf::from("/claim/5"));
        assert!(initial_recycle(&[row(0,4),row(2,5),row(1,6)]).is_err());
        assert!(initial_recycle(&[row(0,4),row(1,5),row(1,6)]).is_err());
    }
    #[test]
    fn old_versions_and_a_same_size_prior_index_are_not_the_fresh_family() {
        let approved=Canonical::new().unwrap();
        assert_eq!(approved.0.len(),178);
        let mut db=Connection::open_in_memory().unwrap();
        super::super::engine::limits(&db).unwrap();
        db.execute_batch(SCHEMA).unwrap();
        let tx=db.transaction().unwrap();
        assert!(approved.require(&tx).is_ok());
        tx.execute_batch("PRAGMA main.user_version = 11;").unwrap();
        assert!(approved.require(&tx).is_err());
        tx.execute_batch("PRAGMA main.user_version = 12;").unwrap();
        assert!(approved.require(&tx).is_err());
        tx.execute_batch("PRAGMA main.user_version = 13;").unwrap();
        assert!(approved.require(&tx).is_err());
        tx.execute_batch("PRAGMA main.user_version = 14;").unwrap();
        assert!(approved.require(&tx).is_err());
        tx.execute_batch("PRAGMA main.user_version = 15;").unwrap();
        assert!(approved.require(&tx).is_err());
        tx.execute_batch("PRAGMA main.user_version = 16;").unwrap();
        assert!(approved.require(&tx).is_err());
        tx.execute_batch("PRAGMA main.user_version = 17;").unwrap();
        assert!(approved.require(&tx).is_err());
        tx.execute_batch("PRAGMA main.user_version = 18;").unwrap();
        assert!(approved.require(&tx).is_err());
        tx.execute_batch("PRAGMA main.user_version = 19;").unwrap();
        assert!(approved.require(&tx).is_ok());
        tx.execute_batch("DROP INDEX rename_destination_name;
                         CREATE INDEX rename_destination_name ON file_rename(volume_id,parent_id,target_name);").unwrap();
        assert_eq!(shape(&tx).unwrap().len(),178);
        assert!(approved.require(&tx).is_err());
    }
    #[test]
    fn the_current_version_and_row_count_do_not_admit_a_weakened_business_table() {
        let approved=Canonical::new().unwrap();
        for (original,weakened) in [
            ("CREATE TABLE file_label (\n  file_id BLOB NOT NULL REFERENCES root_entry(file_id),",
             "CREATE TABLE file_label (\n  file_id BLOB NOT NULL,"),
            ("target_setting_op BLOB REFERENCES share_mode(change_op),","target_setting_op BLOB,"),
            ("CREATE TABLE directory_location (\n  directory_id BLOB NOT NULL PRIMARY KEY REFERENCES directory_entry(directory_id),",
             "CREATE TABLE directory_location (\n  directory_id BLOB NOT NULL PRIMARY KEY,"),
        ]{
            assert_eq!(SCHEMA.matches(original).count(),1,"fixture must change one schema clause");
            let wrong=SCHEMA.replacen(original,weakened,1);
            assert_ne!(wrong,SCHEMA);
            let mut db=Connection::open_in_memory().unwrap();
            super::super::engine::limits(&db).unwrap();
            db.execute_batch(&wrong).unwrap();
            let tx=db.transaction().unwrap();
            let version:i64=tx.query_row("PRAGMA main.user_version",[],|row|row.get(0)).unwrap();
            assert_eq!(version,SCHEMA_VERSION);
            assert_eq!(shape(&tx).unwrap().len(),178);
            assert!(approved.require(&tx).is_err());
        }
    }
    #[test]
    fn bounded_fresh_schema_can_be_read_again_by_an_independent_connection() {
        use rusqlite::OpenFlags as F;
        let fixture=tempfile::tempdir().unwrap();let path=fixture.path().join("new-pool-fixture.sqlite3");
        let writer=Connection::open_with_flags_and_vfs(&path,
            F::SQLITE_OPEN_READ_WRITE|F::SQLITE_OPEN_CREATE|F::SQLITE_OPEN_NOFOLLOW,c"unix").unwrap();
        super::super::engine::limits(&writer).unwrap();
        writer.execute_batch(SCHEMA).unwrap();assert!(writer.close().is_ok());
        let mut reader=Connection::open_with_flags_and_vfs(&path,
            F::SQLITE_OPEN_READ_ONLY|F::SQLITE_OPEN_NOFOLLOW,c"unix").unwrap();
        super::super::engine::limits(&reader).unwrap();
        let tx=reader.transaction().unwrap();
        assert!(Canonical::new().unwrap().require(&tx).is_ok());
    }
}
