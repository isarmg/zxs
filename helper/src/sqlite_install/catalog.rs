//! Reselect the exact operation once from the original database. An admitted
//! SQL row describes its claim; it never supplies an unexamined storage path.
use super::{bad,originals::Group,request::{Id,Request,role},stage::SCHEMA};
use rusqlite::{Connection,params,types::ValueRef};
use std::{io,os::unix::ffi::OsStrExt};

fn sql(_:rusqlite::Error)->io::Error {bad("original SQLite catalog is unavailable or is not this exact format")}
#[derive(Eq,PartialEq)]
pub(super) struct Catalog {pub(super) pool:Id,pub(super) committed:bool,pub(super) markers:Vec<[u8;101]>}
pub(super) const SCHEMA_VERSION:i64=6;
pub(super) const MAX_OBJECTS:i32=128; // includes SQLite's implicit indexes
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
pub(super) struct Canonical(Shape);
impl Canonical{
    pub(super) fn new()->io::Result<Self>{
        // Only a private in-memory template. No file fallback, ATTACH or second
        // source of product state. Use the same upstream parsing/normalization.
        let db=Connection::open_in_memory().map_err(sql)?;
        super::engine::limits(&db)?;
        db.execute_batch(SCHEMA).map_err(sql)?;
        let s=shape(&db)?;db.close().map_err(|_|bad("schema template close failed"))?;Ok(Self(s))
    }
}
pub(super) fn read(tx:&rusqlite::Transaction<'_>,template:&Canonical,group:&Group,r:&Request)->io::Result<Catalog>{
    if group.uid()!=r.uid()||!group.matches(r){return Err(bad("different approved group"));}
    let app:i64=tx.query_row("PRAGMA main.application_id",[],|row|row.get(0)).map_err(sql)?;
    let v:i64=tx.query_row("PRAGMA main.user_version",[],|row|row.get(0)).map_err(sql)?;
    if (app,v)!=(0x5a585331,SCHEMA_VERSION)||shape(tx)?!=template.0{return Err(bad("not the exact released database schema"));}
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
          ||num(12)?!=i64::from(group.limits()[i]){return Err(bad("catalog is not the complete original operation and selected roots"));}
        let volume:Id=blob(10)?.try_into().map_err(|_|bad("invalid volume id"))?;
        let token:[u8;32]=blob(13)?.try_into().map_err(|_|bad("invalid original claim token"))?;
        if volume==[0;16]||volume==pool||token==[0;32]||markers.iter().any(|m:&[u8;101]|&m[37..53]==volume.as_slice()||&m[69..]==token.as_slice()){
            return Err(bad("missing or ambiguous issued identity"));
        }
        let mut m=[0;101];m[..20].copy_from_slice(b"ZXS-SQLITE-1/VOLUME\0");m[20]=role(i);
        m[21..37].copy_from_slice(&pool);m[37..53].copy_from_slice(&volume);m[53..69].copy_from_slice(&r.op());m[69..].copy_from_slice(&token);
        markers.push(m);
    }
    if rows.next().map_err(sql)?.is_some(){return Err(bad("extra original claim rows"));}
    Ok(Catalog{pool,committed:state==1,markers})
}
