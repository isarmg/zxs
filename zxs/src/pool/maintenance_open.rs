//! Pin one original MAINT register for the whole admitted read. Neither a
//! valid label on a different inode nor an account suffix can mint this empty-pool holder.
use super::*;
use crate::fs::stream::PinnedRegular;
use crate::pool::READY;

struct Ready{
    system:HeldDir,register:PinnedRegular,record:Record,frontier:fs::Frontier,
}
impl Ready{
    fn open(path:&Path)->io::Result<Self>{
        // Acquire regular SH first, then the short SYSTEM cut; no data roots
        // are opened from parsed paths until every original SYSTEM fact holds.
        let system=HeldDir::open(path)?;
        let register=system.shared_original(READY,MAX_RECORD as u64)?;system.share()?;
        let bytes=register.original_bytes(&system,READY)?;let frontier=register.opening_frontier()?;
        let(record,tail)=decode_genesis(&bytes,fs::local_uid())?;
        if register.stamp.stat.entity()!=record.origin||register.stamp.birth!=record.origin_birth{
            return Err(invalid("registration is not its committed original regular inode"));}
        let held=Self{system,register,record,frontier};held.require_system(&held.system)?;
        require_empty_tail(&held.record,tail,&frontier)?;
        held.register.file.sync_all()?;held.system.file.sync_all()?;
        held.register.named(&held.system,READY)?;
        held.system.file.unlock()?; // retain regular SH; no long SYSTEM lock
        Ok(held)
    }
    fn original_root(&self,i:usize,root:&HeldDir)->io::Result<()>{
        let r=&self.record;let row=&r.info.volumes[i];
        if root.path.as_os_str()!=row.volume.path.as_os_str()||root.entity!=row.volume.entity||
            fs::ComponentLimit::held(root)?!=r.limits[i]{
            return Err(invalid("original selected root, pathname or component limit changed"));}
        r.require_root(i,root)?;
        root.reject_registered_ancestors(&r.info.volumes.iter().map(|v|v.volume.entity).collect())?;
        if root.label()?.as_deref()!=Some(r.label_in_domain(i,LABEL).as_slice()){
            return Err(invalid("missing or different maintenance-format volume label"));}
        root.require_named()
    }
    fn require_system(&self,root:&HeldDir)->io::Result<()>{
        if !self.system.same_mount(root)?{return Err(invalid("changed selected SYSTEM mount"));}
        self.original_root(0,root)?;root.only(Some(READY))?;self.register.named(root,READY)?;
        if self.register.opening_frontier()?!=self.frontier{
            return Err(invalid("selected original REGISTRATION no longer has the admitted frontier"));}
        Ok(())
    }
    fn at_cut<T>(&self,observe:impl FnOnce()->io::Result<T>)->io::Result<T>{
        let cut=HeldDir::open(&self.system.path)?;cut.share()?;self.require_system(&cut)?;
        let result=observe()?;cut.require_named()?;self.register.named(&cut,READY)?;Ok(result)
    }
}

pub(in crate::pool) struct OnlineEmpty(Ready);
impl OnlineEmpty{
    pub(in crate::pool) fn open(path:&Path)->io::Result<Self>{
        Ready::open(path).map(Self)
    }
    /// Only a checked row from the original SYSTEM can select a data root.
    pub(in crate::pool) fn observe(&self,id:VolumeId)->io::Result<RegisteredVolume>{
        let held=&self.0;
        held.at_cut(||{
            let i=held.record.info.volumes.iter().position(|v|v.id==id).ok_or_else(||invalid("unknown original volume"))?;
            if i!=0{
                let root=HeldDir::open(&held.record.info.volumes[i].volume.path)?;
                root.share()?;held.original_root(i,&root)?;root.only(None)?;root.require_named()?;
            }
            Ok(held.record.info.volumes[i].clone())
        })
    }
}
