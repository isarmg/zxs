//! A plain immutable payload and one companion checksum table. Its page
//! directory is bound by its issuing operation. Only overlapping pages and
//! blocks are read for a slice; both are verified before their bytes are used.
use super::Material;
use crate::{PoolId, fs::invalid, hash::{self,Sha256}};
use crate::ordinary::{content::BLOCK, output::CheckedOutput};
use std::{fs::File, io::{self,Write}, os::unix::fs::{FileExt,MetadataExt}};

const MAGIC: &[u8] = b"ZXS-PORTABLE-1/BLOCK-TABLE\0";
const PER_PAGE: u64 = 1024;
const HEADER: usize = MAGIC.len()+3*16+4+8+4+4;
const MAX_DIRECTORY: usize = 8*1024*1024;
const _: () = assert!(BLOCK == 4*1024*1024);

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(in crate::ordinary) struct Ref { pub(in crate::ordinary) pool: PoolId, pub(in crate::ordinary) material: Material, directory_sha: [u8;32] }
impl Ref {
    /// Identity and hash from the material's original issuance, not from a
    /// filename, size, self-described tag, or a hash of the data alone.
    pub(in crate::ordinary) fn declared(pool:PoolId, material:Material, directory_sha:[u8;32])->io::Result<Self> {
        Geometry::new(pool,material)?;Ok(Self{pool,material,directory_sha})
    }
    pub(in crate::ordinary) fn directory_sha(self)->[u8;32]{self.directory_sha}
    /// The checksum page and block containing the first physical byte are the
    /// largest needed by this contiguous range; only the last can be shorter.
    pub(super) fn read_bytes(self,offset:u64)->io::Result<(usize,usize,usize)>{
        if offset>=self.material.size{return Err(invalid("material read starts past the source"));}
        let g=Geometry::new(self.pool,self.material)?;
        let i=offset/BLOCK;
        Ok((g.pages*32,g.page_bytes(i/PER_PAGE),(self.material.size-i*BLOCK).min(BLOCK) as usize))
    }
}

#[derive(Clone,Copy)]
struct Geometry{blocks:u64,pages:usize,dir_at:u64,table_bytes:u64}
impl Geometry{
    fn new(pool:PoolId,m:Material)->io::Result<Self>{
        if pool.0==[0;16]{return Err(invalid("missing pool for portable material"));}
        // Material::declared already bounded length by the native offset range.
        let blocks=m.size.div_ceil(BLOCK);
        let pages=usize::try_from(blocks.div_ceil(PER_PAGE)).ok().filter(|n|*n<=MAX_DIRECTORY/32)
            .ok_or_else(||invalid("portable checksum directory exceeds its byte budget"))?;
        let dir_at=HEADER as u64+blocks*32;
        Ok(Self{blocks,pages,dir_at,table_bytes:dir_at+pages as u64*32})
    }
    fn header(pool:PoolId,m:Material)->Vec<u8>{
        let mut b=Vec::with_capacity(HEADER);b.extend_from_slice(MAGIC);
        for id in [pool.0,m.file.0,m.issuer.0]{b.extend_from_slice(&id);}
        b.extend_from_slice(&m.slot.to_be_bytes());b.extend_from_slice(&m.size.to_be_bytes());
        b.extend_from_slice(&(BLOCK as u32).to_be_bytes());b.extend_from_slice(&(PER_PAGE as u32).to_be_bytes());b
    }
    fn page_bytes(self,p:u64)->usize{(self.blocks-p*PER_PAGE).min(PER_PAGE) as usize*32}
}

// Only volatility evidence for these live FDs; not an on-disk identity.
#[derive(Clone,Copy,Eq,PartialEq)]
pub(super) struct Stamp { dev:u64, ino:u64, len:u64, mtime:(i64,i64), ctime:(i64,i64) }
impl Stamp{
    pub(super) fn read(f:&File)->io::Result<Self>{
        let s=f.metadata()?;if !s.is_file(){return Err(invalid("portable bytes require a regular file"));}
        Ok(Self{dev:s.dev(),ino:s.ino(),len:s.len(),mtime:(s.mtime(),s.mtime_nsec()),ctime:(s.ctime(),s.ctime_nsec())})
    }
    pub(super) fn require(self,f:&File)->io::Result<()>{
        if Self::read(f)?!=self{return Err(invalid("portable ordinary file changed while held"));}Ok(())
    }
    fn same(self,o:Self)->bool{(self.dev,self.ino)==(o.dev,o.ino)}
}
fn dir_hash(header:&[u8],directory:&[u8])->io::Result<[u8;32]>{
    let mut h=Sha256::new();h.update(header)?;h.update(directory)?;Ok(h.finish())
}
fn scratch(n:usize)->io::Result<Vec<u8>>{
    let mut b=Vec::new();b.try_reserve_exact(n).map_err(|_|invalid("insufficient memory for portable checksum metadata"))?;
    Ok(b)
}

