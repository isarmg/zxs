//! The single bounded SYSTEM genesis record. A label identifies one volume;
//! it contains neither a file inventory nor a location/transaction database.
use super::*;
use std::ffi::OsString;
use std::os::unix::ffi::{OsStrExt, OsStringExt};
use std::collections::BTreeSet;
const MAGIC:&[u8]=b"ZXS/FILE-POOL\0";
pub(super) const MAX_RECORD:usize=(MAX_FILE_ROOTS+2)*(4096+64+fs::MAX_BIRTH)+256;
#[derive(Clone)]
pub(super) struct Record {
    pub(super) info:PoolInfo,
    pub(super) bytes:Vec<u8>,
    digest:[u8;32],
    pub(super) origin:Entity,
    pub(super) actor_uid:u32,
    pub(super) origin_birth:fs::Birth,
    roots:Vec<fs::Birth>,
}
fn tag(role:Role)->u8 {match role{Role::System=>0,Role::Recycle=>1,Role::File=>2}}
fn invalid_record()->io::Error {invalid("unsupported, damaged or noncanonical SYSTEM registration")}

impl Record {
    pub(super) fn observe_roots(selection:&Selection)->io::Result<Vec<fs::Birth>>{
        let roots=selection.roots.iter().map(|(_,dir)|fs::Birth::read(&dir.file)).collect::<io::Result<Vec<_>>>()?;
        Self::distinct_recycle(&roots)?;
        Ok(roots)
    }
    fn distinct_recycle(roots:&[fs::Birth])->io::Result<()>{
        if roots.len()<3 || roots.iter().enumerate().any(|(i,r)|i!=1 && r.filesystem()==roots[1].filesystem()){
            return Err(invalid("recycle requires a separately identified filesystem"));
        }
        Ok(())
    }
    pub(super) fn require_root(&self,index:usize,dir:&HeldDir)->io::Result<()>{
        self.roots[index].require(&fs::Birth::read(&dir.file)?)
    }
    pub(super) fn new(selection:&Selection, op:OperationId, origin:Entity, origin_birth:fs::Birth, roots:Vec<fs::Birth>)->io::Result<Self>{
        let mut volumes=Vec::with_capacity(selection.roots.len());
        let mut taken=BTreeSet::new();
        for volume in selection.view(){
            let id=VolumeId::new()?;
            if !taken.insert(id){return Err(io::Error::other("duplicate random volume identity"));}
            volumes.push(RegisteredVolume {id,volume});
        }
        let info=PoolInfo {id:PoolId::new()?,initialized_by:op,volumes};
        let mut bytes=Vec::new(); bytes.extend_from_slice(MAGIC);
        bytes.extend_from_slice(&info.id.0);bytes.extend_from_slice(&op.0);
        let actor_uid=crate::fs::local_uid();
        bytes.extend_from_slice(&actor_uid.to_be_bytes());
        bytes.extend_from_slice(&origin.device.to_be_bytes()); bytes.extend_from_slice(&origin.inode.to_be_bytes());
        let encode_birth=|out:&mut Vec<u8>,b:&fs::Birth|{out.push(b.bytes().len() as u8);out.extend_from_slice(b.bytes());};
        encode_birth(&mut bytes,&origin_birth);
        bytes.extend_from_slice(&(info.volumes.len() as u16).to_be_bytes());
        for (reg,birth) in info.volumes.iter().zip(&roots) {
            let v=&reg.volume; bytes.push(tag(v.role)); bytes.extend_from_slice(&reg.id.0);
            bytes.extend_from_slice(&v.entity.device.to_be_bytes());bytes.extend_from_slice(&v.entity.inode.to_be_bytes());encode_birth(&mut bytes,birth);
            let path=v.path.as_os_str().as_bytes();
            let len=u16::try_from(path.len()).map_err(|_|invalid_record())?;
            bytes.extend_from_slice(&len.to_be_bytes()); bytes.extend_from_slice(path);
        }
        let digest=crate::hash::digest(&bytes)?;
        bytes.extend_from_slice(&digest);
        Ok(Self{info,bytes,digest,origin,actor_uid,origin_birth,roots})
    }

