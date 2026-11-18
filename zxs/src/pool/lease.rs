//! Leases used by ordinary operations. A registration receipt or diagnostic
//! PoolInfo cannot be converted into either of these capabilities.
use super::*;

#[derive(Clone,Copy)]
pub(crate) enum Access { Read, Write }

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
        match access { Access::Read => system.share()?, Access::Write => system.exclusive()? }
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
        let (index, volume) = self.registration.info.volumes.iter().enumerate()
            .find(|(_,v)| v.id == id)
            .ok_or_else(|| invalid("volume is not in the original pool registration"))?;
        if volume.volume.role != Role::File {
            return Err(invalid("ordinary FILE operations require an ordinary FILE volume"));
        }
        let root = HeldDir::open(&volume.volume.path)?;
        match self.access { Access::Read => root.share()?, Access::Write => root.exclusive()? }
        let result = FileVolume { pool: self, index, root };
        result.require_current()?;
        Ok(result)
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
        let original = &self.pool.registration.info.volumes[self.index].volume;
        self.root.require_named()?;
        if self.root.entity != original.entity {
            return Err(invalid("selected FILE root is not its original physical directory"));
        }
        let entities = self.pool.registration.info.volumes.iter().map(|r|r.volume.entity).collect();
        self.root.reject_registered_ancestors(&entities)?;
        self.pool.registration.require_root(self.index, &self.root)?;
        if self.root.label()?.as_deref() != Some(self.pool.registration.label(self.index).as_slice()) {
            return Err(invalid("selected ordinary root has lost its native role binding"));
        }
        Ok(())
    }
}