/// The caller supplies its freshly created, private destination. Never write
/// via an arbitrary sink here: a second FD (or hard link) may alias the RAW.
/// An error leaves only this candidate for its owner to handle. This is not
/// publication; even a successful sync is not an independent named readback.
pub(in crate::ordinary) fn write_candidate(pool:PoolId,m:Material,raw:&File,target:&File)->io::Result<Ref>{
    let g=Geometry::new(pool,m)?;let before=Stamp::read(raw)?;let dest=Stamp::read(target)?;
    if before.len!=m.size||dest.len!=0||before.same(dest){
        return Err(invalid("checksum destination must be a separate empty regular file and RAW must have its original extent"));}
    let header=Geometry::header(pool,m);let mut dir=scratch(g.pages*32)?;
    let mut page=scratch(g.page_bytes(0))?;let mut data=scratch(m.size.min(BLOCK) as usize)?;
    data.resize(m.size.min(BLOCK) as usize,0);
    before.require(raw)?;dest.require(target)?;
    target.write_all_at(&header,0)?;
    for p in 0..g.pages as u64{
        page.clear();
        for i in p*PER_PAGE..((p+1)*PER_PAGE).min(g.blocks){
            let at=i*BLOCK;let n=(m.size-at).min(BLOCK) as usize;
            raw.read_exact_at(&mut data[..n],at)?;page.extend_from_slice(&hash::digest(&data[..n])?);
        }
        before.require(raw)?;target.write_all_at(&page,HEADER as u64+p*PER_PAGE*32)?;
        dir.extend_from_slice(&hash::digest(&page)?);
    }
    let directory_sha=dir_hash(&header,&dir)?;target.write_all_at(&dir,g.dir_at)?;
    target.sync_all()?;let after=Stamp::read(target)?;
    if !after.same(dest)||after.len!=g.table_bytes{return Err(invalid("checksum output changed or has wrong length"));}
    before.require(raw)?;Ok(Ref{pool,material:m,directory_sha})
}

/// One buffer for each kind, shared only within a single request. The page
/// cache includes the actual table's live stamp, not just its payload digest.
pub(super) struct Buffers{page:Vec<u8>,block:Vec<u8>,page_key:Option<(Ref,Stamp,u64)>}
impl Buffers{
    pub(super) fn new(page:usize,block:usize)->io::Result<Self>{
        let mut p=scratch(page)?;p.resize(page,0);let mut b=scratch(block)?;b.resize(block,0);
        Ok(Self{page:p,block:b,page_key:None})
    }
}

pub(super) struct Reader{
    original:Ref,raw:File,table:File,raw_stamp:Stamp,table_stamp:Stamp,g:Geometry,directory:Vec<u8>,
}
impl Reader{
    /// Enclosing FILE/name/permission selection belongs to the caller.
    /// This method checks the actual byte-index against its external original
    /// reference. It never treats this reference or an identical clone as
    /// independent evidence that a particular physical name is authorized.
    pub(super) fn open(original:Ref,raw:File,table:File)->io::Result<Self>{
        let m=original.material;let g=Geometry::new(original.pool,m)?;
        let raw_stamp=Stamp::read(&raw)?;let table_stamp=Stamp::read(&table)?;
        if raw_stamp.same(table_stamp)||raw_stamp.len!=m.size||table_stamp.len!=g.table_bytes{
            return Err(invalid("portable payload and index are not the expected two regular extents"));}
        let mut header=[0;HEADER];table.read_exact_at(&mut header,0)?;
        if header.as_slice()!=Geometry::header(original.pool,m).as_slice(){return Err(invalid("wrong portable material header"));}
        let mut directory=scratch(g.pages*32)?;directory.resize(g.pages*32,0);
        table.read_exact_at(&mut directory,g.dir_at)?;
        if dir_hash(&header,&directory)?!=original.directory_sha{return Err(invalid("wrong original portable checksum directory"));}
        raw_stamp.require(&raw)?;table_stamp.require(&table)?;
        Ok(Self{original,raw,table,raw_stamp,table_stamp,g,directory})
    }
    pub(super) fn unchanged(&self)->io::Result<()>{self.raw_stamp.require(&self.raw)?;self.table_stamp.require(&self.table)}
    fn block_hash(&self,i:u64,buffers:&mut Buffers)->io::Result<[u8;32]>{
        let p=i/PER_PAGE;let key=(self.original,self.table_stamp,p);
        let n=self.g.page_bytes(p);let bytes=buffers.page.get_mut(..n)
            .ok_or_else(||invalid("portable checksum page exceeds the request budget"))?;
        if buffers.page_key!=Some(key){
            buffers.page_key=None;self.table.read_exact_at(bytes,HEADER as u64+p*PER_PAGE*32)?;
            let start=p as usize*32;
            if hash::digest(bytes)?.as_slice()!=&self.directory[start..start+32]{return Err(invalid("changed portable checksum page"));}
            buffers.page_key=Some(key);
        }
        let off=(i%PER_PAGE) as usize*32;let mut hash=[0;32];hash.copy_from_slice(&bytes[off..off+32]);Ok(hash)
    }
    pub(super) fn range(&self,at:u64,n:u64,buffers:&mut Buffers,out:&mut impl Write)->io::Result<()>{
        let size=self.original.material.size;
        let end=at.checked_add(n).filter(|v|*v<=size).ok_or_else(||invalid("range leaves the exact portable material"))?;
        self.unchanged()?;if n==0{return Ok(());}
        for i in at/BLOCK..=(end-1)/BLOCK{
            let expected=self.block_hash(i,buffers)?;
            let start=i*BLOCK;let count=(size-start).min(BLOCK) as usize;
            let bytes=buffers.block.get_mut(..count).ok_or_else(||invalid("portable block exceeds the request budget"))?;
            self.raw.read_exact_at(bytes,start)?;
            if hash::digest(bytes)?!=expected{return Err(invalid("portable payload block differs from original"));}
            let lo=at.saturating_sub(start) as usize;let hi=(end-start).min(count as u64) as usize;
            CheckedOutput{out:&mut *out,check:||self.unchanged()}.write_all(&bytes[lo..hi])?;
        }
        self.unchanged()
    }
}
