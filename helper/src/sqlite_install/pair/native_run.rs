//! Host-side decisions and the authenticated result for the private v7
//! handoff. No mode can choose media by parsing a UUID or receipt.
mod transport;
mod manifest;
mod worker;
mod release;
mod host;
pub(super) use host::{Selection,run};
pub(super) use manifest::Role;
pub(crate) use worker::run as exec_sql_child;
use super::{Id, bad, native_sql::{Ids,Original,transaction::ResultRow}};
use rustix::fs::{OFlags,fcntl_getfl};
use sha2::{Digest,Sha256};
use std::{fs::File,io,os::unix::fs::MetadataExt};

#[derive(Copy,Clone,Debug,Eq,PartialEq)]
pub(super) enum Intent{Lookup,New,Resume}
#[derive(Copy,Clone,Debug,Eq,PartialEq)]
enum Native{Missing,Previous,Target,Unknown}
#[derive(Copy,Clone,Debug,Eq,PartialEq)]
enum Work{NoChange,Create,Change,Confirm}
fn choose(intent:Intent,sql:Original<ResultRow>,native:Native,reset:bool)->io::Result<Work>{
    match(intent,sql,native,reset){
        (Intent::Lookup,_,_,_)|(_,Original::Committed(_),_,_)=>Ok(Work::NoChange),
        (Intent::New,Original::Absent,Native::Missing,false)=>Ok(Work::Create),
        (Intent::New,Original::Absent,Native::Previous,true)=>Ok(Work::Change),
        (Intent::Resume,Original::Pending(_),Native::Previous,true)=>Ok(Work::Change),
        (Intent::Resume,Original::Pending(_),Native::Target,_)=>Ok(Work::Confirm),
        _=>Err(bad("the original operation and native store do not permit the requested stage; nothing is adopted or repaired")),
    }
}
impl ResultRow {
    fn wire(self)->[u8;33]{
        let mut b=[0;33];
        match self{
            Self::Setup(ids)=>{b[0]=1;b[1..17].copy_from_slice(&ids.web_value());b[17..].copy_from_slice(&ids.smb_value());}
            Self::Reset(n)=>{b[0]=2;b[1..9].copy_from_slice(&n.to_be_bytes());}
        }b
    }
    fn unwire(b:&[u8;33])->io::Result<Self>{
        if b[0]==1{
            let id=|v:&[u8]|->Id{v.try_into().expect("fixed wire range")};
            return Ok(Self::Setup(Ids::checked(id(&b[1..17]),id(&b[17..]))?));
        }
        if b[0]==2&&b[9..]==[0;24]{
            let n=i64::from_be_bytes(b[1..9].try_into().expect("fixed reset generation"));
            if n>=2{return Ok(Self::Reset(n));}
        }
        Err(bad("wrong result role, generation or reserved bytes"))
    }
}
// This crosses only the one parent/child hop. While the returned O_PATH
// is held, the inode cannot be recycled into an identical-looking record.
// These numbers are not durable identifiers, not authorization, and never
// substitute for the supervisor's actual same-name and native readback.
fn original_stamp(pin:&File)->io::Result<[u8;32]>{
    let flags=fcntl_getfl(pin)?;let m=pin.metadata()?;
    let location=crate::pinned::Mount::on(pin)?.ok_or_else(||bad("missing native store mount identity"))?;
    if !flags.contains(OFlags::PATH)||!m.is_file()||m.uid()!=0||m.gid()!=0
        ||m.mode()!=0o100600||m.nlink()!=1||!(1..=64<<20).contains(&m.len())||m.ino()==0||location.id==0{
        return Err(bad("wrong returned original native data inode"));
    }
    let mut h=Sha256::new();h.update(b"helper native store after independent readback\0");
    for v in [m.dev(),m.ino(),location.id,m.nlink(),m.len()]{h.update(v.to_be_bytes());}
    for v in [m.mtime(),m.mtime_nsec(),m.ctime(),m.ctime_nsec()]{h.update(v.to_be_bytes());}
    Ok(h.finalize().into())
}

/// Only constructed by a live credentials-checked message on the original
/// private instance-maintenance channel. No caller-selected path or naked
/// boolean can complete SQL. The exact native target remains pinned.
pub(super) struct Synced{digest:[u8;32],row:ResultRow,stamp:[u8;32],pin:File}
impl Synced{
    fn receive(channel:&transport::Channel,digest:&[u8;32],row:ResultRow,until:std::time::Instant)->io::Result<Self>{
        let (body,[pin])=channel.wait::<1>(transport::Kind::NativeSynced,digest,until)?;
        let (a,b)=body.split_at(33);let expected=ResultRow::unwire(a.try_into().expect("fixed body"))?;
        let s=Self{digest:*digest,row:expected,stamp:b.try_into().expect("fixed body"),pin};
        s.require(digest,row)?;Ok(s)
    }
    pub(super) fn require(&self,digest:&[u8;32],row:ResultRow)->io::Result<()>{
        if self.digest!=*digest||self.row!=row||self.stamp!=original_stamp(&self.pin)?{
            return Err(bad("native result differs from this exact pending order or original object"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]fn a_pending_first_operation_never_creates_a_new_database_at_an_absent_name(){
        let first=ResultRow::Setup(Ids::checked([7;16],[8;16]).unwrap());
        let reset=ResultRow::Reset(2);
        assert_eq!(choose(Intent::New,Original::Absent,Native::Missing,false).unwrap(),Work::Create);
        assert!(choose(Intent::Resume,Original::Pending(first),Native::Missing,false).is_err());
        assert!(choose(Intent::New,Original::Pending(first),Native::Missing,false).is_err());
        assert!(choose(Intent::Resume,Original::Pending(reset),Native::Unknown,true).is_err());
        assert_eq!(choose(Intent::Resume,Original::Pending(first),Native::Target,false).unwrap(),Work::Confirm);
        assert_eq!(choose(Intent::Resume,Original::Pending(reset),Native::Previous,true).unwrap(),Work::Change);
        assert_eq!(choose(Intent::Lookup,Original::Pending(first),Native::Target,false).unwrap(),Work::NoChange);
        assert_eq!(choose(Intent::New,Original::Committed(first),Native::Unknown,false).unwrap(),Work::NoChange);
        assert_eq!(ResultRow::unwire(&first.wire()).unwrap(),first);
        assert_eq!(ResultRow::unwire(&reset.wire()).unwrap(),reset);
        let mut wrong=reset.wire();wrong[32]=1;assert!(ResultRow::unwire(&wrong).is_err());
    }
}
