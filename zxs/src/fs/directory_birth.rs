//! A real ordinary directory's immutable two-name birth proof. The
//! original SQL scope selects every DirectoryId, parent and spelling;
//! these helpers cannot infer authority from a pathname or an inode.
use super::*;
use crate::{DirectoryId,EntryId,OperationId,PoolId,VolumeId};
use std::{collections::BTreeSet,os::unix::fs::FileExt};

pub(crate) const DIRECTORY_MARK:&CStr=c".zxs-new-ffffffffffffffffffffffffffffffff-1";
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(crate) enum DirectoryParent{Root,Directory(DirectoryId)}
impl DirectoryParent{
    pub(crate) fn bytes(self)->[u8;16]{match self{Self::Root=>[0;16],Self::Directory(id)=>id.0}}
    pub(crate) fn from_bytes(id:[u8;16])->Self{if id==[0;16]{Self::Root}else{Self::Directory(DirectoryId(id))}}
}
#[derive(Clone,Debug,Eq,PartialEq)]
pub(crate) struct DirectoryBirth{
    pub(crate) pool:PoolId,
    pub(crate) volume:VolumeId,
    pub(crate) parent:DirectoryParent,
    pub(crate) id:DirectoryId,
    pub(crate) entry:EntryId,
    pub(crate) original:OperationId,
    pub(crate) name:String,
}
fn conflict()->io::Error{io::Error::new(io::ErrorKind::AlreadyExists,"ordinary directory destination is occupied")}
fn unknown()->io::Error{invalid("cannot prove this exact original ordinary directory and its two names")}
impl DirectoryBirth{
    pub(crate) fn valid(&self)->io::Result<()>{
        let selected=crate::display::component(self.name.as_bytes(),255)?;
        if crate::ordinary::portable_birth::Birth::reserved_spelling(selected.as_bytes())||
           [self.pool.0,self.volume.0,self.id.0,self.entry.0,self.original.0].contains(&[0;16])||
           self.original.0==[0xff;16]||self.parent==DirectoryParent::Directory(self.id)||
           matches!(self.parent,DirectoryParent::Directory(d) if d.0==[0;16]){
            return Err(unknown());
        }Ok(())
    }
    pub(crate) fn leaf(&self)->io::Result<CString>{
        self.valid()?;CString::new(self.name.as_bytes()).map_err(|_|unknown())
    }
    pub(crate) fn witness(&self)->CString{CString::new(format!(".zxs-new-{}-7",self.original)).unwrap()}
    pub(crate) fn stage(&self)->CString{CString::new(format!(".zxs-new-{}-4",self.original)).unwrap()}
    fn header(&self)->Vec<u8>{
        let mut b=b"ZXS-PORTABLE-1/DIRECTORY\0".to_vec();
        for id in [self.pool.0,self.volume.0,self.parent.bytes(),self.id.0,self.entry.0,self.original.0]{b.extend(id);}
        b.push(self.name.len() as u8);b.extend(self.name.as_bytes());b
    }
    pub(crate) fn validate_geometry(&self,parent:&HeldDir)->io::Result<()>{
        let limit=ComponentLimit::held(parent)?;
        let budget=NameBudget::from_original(path_budget(&parent.path)?,limit);
        budget.child(&self.witness())?;
        for leaf in [self.leaf()?,self.stage()]{
            let child=NameBudget::from_original(budget.child(&leaf)?,limit);
            child.child(DIRECTORY_MARK)?;
        }
        Ok(())
    }
    pub(crate) fn open(&self,parent:&HeldDir)->io::Result<DirectoryHold>{
        let names=parent.names()?.into_iter().collect::<BTreeSet<_>>();
        self.open_selected(parent,&names)
    }
    pub(crate) fn open_selected(&self,parent:&HeldDir,names:&BTreeSet<Vec<u8>>)->io::Result<DirectoryHold>{
        self.open_at(parent,names,&self.leaf()?)
    }
    /// Reopen only the business spelling that the current SQL selection
    /// specifies, but check it against this immutable birth's original pair.
    pub(crate) fn open_position(&self,parent:&HeldDir,names:&BTreeSet<Vec<u8>>,name:&str)->io::Result<DirectoryHold>{
        self.valid()?;
        let name=crate::display::component(name.as_bytes(),255)?;
        if crate::ordinary::portable_birth::Birth::reserved_spelling(name.as_bytes()){return Err(unknown());}
        self.open_at(parent,names,&CString::new(name).map_err(|_|unknown())?)
    }
    fn open_at(&self,parent:&HeldDir,names:&BTreeSet<Vec<u8>>,name:&CStr)->io::Result<DirectoryHold>{
        let dir=parent.directory_in_sample(name,names)?.ok_or_else(||io::Error::from(io::ErrorKind::NotFound))?;
        let scope=WitnessRead::at(&dir)?;
        let expected=self.header();let witness=self.witness();
        let marker=scope.directory_header(parent,names,DIRECTORY_MARK,&witness,expected.len() as u64)?;
        let mut bytes=vec![0;expected.len()];marker.file().read_exact_at(&mut bytes,0)?;
        if bytes!=expected{return Err(unknown());}
        let h=DirectoryHold{dir,marker,witness};
        h.require(parent)?;Ok(h)
    }
    fn room(&self,names:&BTreeSet<Vec<u8>>,staged:bool)->io::Result<()>{
        let native:usize=names.iter().map(|n|n.len()+1).sum();
        let real=self.leaf()?.to_bytes().len();let stage=self.stage().to_bytes().len();
        let (count,bytes)=if staged{(0,real.saturating_sub(stage))}
          else{(2,self.witness().to_bytes().len()+real.max(stage)+2)};
        if names.len()+count>DIRECTORY_NAMES||native+bytes>DIRECTORY_BYTES{
            return Err(io::Error::new(io::ErrorKind::StorageFull,"ordinary parent has no room for the directory's original names"));
        }Ok(())
    }
    pub(crate) fn new_vacancy(&self,parent:&HeldDir,names:&BTreeSet<Vec<u8>>)->io::Result<()>{
        self.validate_geometry(parent)?;
        for n in [&self.leaf()?,&self.stage(),&self.witness()]{
            if parent.entry_present(n)?!=names.contains(n.to_bytes()){return Err(unknown());}
            if names.contains(n.to_bytes()){return Err(conflict());}
        }
        self.room(names,false)
    }
    /// A later request holding this very parent can release only an
    /// unpublished slot. An unverifiable, partly materialized operation
    /// stays on media and is never used by the later attempt.
    pub(crate) fn pending_shape(&self,parent:&HeldDir,names:&BTreeSet<Vec<u8>>)->io::Result<(bool,bool)>{
        let(real,stage,pin)=(self.leaf()?,self.stage(),self.witness());
        let mut known=[false;3];
        for (i,name) in [&real,&stage,&pin].into_iter().enumerate(){
            let here=parent.entry_present(name)?;
            if names.contains(name.to_bytes())!=here{return Err(unknown());}
            known[i]=here;
        }
        Ok((known[0],known[1]&&known[2]))
    }
    pub(crate) fn resumable(&self,parent:&HeldDir)->io::Result<bool>{
        let names=parent.names()?.into_iter().collect();
        let (terminal,paired)=self.pending_shape(parent,&names)?;
        if terminal&&paired{return Err(unknown());}
        if !terminal&&!paired{return Ok(false);}
        let edge=if terminal{self.leaf()?}else{self.stage()};
        let h=self.open_at(parent,&names,&edge)?;
        h.dir.only(Some(DIRECTORY_MARK))?;h.require_selected(parent,&names)?;
        Ok(true)
    }
    /// Called only after the original SQL pending decision while holding
    /// this actual native parent's exclusive namespace lease. An existing
    /// terminal is never adopted without these exact original dual names.
    pub(crate) fn install(&self,parent:&HeldDir)->io::Result<DirectoryHold>{
        self.validate_geometry(parent)?;WitnessRead::require_parent(parent)?;
        let(real,stage,witness)=(self.leaf()?,self.stage(),self.witness());
        let names=parent.names()?.into_iter().collect::<BTreeSet<_>>();
        let has_terminal=parent.entry_present(&real)?;
        let has_stage=parent.entry_present(&stage)?;
        let has_witness=parent.entry_present(&witness)?;
        for (name,present) in [(&real,has_terminal),(&stage,has_stage),(&witness,has_witness)]{
            if names.contains(name.to_bytes())!=present{return Err(unknown());}
        }
        if has_terminal{
            if has_stage||!has_witness{return Err(conflict());}
            let h=self.open_at(parent,&names,&real)?;
            h.dir.only(Some(DIRECTORY_MARK))?;
            h.sync_before_publish(parent)?;return Ok(h);
        }
        if has_stage!=has_witness{return Err(unknown());} // no blind recovery of a partial pair
        self.room(&names,has_stage)?;
        let written=if !has_stage{
            let staged=parent.new_child_directory(&stage)?;staged.only(None)?;
            let fd=unsafe{zxs_create_new_regular(staged.file.as_raw_fd(),DIRECTORY_MARK.as_ptr())};
            if fd<0{return Err(io::Error::last_os_error());}
            let marked=unsafe{File::from_raw_fd(fd)};
            let m=marked.metadata()?;
            if !m.is_file()||m.nlink()!=1||m.len()!=0||m.uid()!=local_uid()||m.mode()&0o7777!=0o600||!staged.shares_file_mount(&marked)?{
                return Err(unknown());
            }
            marked.write_all_at(&self.header(),0)?;marked.sync_all()?;
            // Both names are controlled single components in real held
            // parents; hard-linking never replaces an existing name.
            os_result(unsafe{linkat(staged.file.as_raw_fd(),DIRECTORY_MARK.as_ptr(),parent.file.as_raw_fd(),witness.as_ptr(),0)})?;
            marked.sync_all()?;staged.file.sync_all()?;parent.file.sync_all()?;
            Some((staged.entity,LiveStamp::read(&marked)?))
        }else{None};
        let sample=parent.names()?.into_iter().collect::<BTreeSet<_>>();
        let h=self.open_at(parent,&sample,&stage)?;h.dir.only(Some(DIRECTORY_MARK))?;
        if written.is_some_and(|v|v!=(h.dir.entity,h.marker.stamp())){return Err(unknown());}
        self.room(&sample,true)?;h.sync_before_publish(parent)?;
        parent.require_named()?;h.require(parent)?;
        os_result(unsafe{zxs_rename_fresh(parent.file.as_raw_fd(),stage.as_ptr(),parent.file.as_raw_fd(),real.as_ptr())})?;
        parent.file.sync_all()?;
        let installed=self.open(parent)?;
        if installed.dir.entity!=h.dir.entity||installed.marker.stamp()!=h.marker.stamp()||parent.entry_present(&stage)?{
            return Err(unknown());
        }
        installed.sync_before_publish(parent)?;Ok(installed)
    }
}
pub(crate) struct DirectoryHold{dir:HeldDir,marker:HeldBytes,witness:CString}
impl DirectoryHold{
    pub(crate) fn duplicate(&self)->io::Result<Self>{
        Ok(Self{dir:self.dir.duplicate()?,marker:self.marker.duplicate()?,witness:self.witness.clone()})
    }
    pub(crate) fn directory(&self)->&HeldDir{&self.dir}
    pub(crate) fn require(&self,parent:&HeldDir)->io::Result<()>{
        if self.dir.path.parent()!=Some(parent.path.as_path())||!parent.same_mount(&self.dir)?{return Err(unknown());}
        WitnessRead::require_parent(parent)?;WitnessRead::require_parent(&self.dir)?;
        self.marker.require_name(&self.dir,DIRECTORY_MARK)?;self.marker.require_name(parent,&self.witness)
    }
    pub(crate) fn require_selected(&self,parent:&HeldDir,names:&BTreeSet<Vec<u8>>)->io::Result<()>{
        if !self.dir.path.file_name().is_some_and(|name|names.contains(name.as_bytes()))||
           !names.contains(self.witness.to_bytes()){return Err(unknown());}
        self.require(parent)
    }
    pub(crate) fn require_newborn(&self,parent:&HeldDir)->io::Result<()>{
        self.dir.only(Some(DIRECTORY_MARK))?;
        self.require_selected(parent,&parent.names()?.into_iter().collect())
    }
    fn sync_before_publish(&self,parent:&HeldDir)->io::Result<()>{
        self.require(parent)?;self.marker.file().sync_all()?;self.dir.file.sync_all()?;parent.file.sync_all()?;self.require(parent)
    }
}
unsafe extern "C"{
    fn zxs_create_new_regular(parent:c_int,name:*const c_char)->c_int;
    fn linkat(old:c_int,name:*const c_char,new:c_int,witness:*const c_char,flags:c_int)->c_int;
}