    pub(super) fn read(bytes:Vec<u8>)->io::Result<Self>{
        let (record,tail)=Self::prefix(&bytes)?;
        if !tail.is_empty(){return Err(invalid_record());}
        Ok(record)
    }
    // REGISTRATION begins with exactly the same self-delimiting immutable
    // genesis as INITIALIZING. Business frames follow its original checksum;
    // they are never part of the root label and never replace the genesis.
    pub(super) fn prefix(image:&[u8])->io::Result<(Self,&[u8])>{
        let mut input=image.strip_prefix(MAGIC).ok_or_else(invalid_record)?;
        fn take<'a>(input:&mut &'a [u8],len:usize)->io::Result<&'a [u8]> {
            let all=*input;
            let (part,rest)=all.split_at_checked(len).ok_or_else(invalid_record)?;
            *input=rest;Ok(part)
        }
        fn id(input:&mut &[u8])->io::Result<[u8;16]> {
            let id:[u8;16]=take(input,16)?.try_into().expect("length checked");
            if id==[0;16]{Err(invalid_record())}else{Ok(id)}
        }
        fn num(input:&mut &[u8])->io::Result<u64>{
            Ok(u64::from_be_bytes(take(input,8)?.try_into().expect("length checked")))
        }
        fn short(input:&mut &[u8])->io::Result<usize>{
            Ok(u16::from_be_bytes(take(input,2)?.try_into().expect("length checked")) as usize)
        }
        fn birth(input:&mut &[u8])->io::Result<fs::Birth>{
            let n=usize::from(take(input,1)?[0]);fs::Birth::decode(take(input,n)?)
        }
        let pool=PoolId(id(&mut input)?); let op=OperationId(id(&mut input)?);
        let actor_uid=u32::from_be_bytes(take(&mut input,4)?.try_into().expect("four bytes"));
        let origin=Entity{device:num(&mut input)?,inode:num(&mut input)?};
        if origin.inode==0{return Err(invalid_record());}
        let origin_birth=birth(&mut input)?;
        let count=short(&mut input)?;
        if !(3..=MAX_FILE_ROOTS+2).contains(&count){return Err(invalid_record());}
        let mut volumes=Vec::with_capacity(count);
        let mut roots=Vec::with_capacity(count);
        let mut ids=BTreeSet::new();let mut entities=BTreeSet::new();let mut paths=BTreeSet::new();
        let mut last_file:Option<PathBuf>=None;
        for index in 0..count{
            let role=match index {0=>Role::System,1=>Role::Recycle,_=>Role::File};
            if take(&mut input,1)?[0]!=tag(role){return Err(invalid_record());}
            let volume_id=VolumeId(id(&mut input)?);
            let entity=Entity {device:num(&mut input)?,inode:num(&mut input)?};
            roots.push(birth(&mut input)?);
            let length=short(&mut input)?;
            if length==0 || length>4096{return Err(invalid_record());}
            let path=PathBuf::from(OsString::from_vec(take(&mut input,length)?.to_vec()));
            crate::fs::check_path(&path)?;
            if entity.inode==0 || !entities.insert(entity) || !paths.insert(path.clone()) || !ids.insert(volume_id) {
                return Err(invalid_record());
            }
            if role==Role::File {
                if last_file.as_ref().is_some_and(|old|old.as_os_str().as_bytes()>=path.as_os_str().as_bytes()){return Err(invalid_record());}
                last_file=Some(path.clone());
            }
            volumes.push(RegisteredVolume{id:volume_id,volume:ObservedVolume {role,path,entity}});
        }
        let end=image.len()-input.len();
        if end+32>MAX_RECORD{return Err(invalid_record());}
        let digest=crate::hash::digest(&image[..end])?;
        if take(&mut input,32)?!=digest.as_slice(){return Err(invalid_record());}
        let bytes=image[..end+32].to_vec();
        Self::distinct_recycle(&roots)?;
        if origin.device!=volumes[0].volume.entity.device || origin_birth.filesystem()!=roots[0].filesystem(){return Err(invalid_record());}
        let recycle=volumes[1].volume.entity.device;
        if volumes.iter().any(|v|v.volume.role!=Role::Recycle && v.volume.entity.device==recycle) {
            return Err(invalid_record());
        }
        Ok((Self{info:PoolInfo{id:pool,initialized_by:op,volumes},bytes,digest,origin,actor_uid,origin_birth,roots},input))
    }

    pub(super) fn digest(&self)->[u8;32]{self.digest}
    pub(super) fn original_root(&self,i:usize)->(Entity,&fs::Birth){
        (self.info.volumes[i].volume.entity,&self.roots[i])
    }
    // A business directory is never the SYSTEM register or any registered
    // root, even when ordinary and SYSTEM roots share a filesystem.
    pub(super) fn directory_child(&self,volume:VolumeId,uid:u32,entity:Entity,birth:&fs::Birth)->bool{
        self.allows_file(volume,uid,entity,birth,false)&&self.origin!=entity && &self.origin_birth!=birth &&
            self.info.volumes.iter().zip(&self.roots).all(|(r,b)|r.volume.entity!=entity&&b!=birth)
    }
    pub(super) fn allows_file(&self,volume:VolumeId,uid:u32,entity:Entity,birth:&fs::Birth,root:bool)->bool{
        uid==self.actor_uid && self.info.volumes.iter().enumerate().any(|(i,r)|
            r.id==volume && r.volume.role==Role::File && r.volume.entity.device==entity.device
                && self.roots[i].filesystem()==birth.filesystem() && (!root || (r.volume.entity==entity && &self.roots[i]==birth)))
    }

    pub(super) fn label(&self,index:usize)->Vec<u8>{
        let root=&self.info.volumes[index];
        let mut bytes=Vec::new();bytes.extend_from_slice(b"ZXS/VOLUME\0");
        bytes.push(tag(root.volume.role));bytes.extend_from_slice(&self.info.id.0);
        bytes.extend_from_slice(&root.id.0);bytes.extend_from_slice(&self.info.initialized_by.0);
        // Only the SYSTEM label authenticates which full, immutable original
        // registration its private file must contain, before opening any saved path.
        if root.volume.role==Role::System { bytes.extend_from_slice(&self.digest); }
        bytes
    }
    pub(super) fn matches_request(&self,system:&Path,recycle:&Path,files:&[PathBuf],op:OperationId)->io::Result<()>{
        let expected=std::iter::once(system).chain(std::iter::once(recycle)).chain(files.iter().map(PathBuf::as_path));
        if self.actor_uid!=crate::fs::local_uid() || op!=self.info.initialized_by || files.len()+2!=self.info.volumes.len()
            || !self.info.volumes.iter().map(|r|r.volume.path.as_os_str()).eq(expected.map(Path::as_os_str)) {
            return Err(invalid("original initializer or original request does not match"));
        }
        Ok(())
    }
    pub(super) fn matches(&self,selection:&Selection,op:OperationId)->io::Result<()>{
        if self.actor_uid!=crate::fs::local_uid() || op!=self.info.initialized_by || self.info.volumes.len()!=selection.roots.len() ||
            self.info.volumes.iter().zip(&selection.roots).any(|(r,(role,d))| {
                r.volume.role!=*role || r.volume.entity!=d.entity || r.volume.path.as_os_str()!=d.path.as_os_str()
            }){
            return Err(invalid("request differs from original registration, named roots or operation"));
        }
        Ok(())
    }
}
