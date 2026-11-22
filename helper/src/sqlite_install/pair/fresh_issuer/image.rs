//! Approve only the exact already positioned one-ELF ZXS product in the
//! private disposable environment. An ELF header check is not cold acceptance.
mod path;
#[cfg(any(test,feature="embedded-zxs"))]
mod asset;
#[cfg(feature="embedded-zxs")]
pub(crate) use asset::EmbeddedInstall;
pub(in crate::sqlite_install::pair) use path::check_image as check_path;
use crate::pinned::{Entity,Mount,installed_regular_reader};
use path::{OriginalPath,no_attribute};
use rustix::fs::{StatVfsMountFlags,fstatvfs};
use std::{fs::File,io,os::unix::fs::{FileExt,MetadataExt},path::Path};

pub(super) const MAX_IMAGE:u64=64<<20;
fn denied()->io::Error{io::Error::new(io::ErrorKind::PermissionDenied,"not the independently approved protected static ZXS ELF")}
#[derive(Clone,Copy,Eq,PartialEq)]
struct ImageStamp{entity:Entity,mount:u64,len:u64,ctime:(i64,i64),mtime:(i64,i64)}
impl ImageStamp{
    fn at(f:&File)->io::Result<Self>{
        let m=f.metadata()?;
        if m.mode()!=0o100_755||m.uid()!=0||m.gid()!=0||m.nlink()!=1||
            !(1..=MAX_IMAGE).contains(&m.len()){return Err(denied());}
        Ok(Self{entity:Entity::of(&m)?,mount:Mount::on(f)?.ok_or_else(denied)?.id,len:m.len(),
            ctime:(m.ctime(),m.ctime_nsec()),mtime:(m.mtime(),m.mtime_nsec())})
    }
}
pub(in crate::sqlite_install::pair) struct Image{original:OriginalPath,reader:File,stamp:ImageStamp}
impl Image{
    pub(in crate::sqlite_install::pair) fn select(path:&Path,expected:&[u8;32],product:Entity)->io::Result<Self>{
        if expected==&[0;32]{return Err(denied());}
        let original=OriginalPath::selected(path,false,product)?;
        let stamp=ImageStamp::at(original.pin())?;
        let reader=installed_regular_reader(original.pin())?;
        let owned=Self{original,reader,stamp};
        owned.require()?;
        if crate::fs_tools::hash_upto(&mut &owned.reader,stamp.len,MAX_IMAGE)?!=*expected{return Err(denied());}
        standalone(stamp.len,|offset,body|owned.reader.read_exact_at(body,offset))?;
        owned.require()?;Ok(owned)
    }
    pub(in crate::sqlite_install::pair) fn require(&self)->io::Result<()>{
        self.original.require()?;
        for x in [c"security.capability",c"system.posix_acl_access"]{no_attribute(&self.reader,x)?;}
        if fstatvfs(&self.reader)?.f_flag.contains(StatVfsMountFlags::NOEXEC)||
            ImageStamp::at(self.original.pin())?!=self.stamp||ImageStamp::at(&self.reader)?!=self.stamp{
            return Err(denied());
        }
        Ok(())
    }
    pub(in crate::sqlite_install::pair) fn path(&self)->&Path{self.original.path()}
    pub(in crate::sqlite_install::pair) fn includes(&self,entity:Entity)->bool{self.original.includes(entity)}
    pub(in crate::sqlite_install::pair) fn entity(&self)->Entity{self.stamp.entity}
    #[cfg(any(test,feature="embedded-zxs"))]
    fn mount(&self)->u64{self.stamp.mount}
    #[cfg(any(test,feature="embedded-zxs"))]
    fn require_same_published(&self,original:&File)->io::Result<()>{
        self.require()?;
        if ImageStamp::at(original)?!=self.stamp{return Err(denied());}
        Ok(())
    }
}

// Same bounded ELF structure as the ZXS consumer's private image gate.
const MAX_PH:u16=128;
const MAX_DYN:u64=128*1024;
const PH_LEN:u64=56;
const ET_EXEC:u16=2;
const ET_DYN:u16=3;
const PT_LOAD:u32=1;
const PT_DYNAMIC:u32=2;
const PT_INTERP:u32=3;
const PT_GNU_STACK:u32=0x6474_e551;
const PF_X:u32=1;

