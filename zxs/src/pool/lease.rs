//! Leases used by ordinary operations. A registration receipt or diagnostic
//! PoolInfo cannot be converted into either of these capabilities.
use super::*;
pub(crate) mod recycle;

#[derive(Clone,Copy)]
pub(crate) enum Access { Read, Incoming, Write }

pub(crate) struct LocalPool {
    pub(crate) system: HeldDir,
    registration: Record,
    pub(crate) access: Access,
}

impl LocalPool {
    #[cfg(test)]
    pub(crate) fn open(path:&Path,access:Access)->io::Result<Self>{
        Self::with_snapshot(path,access).map(|(pool,_,_)|pool)
    }
    pub(crate) fn with_snapshot(path: &Path, access: Access) -> io::Result<(Self,fs::OriginalStamp,crate::ordinary::journal::Journal)> {
        let system = HeldDir::open(path)?;
        match access { Access::Read | Access::Incoming => system.share()?, Access::Write => system.exclusive()? }
        system.require_named()?;
        system.reject_registered_ancestors(&BTreeSet::new())?;
        let Published{stamp,record:registration,journal}=published(&system)?
            .ok_or_else(|| invalid("no original published pool registration"))?;
        if system.record(PENDING, MAX_RECORD)?.is_some() {
            return Err(invalid("pool registration is not unambiguous"));
        }
        system.file.sync_all()?;
        system.confirm_original_name(READY,&stamp,MAX_SYSTEM)?;
        Ok((Self { system, registration, access },stamp,journal))
    }

    pub(crate) fn id(&self)->PoolId{self.registration.info.id}
    pub(crate) fn require_business_op(&self,op:OperationId)->io::Result<()>{
        if op==self.registration.info.initialized_by{return Err(invalid("operation ID already belongs to pool initialization"));}
        Ok(())
    }
    pub(crate) fn snapshot(&self)->io::Result<(fs::OriginalStamp,crate::ordinary::journal::Journal)>{
        let now=published(&self.system)?.ok_or_else(||invalid("original SYSTEM journal missing"))?;
        if now.record.bytes!=self.registration.bytes{return Err(invalid("original SYSTEM genesis changed"));}
        Ok((now.stamp,now.journal))
    }

    /// Open only the requested original ordinary volume. Neither a recycle
    /// root nor an unrelated absent data root is a substitute or a prerequisite.
    pub(crate) fn ordinary(&self, id: VolumeId) -> io::Result<FileVolume<'_>> {
        let index = self.index_for(id,Role::File)?;
        let root = self.open_root(index,Role::File)?;
        let volume=FileVolume{pool:self,index,root};
        volume.require_current()?;
        Ok(volume)
    }
    fn index_for(&self,id:VolumeId,role:Role)->io::Result<usize>{
        self.registration.info.volumes.iter().position(|r|r.id==id && r.volume.role==role)
            .ok_or_else(||invalid("volume is not registered with the required role"))
    }
    // Neither method chooses a role or interprets a directory listing. In
    // particular there is no conversion from a recycle root to FileVolume.
    fn open_root(&self,index:usize,role:Role)->io::Result<HeldDir>{
        let row=self.registration.info.volumes.get(index).filter(|v|v.volume.role==role)
            .ok_or_else(||invalid("wrong original volume role"))?;
        let root=HeldDir::open(&row.volume.path)?;
        match self.access { Access::Read|Access::Incoming=>root.share()?,Access::Write=>root.exclusive()? }
        Ok(root)
    }
    fn check_root(&self,index:usize,role:Role,root:&HeldDir)->io::Result<()>{
        let r=self.registration.info.volumes.get(index).filter(|r|r.volume.role==role)
            .ok_or_else(||invalid("wrong original volume role"))?;
        root.require_named()?;
        if root.entity!=r.volume.entity || root.path.as_os_str()!=r.volume.path.as_os_str(){
            return Err(invalid("selected root is not its registered physical directory"));
        }
        let all=self.registration.info.volumes.iter().map(|r|r.volume.entity).collect();
        root.reject_registered_ancestors(&all)?;
        self.registration.require_root(index,root)?;
        if root.label()?.as_deref()!=Some(self.registration.label(index).as_slice()){
            return Err(invalid("selected root lost its native pool or role binding"));
        }
        Ok(())
    }
}

pub(crate) struct FileVolume<'a> {
    pub(crate) pool: &'a LocalPool,
    index: usize,
    pub(crate) root: HeldDir,
}
impl FileVolume<'_> {
    pub(crate) fn id(&self) -> VolumeId { self.pool.registration.info.volumes[self.index].id }
    pub(crate) fn require_current(&self) -> io::Result<()> {
        self.pool.check_root(self.index,Role::File,&self.root)
    }
}
