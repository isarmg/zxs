//! The management list is an authorized SYSTEM projection at one original
//! prefix. It does not mount or scan R and never certifies current media.
use super::{Call,DeletionKey,Journal,Moved,Purpose,RecycleReceipt,Request,absolute_time,read_digest,receipt};
use crate::{FileId,PoolId,VolumeId,fs::{self,invalid},pool::lease::{Access,LocalPool}};
use std::{collections::BTreeMap,fmt,io,path::Path,str::FromStr};
const PAGE_MAX:u16=128;
#[derive(Clone,Copy,Debug,Default,Eq,PartialEq)]
pub struct RecycleFilter{pub ordinary_source:Option<VolumeId>,pub since_unix_seconds:Option<u64>,pub through_unix_seconds:Option<u64>}
impl RecycleFilter{
    fn authorize(self,p:&LocalPool)->io::Result<()>{
        if self.since_unix_seconds.zip(self.through_unix_seconds).is_some_and(|(a,b)|a>b){return Err(invalid("reversed original deletion time interval"));}
        // This local implementation discloses only the actual original pool
        // owner's rows. A source filter is an original FILE role, not a path.
        if let Some(v)=self.ordinary_source{let _=p.recycle_for(v)?;}Ok(())
    }
    fn includes(self,m:&Moved)->io::Result<bool>{
        let time=absolute_time(m.original.purpose)?.unix_seconds;
        Ok(self.ordinary_source.is_none_or(|id|id==m.original.from)&&
            self.since_unix_seconds.is_none_or(|t|t<=time)&&self.through_unix_seconds.is_none_or(|t|time<=t))
    }
}
/// The rows/order are fixed as of this cursor's authenticated original
/// frontier. The checksum prevents accidental mixing; it is NOT a bearer
/// credential. Every use rechecks the principal, register, actual prefix,
/// filter, reachable page boundary and each returned original.
#[derive(Clone,Debug,Eq,PartialEq)]
pub struct RecycleCursor{cut:u64,head:[u8;32],after:u64,scope:[u8;32]}
fn hex(d:&[u8;32])->String{d.iter().map(|v|format!("{v:02x}")).collect()}
impl fmt::Display for RecycleCursor{
    fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{write!(f,"1:{}:{}:{}:{}",self.cut,hex(&self.head),self.after,hex(&self.scope))}
}
impl FromStr for RecycleCursor{
    type Err=io::Error;
    fn from_str(s:&str)->io::Result<Self>{
        if !s.is_ascii()||s.len()>174{return Err(invalid("invalid recycle cursor length"));}
        let fields:Vec<_>=s.split(':').collect();if fields.len()!=5||fields[0]!="1"{return Err(invalid("invalid recycle cursor"));}
        let number=|x:&str|x.parse::<u64>().map_err(|_|invalid("invalid recycle cursor position"));
        let c=Self{cut:number(fields[1])?,head:read_digest(fields[2])?,after:number(fields[3])?,scope:read_digest(fields[4])?};
        if c.cut==0||c.after==0||c.after>c.cut||s!=c.to_string(){return Err(invalid("noncanonical recycle cursor"));}Ok(c)
    }
}
impl RecycleCursor{
    fn issue(pool:PoolId,uid:u32,f:RecycleFilter,limit:u16,cut:u64,head:[u8;32],after:u64)->io::Result<Self>{
        let mut b=b"ZXS/RECYCLE/PAGE/1\0".to_vec();
        b.extend_from_slice(&pool.0);b.extend_from_slice(&uid.to_be_bytes());b.extend_from_slice(&limit.to_be_bytes());
        if let Some(v)=f.ordinary_source{b.push(1);b.extend_from_slice(&v.0);}else{b.push(0);}
        for t in [f.since_unix_seconds,f.through_unix_seconds]{if let Some(n)=t{b.push(1);b.extend_from_slice(&n.to_be_bytes());}else{b.push(0);}}
        b.extend_from_slice(&cut.to_be_bytes());b.extend_from_slice(&head);b.extend_from_slice(&after.to_be_bytes());
        Ok(Self{cut,head,after,scope:crate::hash::digest(&b)?})
    }
}
#[derive(Clone,Debug,Eq,PartialEq)]
pub struct RetainedEntry{
    /// The exact deletion that made this FILE retained at the frozen cutoff.
    /// Its operation status is a current SYSTEM observation, not a health check.
    pub original:RecycleReceipt,pub retained_by_this_deletion_now:bool,pub pending_restore:Option<RecycleReceipt>,
}
#[derive(Clone,Debug,Eq,PartialEq)]
pub struct RecyclePage{pub as_of_sequence:u64,pub as_of_digest:[u8;32],pub entries:Vec<RetainedEntry>,pub next:Option<RecycleCursor>}

