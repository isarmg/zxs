//! One fixed protected original deployment for the normal program entry.
//! No search, CLI InstanceId or default fallback. Immutable site proof, not
//! a second credential, authority database or independent settings file.
use super::{layout,denied,held::{self,PathPin,PublicLeaf,Role},protected_dir};
use std::{fs::File,io,path::Path};

const MAGIC:&[u8;16]=b"ZXS-DEPLOYED-V1\0";
const SIZE:usize=64;

#[derive(Debug,Eq,PartialEq)]
struct Selected{instance:[u8;16],record:[u8;32]}
impl Selected{
    fn decode(raw:&[u8])->io::Result<Self>{
        let rest=raw.strip_prefix(MAGIC).filter(|r|r.len()==SIZE-MAGIC.len()).ok_or_else(denied)?;
        let (first,last)=rest.split_at(16);
        let instance=first.try_into().expect("exact deployment instance");
        let record=last.try_into().expect("exact deployment record digest");
        if instance==[0;16]||record==[0;32]{return Err(denied());}
        Ok(Self{instance,record})
    }
}

/// Directory SH stays live before the original S SH and per-site EX, in
/// the same order as the fresh one-time issuer's deployment-directory EX.
pub(super) struct Deployment{
    pin:PathPin,directory:File,record:PublicLeaf,selected:Selected,
}
impl Deployment{
    pub(super) fn select(host:&File,task:&File)->io::Result<Self>{
        let pin=PathPin::select(host,task,Path::new(layout::STATE),Role::FixedDir)?;
        let directory=held::dir_reader(pin.held())?;
        protection(&directory)?;
        directory.try_lock_shared().map_err(io::Error::from)?;
        pin.require(host,task)?;
        held::require_spelling(&directory,b"deployment.v1")?;
        let record=PublicLeaf::select(task,&directory,c"deployment.v1",0o100600,SIZE)?;
        let selected=Selected::decode(record.bytes())?;
        let this=Self{pin,directory,record,selected};
        this.require(host,task,this.selected.record)?;
        Ok(this)
    }
    pub(super) fn state_pin(&self)->&PathPin{&self.pin}
    pub(super) fn into_state_pin(self)->PathPin{self.pin} // O_PATH; the separate locked OFD drops
    pub(super) fn product(&self)->crate::fs::Entity{
        self.pin.parent_place().expect("fixed /zxs/state has a parent").entity
    }
    pub(super) fn instance(&self)->[u8;16]{self.selected.instance}
    pub(super) fn overlaps(&self,other:&PathPin)->bool{self.pin.overlaps(other)}
    pub(super) fn require(&self,host:&File,task:&File,digest:[u8;32])->io::Result<()>{
        if digest!=self.selected.record||held::place(&self.directory)?!=self.pin.location(){return Err(denied());}
        self.pin.require(host,task)?;
        protection(&self.directory)?;
        held::require_spelling(&self.directory,b"deployment.v1")?;
        self.record.require(task,&self.directory)?;
        self.pin.require(host,task)
    }
}
fn protection(directory:&File)->io::Result<()>{
    // The paired fresh issuer uses this same traversable parent.
    // The selector itself is separately checked root-private 0600.
    protected_dir(directory,0,0o040755)
}

#[cfg(test)]
#[path = "../../../../tests/unit/attached/smb/launch/site/deployment/tests.rs"]
mod tests;
