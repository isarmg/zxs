//! Physical media that may be projected for this one authenticated preflight.
//! Paths come from the separately qualified original S reader, not the
//! terminal or a directory scan. Missing or uncertain originals stay absent.
use super::{SiteLease,denied,held::{self,PathPin,Role,Stamp}};
use crate::{fs::ComponentLimit,smb::launch::custody::VerifiedPreflight,system::{ROOT_MARK,reader::smb::runtime::PreflightClaim}};
use std::{fs::File,io,os::{fd::AsRawFd,unix::fs::FileExt},path::Path};

struct Volume{
    pin:PathPin,dir:File,marker:File,stamp:Stamp,bytes:[u8;101],limit:ComponentLimit,
}
impl Volume{
    fn select(site:&SiteLease<'_>,claim:&PreflightClaim,kind:u8)->io::Result<Self>{
        let host=site.parent.host().map_err(|_|denied())?;
        let pin=PathPin::select(host,&site.task,&claim.path,Role::Media(site.subject().uid,site.deployment.product()))?;
        if pin.overlaps(&site.system)||site.deployment.overlaps(&pin)||pin.overlaps(&site.site)||
            pin.overlaps(&site.image_pin)||claim.marker[20]!=kind{return Err(denied());}
        let dir=held::dir_reader(pin.held())?;
        let limit=ComponentLimit::from_original(claim.component_limit)?;
        if ComponentLimit::on_fd(&dir)?!=limit{return Err(denied());}
        let marker_pin=held::leaf(&dir,ROOT_MARK)?;
        let stamp=held::regular(&marker_pin)?;
        if stamp.place.mount!=pin.location().mount||stamp.place.entity.device!=pin.location().entity.device||
            stamp.mode!=0o100600||stamp.uid!=site.subject().uid||stamp.bytes!=101{return Err(denied());}
        let marker=held::read_original(&site.task,&marker_pin,stamp)?;
        let this=Self{pin,dir,marker,stamp,bytes:claim.marker,limit};
        this.require(site)?;
        Ok(this)
    }
    fn require(&self,site:&SiteLease<'_>)->io::Result<()>{
        let host=site.parent.host().map_err(|_|denied())?;
        self.pin.require(host,&site.task)?;
        let verify=||->io::Result<()>{
            if held::place(&self.dir)?!=self.pin.location()||ComponentLimit::on_fd(&self.dir)?!=self.limit||
                held::regular(&self.marker)?!=self.stamp||
                held::regular(&held::leaf(&self.dir,ROOT_MARK)?)?!=self.stamp{return Err(denied());}
            Ok(())
        };
        verify()?;
        held::rdonly(&self.marker)?;
        held::no_xattr(&self.marker,c"system.posix_acl_access")?;
        let mut buf=[0;101];
        self.marker.read_exact_at(&mut buf,0)?;
        let mut last=[0];
        if buf!=self.bytes||self.marker.read_at(&mut last,101)?!=0{return Err(denied());}
        verify()?;
        self.pin.require(host,&site.task)
    }
}
fn writable(f:&File)->io::Result<bool>{
    // SAFETY: only the actual selected directory and initialized output.
    let mut v:libc::statvfs=unsafe{std::mem::zeroed()};
    if unsafe{libc::fstatvfs(f.as_raw_fd(),&raw mut v)}!=0{return Err(io::Error::last_os_error());}
    Ok(v.f_flag&libc::ST_RDONLY==0)
}
fn separate_recycle(site:&SiteLease<'_>,r:&Volume,ordinary:&[Option<Volume>])->io::Result<()>{
    let host=site.parent.host().map_err(|_|denied())?;
    if !writable(&r.dir)?{return Err(denied());}
    // Compare backing identities from the pinned filesystem roots.
    let backing=r.pin.backing(host,&site.task)?;
    let r_where=r.pin.location();
    for origin in std::iter::once(&site.system).chain(ordinary.iter().flatten().map(|f|&f.pin)){
        let other=origin.location();
        if r.pin.overlaps(origin)||r_where.entity.device==other.entity.device||r_where.mount==other.mount||
            backing==origin.backing(host,&site.task)?{return Err(denied());}
    }
    Ok(())
}
#[must_use]
pub(in crate::smb::launch) struct Media<'v,'s,'p>{
    original:&'v VerifiedPreflight<'s,'p>,site:&'s SiteLease<'p>,
    ordinary:Vec<Option<Volume>>,recycle:Option<Volume>,
}
impl<'v,'s,'p> Media<'v,'s,'p>{
    pub(in crate::smb::launch) fn select(site:&'s SiteLease<'p>,original:&'v VerifiedPreflight<'s,'p>)->io::Result<Self>{
        let records=original.qualified_for(site)?;
        let mut ordinary=Vec::<Option<Volume>>::with_capacity(records.ordinary.len());
        for c in &records.ordinary{
            let selected=Volume::select(site,c,2).and_then(|actual|{
                if ordinary.iter().flatten().any(|other|actual.pin.overlaps(&other.pin)){return Err(denied());}
                Ok(actual)
            }).ok();
            ordinary.push(selected);
        }
        let recycle=Volume::select(site,&records.recycle,1).and_then(|r|{
            separate_recycle(site,&r,&ordinary)?;
            Ok(r)
        }).ok();
        let this=Self{original,site,ordinary,recycle};
        this.require_for(site,original)?;
        Ok(this)
    }
    pub(in crate::smb::launch) fn require_for(&self,site:&SiteLease<'p>,original:&VerifiedPreflight<'s,'p>)->io::Result<()>{
        if !std::ptr::eq(self.site,site)||!std::ptr::eq(self.original,original){return Err(denied());}
        original.qualified_for(site)?;
        for f in self.ordinary.iter().flatten(){f.require(site)?;}
        if let Some(r)=&self.recycle{
            r.require(site)?;
            separate_recycle(site,r,&self.ordinary)?;
        }
        original.qualified_for(site)?;
        Ok(())
    }
    pub(in crate::smb::launch) fn has_ordinary(&self,index:usize)->bool{self.ordinary.get(index).is_some_and(Option::is_some)}
    pub(in crate::smb::launch) fn recycle_available(&self)->bool{self.recycle.is_some()}
    pub(in crate::smb::launch) fn ordinary_roots(&self)->impl Iterator<Item=(&Path,&File)>{
        self.ordinary.iter().flatten().map(|f|(f.pin.path(),&f.dir))
    }
    pub(in crate::smb::launch) fn recycle_root(&self)->Option<(&Path,&File)>{
        self.recycle.as_ref().map(|f|(f.pin.path(),&f.dir))
    }
}