fn retained_at<'a>(s:&'a Journal,seq:u64,filter:RecycleFilter,uid:u32)->io::Result<Vec<&'a Moved>>{
    let mut retained:BTreeMap<FileId,&Moved>=BTreeMap::new();
    for m in s.moves().take_while(|m|m.seq<=seq){
        let o=&m.original;
        match o.purpose{
            Purpose::Ordinary=>{},
            Purpose::Delete{..}=>{if retained.insert(o.file,m).is_some(){return Err(invalid("two retained originals in one custody prefix"));}}
            Purpose::Restore{deleted_by,previous}=>{
                let old=retained.remove(&o.file).ok_or_else(||invalid("no corresponding deletion in confirmed prefix"))?;
                if old.original.op!=deleted_by||old.digest!=previous{return Err(invalid("restore does not match deletion at this cutoff"));}
            }
        }
    }
    let mut rows=Vec::new();
    for m in retained.into_values(){if m.original.uid==uid&&filter.includes(m)?{rows.push(m);}}
    // Wall-clock order is never used as the issuance order. FileId is not a
    // sorting surrogate for two identical human names at different roots.
    rows.sort_unstable_by_key(|m|std::cmp::Reverse(m.seq));Ok(rows)
}
pub fn list_retained(system:&Path,filter:RecycleFilter,limit:u16,cursor:Option<&RecycleCursor>)->io::Result<RecyclePage>{
    if !(1..=PAGE_MAX).contains(&limit){return Err(invalid("recycle page size must be 1 to 128"));}
    let(p,_,s)=LocalPool::with_snapshot(system,Access::Read)?;filter.authorize(&p)?;let uid=fs::local_uid();
    let(seq,head)=if let Some(c)=cursor{
        let head=s.prefix_digest(c.cut)?;if c!=&RecycleCursor::issue(p.id(),uid,filter,limit,c.cut,head,c.after)?{
            return Err(invalid("another pool, user, filter, page size or confirmed prefix"));}(c.cut,head)
    }else{s.frontier()};
    let rows=retained_at(&s,seq,filter,uid)?;let size=usize::from(limit);
    let start=if let Some(c)=cursor{
        let i=rows.iter().position(|m|m.seq==c.after).ok_or_else(||invalid("cursor is not an original retained row in its frozen scope"))?+1;
        if i%size!=0||i>=rows.len(){return Err(invalid("not an issued continuing page boundary"));}i
    }else{0};
    let end=start.checked_add(size).ok_or_else(||invalid("invalid page offset"))?.min(rows.len());
    let mut entries=Vec::with_capacity(end-start);
    for &m in &rows[start..end]{
        let o=&m.original;
        let r=Request{parents:o.parent_choice(),op:o.op,id:o.file,epoch:o.epoch,call:Call::Delete,from:o.from,to:o.to};
        let original=receipt(&p,&s,r)?.ok_or_else(||invalid("missing original deletion receipt"))?;
        let key=DeletionKey{file:o.file,operation:o.op,decision:m.digest};if original.deletion!=Some(key){
            return Err(invalid("different original on frozen recycle page"));}
        let retained_by_this_deletion_now=s.placement(o.file)?.retained().is_some_and(|now|now.operation==o.op&&now.decision==m.digest);
        let pending_restore=if let Some(a)=s.moving.as_ref().filter(|a|a.original.file==o.file&&
            matches!(a.original.purpose,Purpose::Restore{deleted_by,previous} if deleted_by==o.op&&previous==m.digest)){
            let v=&a.original;let r=Request{parents:v.parent_choice(),op:v.op,id:v.file,epoch:v.epoch,call:v.purpose.call(),from:v.from,to:v.to};
            Some(receipt(&p,&s,r)?.ok_or_else(||invalid("missing original pending restoration"))?)
        }else{None};
        entries.push(RetainedEntry{original,retained_by_this_deletion_now,pending_restore});
    }
    let next=if end<rows.len(){Some(RecycleCursor::issue(p.id(),uid,filter,limit,seq,head,rows[end-1].seq)?)}else{None};
    Ok(RecyclePage{as_of_sequence:seq,as_of_digest:head,entries,next})
}
