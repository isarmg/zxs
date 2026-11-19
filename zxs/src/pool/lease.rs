//! Leases used by ordinary operations. A registration receipt or diagnostic
//! PoolInfo cannot be converted into either of these capabilities.
use super::*;
pub(crate) mod recycle;

#[derive(Clone,Copy)]
pub(crate) enum Access { Read, Incoming, Write }

/// The selected BODY and its native FDs drop before the original-register SH.
pub(crate) struct RegisteredRead<T>{value:T,register:std::fs::File}
impl<T> RegisteredRead<T>{
    #[cfg(test)]
    pub(crate) fn get(&self)->&T{&self.value}
    pub(crate) fn deliver<R>(self,use_value:impl FnOnce(T)->io::Result<R>)->io::Result<R>{
        let result=use_value(self.value);drop(self.register);result
    }
}
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
        Self::admit_system(HeldDir::open(path)?,access,published)
    }
    /// The slow sink only keeps its exact materials and this one regular OFD.
    /// All initial register bytes are from that FD after SH, not another open.
    pub(crate) fn capture_read<T>(path:&Path,select:impl FnOnce(Self,crate::ordinary::journal::Journal)->io::Result<T>)->io::Result<RegisteredRead<T>>{
        let system=HeldDir::open(path)?;let pin=system.shared_original(READY,MAX_SYSTEM as u64)?;
        let(pool,_,journal)=Self::admit_system(system,Access::Read,|dir|{
            let bytes=pin.original_bytes(dir,READY)?;
            read_published(dir,pin.stamp.clone(),&bytes,|_,_|pin.file.sync_all()).map(Some)
        })?;
        let value=select(pool,journal)?;Ok(RegisteredRead{value,register:pin.file})
    }
    fn admit_system(system:HeldDir,access:Access,read:impl FnOnce(&HeldDir)->io::Result<Option<Published>>)
        ->io::Result<(Self,fs::OriginalStamp,crate::ordinary::journal::Journal)>{
        match access { Access::Read | Access::Incoming => system.share()?, Access::Write => system.exclusive()? }
        system.require_named()?;
        system.reject_registered_ancestors(&BTreeSet::new())?;
        let Published{stamp,record:registration,journal}=read(&system)?
            .ok_or_else(|| invalid("no original published pool registration"))?;
        if system.record(PENDING, MAX_RECORD)?.is_some() {
            return Err(invalid("pool registration is not unambiguous"));
        }
        system.file.sync_all()?;
        system.confirm_original_name(READY,&stamp,MAX_SYSTEM)?;
        Ok((Self { system, registration, access },stamp,journal))
    }

    pub(crate) fn journal_registration(&self)->super::JournalRegistration<'_>{super::JournalRegistration(&self.registration)}
    pub(crate) fn id(&self)->PoolId{self.registration.info.id}
    /// A low-water mark is not evidence of missing or altered frames. The
    /// fast path uses the exact named inode stamp from a complete, checked
    /// read of its bytes. A legitimately appended, unconfirmed suffix can
    /// change this stamp; then only another full, original admission suffices.
    pub(crate) fn require_namespace_floor(&self,admitted:&fs::OriginalStamp,frames:usize,value:fs::Frontier)->io::Result<()>{
        let min=self.registration.bytes.len().checked_add(frames).ok_or_else(||invalid("namespace prefix size overflow"))?;
        if admitted.stat.entity()!=self.registration.origin||admitted.birth!=self.registration.origin_birth||
            admitted.stat.size()<(min as u64){return Err(invalid("namespace cut was not read from the complete original register"));}
        if matches!(self.system.read_frontier(READY,admitted,MAX_SYSTEM),Ok(now) if now==value){return Ok(());}
        let(_,now)=self.snapshot()?;
        if now.confirmed_floor()!=value{return Err(invalid("FILE read belongs to another confirmed namespace cut"));}
        Ok(())
    }
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
        let volume=FileVolume{pool:self,index,root,listing:std::sync::OnceLock::new()};
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
    // One read lease samples at most one originally identified directory.
    // A writer and a standalone FILE read never reuse a listing.
    pub(crate) listing:std::sync::OnceLock<crate::ordinary::OrdinaryNameSample>,
}
impl FileVolume<'_> {
    pub(crate) fn id(&self) -> VolumeId { self.pool.registration.info.volumes[self.index].id }
    pub(crate) fn require_current(&self) -> io::Result<()> {
        self.pool.check_root(self.index,Role::File,&self.root)
    }
}