#[path="directory_rename.rs"]
mod rename;
pub(crate) use rename::{DirectoryRename,DirectoryRenameSide};

#[cfg(test)]
mod tests{
    use super::*;
    use crate::test_support::PrivateTemp;
    fn slot(parent:DirectoryParent,id:u8,name:&str)->DirectoryBirth{
        DirectoryBirth{pool:PoolId([1;16]),volume:VolumeId([2;16]),parent,id:DirectoryId([id;16]),
            entry:EntryId([id+32;16]),original:OperationId([id+64;16]),name:name.into()}
    }
    #[test]
    fn fresh_and_cold_stage_charge_only_the_exact_peak_in_the_native_parent(){
        let all:BTreeSet<_>=(0..DIRECTORY_NAMES-1).map(|n|n.to_string().into_bytes()).collect();
        let long=slot(DirectoryParent::Root,3,&"x".repeat(180));
        assert_eq!(long.room(&all,false).unwrap_err().kind(),io::ErrorKind::StorageFull);
        let mut stage=all.clone();stage.insert(long.stage().into_bytes());
        assert!(long.room(&stage,true).is_ok());
        let mut bytes:BTreeSet<_>=(0..2047u16).map(|i|{let mut v=vec![b'x';255];v[..2].copy_from_slice(&i.to_be_bytes());v}).collect();
        bytes.insert(long.stage().into_bytes());bytes.insert(long.witness().into_bytes());
        assert!(long.room(&bytes,true).is_ok());
        bytes.insert(vec![b'y';100]);
        assert_eq!(long.room(&bytes,true).unwrap_err().kind(),io::ErrorKind::StorageFull);
    }
    #[test]
    #[ignore="source-only native directory inode and exact outer marker fixture, no mounted user disks"]
    fn traced_real_child_names_and_their_two_original_links_bind_parent_identity(){
        let t=PrivateTemp::under(&std::env::temp_dir());
        let root=HeldDir::trusted_storage(&t.0).unwrap();root.exclusive().unwrap();
        let p=slot(DirectoryParent::Root,3,"Reports");let second=slot(DirectoryParent::Root,4,"reports");
        let held=p.install(&root).unwrap();
        assert_eq!(p.install(&root).unwrap().dir.entity,held.dir.entity);
        assert_ne!(second.install(&root).unwrap().dir.entity,held.dir.entity);
        assert!(!t.0.join(p.stage().to_str().unwrap()).exists());

        let child=slot(DirectoryParent::Directory(p.id),5,"2027");
        held.directory().exclusive().unwrap();
        let next=child.install(held.directory()).unwrap();
        assert_eq!(next.dir.path,t.0.join("Reports/2027"));
        assert!(child.open(&root).is_err());
        let mut wrong=child.clone();wrong.parent=DirectoryParent::Directory(second.id);
        assert!(wrong.open(held.directory()).is_err());
        let pin=t.0.join(p.witness().to_str().unwrap());let hidden=t.0.join("parked-pin");
        std::fs::rename(&pin,&hidden).unwrap();
        let rogue=std::fs::OpenOptions::new().write(true).create_new(true).mode(0o600).open(&pin).unwrap();
        rogue.write_all_at(&p.header(),0).unwrap();rogue.sync_all().unwrap();drop(rogue);
        assert!(p.open(&root).is_err());assert!(!held.require(&root).is_ok());
        std::fs::remove_file(&pin).unwrap();std::fs::rename(&hidden,&pin).unwrap();
        assert_eq!(p.open(&root).unwrap().dir.entity,held.dir.entity);

        let bad=slot(DirectoryParent::Root,6,"Guessed");let raw=t.0.join("Guessed");
        std::fs::DirBuilder::new().mode(0o700).create(&raw).unwrap();
        assert!(bad.install(&root).is_err());assert!(raw.is_dir());
        let incomplete=slot(DirectoryParent::Root,7,"Partial");
        let half=t.0.join(incomplete.stage().to_str().unwrap());
        std::fs::DirBuilder::new().mode(0o700).create(&half).unwrap();
        assert!(incomplete.install(&root).is_err()&&!t.0.join("Partial").exists()&&half.is_dir());
    }
}