fn get<const N:usize>(b:&[u8],offset:usize)->[u8;N]{
    b[offset..offset+N].try_into().expect("fixed ELF field")
}
fn short(b:&[u8],o:usize)->u16{u16::from_le_bytes(get(b,o))}
fn word(b:&[u8],o:usize)->u32{u32::from_le_bytes(get(b,o))}
fn long(b:&[u8],o:usize)->u64{u64::from_le_bytes(get(b,o))}
fn range(o:u64,n:u64,len:u64)->bool{o.checked_add(n).is_some_and(|end|end<=len)}
fn standalone(len:u64,mut read:impl FnMut(u64,&mut[u8])->io::Result<()>)->io::Result<()>{
    let mut head=[0;64];read(0,&mut head)?;
    let ty=short(&head,16);let machine=short(&head,18);let n=short(&head,56);
    let entry=long(&head,24);let ph=long(&head,32);
    if !head.starts_with(b"\x7fELF\x02\x01\x01")||machine!=62||word(&head,20)!=1||
        !matches!(ty,ET_EXEC|ET_DYN)||entry==0||short(&head,52)!=64||short(&head,54)!=PH_LEN as u16||
        !(1..=MAX_PH).contains(&n)||ph<64||!range(ph,u64::from(n)*PH_LEN,len){
        return Err(denied());
    }
    let mut executable_entry=false;
    let mut dyn_rows=None;
    let mut loads=Vec::with_capacity(usize::from(n));
    for i in 0..u64::from(n){
        let mut p=[0;PH_LEN as usize];read(ph+i*PH_LEN,&mut p)?;
        let k=word(&p,0);let flags=word(&p,4);
        let (o,address,bytes,mem)=(long(&p,8),long(&p,16),long(&p,32),long(&p,40));
        if k==PT_INTERP||k==PT_GNU_STACK&&flags&PF_X!=0{return Err(denied());}
        if k==PT_LOAD{
            if bytes>mem||!range(o,bytes,len)||address.checked_add(mem).is_none(){return Err(denied());}
            if flags&PF_X!=0&&entry>=address&&entry-address<bytes{executable_entry=true;}
            loads.push((o,address,bytes));
        }
        if k==PT_DYNAMIC&&(dyn_rows.replace((o,address,bytes)).is_some()||!(16..=MAX_DYN).contains(&bytes)||
            bytes>mem||!bytes.is_multiple_of(16)||!range(o,bytes,len)){
            return Err(denied());
        }
    }
    if !executable_entry{return Err(denied());}
    let mut pie=false;
    if let Some((o,address,bytes))=dyn_rows{
        if !loads.into_iter().any(|(off,v,len)|address.checked_sub(v)
            .is_some_and(|d|off.checked_add(d)==Some(o)&&range(d,bytes,len))){
            return Err(denied());
        }
        let mut end=false;
        for i in (0..bytes).step_by(16){
            let mut row=[0;16];read(o+i,&mut row)?;
            let tag=long(&row,0);let value=long(&row,8);
            if end&&tag!=0{return Err(denied());}
            match tag {
                0=>end=true,  // DT_NULL
                1|15|29|0x7fff_fffd|0x7fff_ffff|0x6fff_fefa|0x6fff_fefb|0x6fff_fefc=>return Err(denied()),
                0x6fff_fffb=>pie|=value&0x0800_0000!=0, // DT_FLAGS_1 / DF_1_PIE
                _=>{},
            }
        }
        if !end{return Err(denied());}
    }
    if ty==ET_DYN&&!pie{return Err(denied());}
    Ok(())
}

#[cfg(test)]
mod tests{
    use super::*;
    pub(super) fn small()->Vec<u8>{
        let mut b=vec![0;224];
        b[..7].copy_from_slice(b"\x7fELF\x02\x01\x01");
        b[16..18].copy_from_slice(&ET_EXEC.to_le_bytes());b[18..20].copy_from_slice(&62u16.to_le_bytes());
        b[20..24].copy_from_slice(&1u32.to_le_bytes());b[24..32].copy_from_slice(&0x0040_0080_u64.to_le_bytes());
        b[32..40].copy_from_slice(&64u64.to_le_bytes());b[52..54].copy_from_slice(&64u16.to_le_bytes());
        b[54..56].copy_from_slice(&56u16.to_le_bytes());b[56..58].copy_from_slice(&2u16.to_le_bytes());
        b[64..68].copy_from_slice(&PT_LOAD.to_le_bytes());b[68..72].copy_from_slice(&PF_X.to_le_bytes());
        b[80..88].copy_from_slice(&0x0040_0000_u64.to_le_bytes());b[96..104].copy_from_slice(&224u64.to_le_bytes());
        b[104..112].copy_from_slice(&224u64.to_le_bytes());b
    }
    fn check(b:&[u8])->io::Result<()>{
        standalone(b.len() as u64,|o,out|{
            let o=o as usize;
            out.copy_from_slice(b.get(o..o+out.len()).ok_or_else(denied)?);Ok(())
        })
    }
    #[test]
    fn fresh_v17_image_rejects_interpreter_loader_and_ambiguous_programs(){
        let b=small();check(&b).unwrap();
        let mut bad=b.clone();bad[120..124].copy_from_slice(&PT_INTERP.to_le_bytes());assert!(check(&bad).is_err());
        let mut bad=b.clone();bad[68..72].fill(0);assert!(check(&bad).is_err());
        let mut bad=b.clone();bad[16..18].copy_from_slice(&ET_DYN.to_le_bytes());assert!(check(&bad).is_err());
        let mut pie=b;pie[16..18].copy_from_slice(&ET_DYN.to_le_bytes());
        pie[120..124].copy_from_slice(&PT_DYNAMIC.to_le_bytes());
        pie[128..136].copy_from_slice(&176u64.to_le_bytes());pie[136..144].copy_from_slice(&0x0040_00b0_u64.to_le_bytes());
        pie[152..160].copy_from_slice(&32u64.to_le_bytes());pie[160..168].copy_from_slice(&32u64.to_le_bytes());
        pie[176..184].copy_from_slice(&0x6fff_fffbu64.to_le_bytes());
        pie[184..192].copy_from_slice(&0x0800_0000u64.to_le_bytes());check(&pie).unwrap();
        pie[192..200].copy_from_slice(&1u64.to_le_bytes());assert!(check(&pie).is_err());
    }
}
