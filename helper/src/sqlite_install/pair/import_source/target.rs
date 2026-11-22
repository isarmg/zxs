//! Read-only constraint of one operator-selected, originally enrolled F.
use super::{Id, Stamp, unknown};
use crate::{pinned::Pinned, sqlite_install::{MARKER, catalog::Initial, markers::DiscoveryOrdinaryMark,
    originals::guarded_named_backing, pair::native_run::no_foreign_media}};
use helper_linux_evidence::portable_component_limit;
use rusqlite::{Transaction, params};
use rustix::fs::{FlockOperation, StatVfsMountFlags, flock, fstatvfs};
use std::{fs::File, io, os::unix::ffi::OsStrExt, path::PathBuf};

pub(in crate::sqlite_install::pair) const READ_ONLY:u8=1;
pub(in crate::sqlite_install::pair) const COMPONENT:u8=2;
pub(in crate::sqlite_install::pair) const RELATIVE:u8=4;

#[derive(Debug)]
pub(in crate::sqlite_install::pair) struct Target{
    pub(in crate::sqlite_install::pair) volume:Id,
    pub(in crate::sqlite_install::pair) path:PathBuf,
}
#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) struct Claim{marker:[u8;101],limit:u8}
impl Target{
    pub(super) fn claim(&self,tx:&Transaction<'_>,init:&Id,claims:&[Initial])->io::Result<Claim>{
        // A catalog row never selects the destination. It confirms the
        // terminal's exact original F ID and raw path together.
        let (n,entry)=claims.iter().enumerate().skip(2).find(|(_,entry)|entry.marker[20]==2
            &&entry.marker[37..53]==self.volume&&entry.path.as_os_str().as_bytes()==self.path.as_os_str().as_bytes())
            .ok_or_else(unknown)?;
        let bytes:i64=tx.query_row("SELECT component_limit FROM main.volume_claim
            WHERE claim_op=?1 AND ordinal=?2 AND role=2 AND volume_id=?3 AND root_path=?4",
            params![&init[..],n as i64,&self.volume[..],self.path.as_os_str().as_bytes()],|r|r.get(0)).map_err(|_|unknown())?;
        let limit=u8::try_from(bytes).ok().filter(|v|usize::from(*v)>=MARKER.len()).ok_or_else(unknown)?;
        Ok(Claim{marker:entry.marker,limit})
    }
}

pub(super) struct Held{pin:Pinned,dir:File,marker:DiscoveryOrdinaryMark,stamp:Stamp,claim:Claim,backing:u64}
impl Held{
    pub(super) fn open(pin:Pinned,claim:Claim,host:&File,uid:u32)->io::Result<Self>{
        let dir=pin.storage_directory()?;
        flock(&dir,FlockOperation::NonBlockingLockExclusive)?;
        let stamp=Stamp::get(&dir)?;
        if stamp.mode!=0o040_700||stamp.uid!=uid{return Err(unknown());}
        let backing=guarded_named_backing(&pin,&dir,host,uid)?;
        let marker=DiscoveryOrdinaryMark::select(&dir,uid,&claim.marker)?;
        Ok(Self{pin,dir,marker,stamp,claim,backing})
    }
    pub(super) fn sample(&self,host:&File,uid:u32)->io::Result<Sample>{
        self.require(host,uid)?;
        no_foreign_media(&self.dir)?; // fixed marker is the only name; no body opened
        let space=fstatvfs(&self.dir)?;
        if space.f_frsize==0||space.f_bavail>space.f_blocks{return Err(unknown());}
        let available=space.f_bavail.checked_mul(space.f_frsize).ok_or_else(unknown)?;
        self.require(host,uid)?;
        Ok(Sample{available,limit:self.claim.limit,readonly:space.f_flag.contains(StatVfsMountFlags::RDONLY)})
    }
    fn require(&self,host:&File,uid:u32)->io::Result<()>{
        if Stamp::get(&self.dir)?!=self.stamp||portable_component_limit(&self.dir)?!=self.claim.limit
            ||guarded_named_backing(&self.pin,&self.dir,host,uid)?!=self.backing{return Err(unknown());}
        self.marker.verify(&self.dir,&self.claim.marker)
    }
}

pub(super) struct Sample{available:u64,limit:u8,readonly:bool}
impl Sample{
    pub(super) fn restrict(self,later:Self,component:usize,relative:usize)->io::Result<Facts>{
        if self.limit!=later.limit||self.readonly!=later.readonly{return Err(unknown());}
        let mut blockers=0;
        if self.readonly{blockers|=READ_ONLY;}
        if component>usize::from(self.limit){blockers|=COMPONENT;}
        if relative>crate::path_rules::MAX_PATH_BYTES{blockers|=RELATIVE;}
        Ok(Facts{available:self.available.min(later.available),limit:self.limit,blockers})
    }
}

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(in crate::sqlite_install::pair) struct Facts{
    pub(in crate::sqlite_install::pair) available:u64,
    pub(in crate::sqlite_install::pair) limit:u8,
    pub(in crate::sqlite_install::pair) blockers:u8,
}
impl Facts{
    pub(in crate::sqlite_install::pair) fn valid(self)->bool{
        self.blockers&!(READ_ONLY|COMPONENT|RELATIVE)==0&&usize::from(self.limit)>=MARKER.len()
    }
}
