//! Read-only, source-gated first cold admission. This original register SH
//! survives the short SYSTEM transaction and every slower per-root query.
use super::*;
use crate::fs::stream::PinnedRegular;
use crate::pool::READY;

/// No writable FD, registration installer or derived business capability
/// escapes. Existing business entry points continue to reject this format.
pub(in crate::pool) struct OnlineEmpty{
    system:HeldDir,
    register:PinnedRegular,
    record:Record,
}
impl OnlineEmpty{
    /// The only externally chosen path is SYSTEM. Parse bytes under the original
    /// regular file flock; never open a root named by an untrusted registration.
    pub(in crate::pool) fn open(path:&Path)->io::Result<Self>{
        let system=HeldDir::open(path)?;
        let register=system.shared_original(READY,MAX_RECORD as u64)?;
        system.share()?;let bytes=register.original_bytes(&system,READY)?;
        let record=decode_empty(&bytes,fs::local_uid(),&register.opening_frontier()?)?;
        if register.stamp.stat.entity()!=record.origin||register.stamp.birth!=record.origin_birth{
            return Err(invalid("registration is not its committed original regular inode"));}
        let admitted=Self{system,register,record};
        admitted.require_system(&admitted.system)?;
        // Only sync a positively identified register, not a foreign candidate.
        admitted.register.file.sync_all()?;admitted.system.file.sync_all()?;
        admitted.register.named(&admitted.system,READY)?;
        admitted.system.file.unlock()?; // short directory SH only; register SH remains
        Ok(admitted)
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
        if self.register.opening_frontier()?!=fs::frontier(0,0,self.record.digest()){
            return Err(invalid("selected register is no longer an empty confirmed prefix"));}
        Ok(())
    }
    /// Only a checked row from the original SYSTEM can select a data root.
    /// The returned row is an observation, not an assertion that all volumes
    /// or the future service are healthy. No unrelated data root is opened.
    pub(in crate::pool) fn observe(&self,id:VolumeId)->io::Result<RegisteredVolume>{
        let cut=HeldDir::open(&self.system.path)?;cut.share()?;self.require_system(&cut)?;
        let i=self.record.info.volumes.iter().position(|v|v.id==id).ok_or_else(||invalid("unknown original volume"))?;
        if i!=0{
            let root=HeldDir::open(&self.record.info.volumes[i].volume.path)?;
            root.share()?;self.original_root(i,&root)?;root.only(None)?;root.require_named()?;
        }
        self.register.named(&cut,READY)?;Ok(self.record.info.volumes[i].clone())
    }
}
