//! Stable native locking/stat projection, not access authority. Check the
//! original 16-byte BLOB via the existing unique root_entry.file_id index.
//! A tentative, retained or otherwise invisible birth still owns its number.
use super::{Connection,check};
use crate::{FileId,PoolId,fs::invalid};
use std::io;

#[derive(Debug,Copy,Clone,PartialEq,Eq)]
pub(crate) struct NativeKey{device:u64,inode:u64,extended:u64}
impl NativeKey{
    /// Exact storage witness words. The protocol exposes only its unique
    /// number in the fixed logical volume, not the host inode or pool words.
    pub(crate) fn words(self)->(u64,u64,u64){(self.device,self.inode,self.extended)}
}

/// The SMB information number, not an OPEN key or a host volume/file ID.
/// Only the original unique SQL owner may construct it.
#[derive(Clone,Copy,Eq,PartialEq)]
pub(crate) struct SmbFileIndex(u64);
impl SmbFileIndex{
    pub(crate) fn number(self)->u64{self.0}
    pub(crate) fn qfid(self)->(u64,u64){(self.0,1)} // the one ZXS logical volume
}
fn number(id:FileId)->Option<u64>{
    let mut part=[0;8];part.copy_from_slice(&id.0[..8]);let n=u64::from_be_bytes(part);
    (n>255&&n!=u64::MAX).then_some(n)
}
enum Owners{None,One(FileId),Multiple}
fn owners(db:&Connection,id:FileId)->io::Result<Owners>{
    let mut lo=id.0;lo[8..].fill(0);let mut hi=id.0;hi[8..].fill(255);
    // Compare complete BLOB endpoints. substr(), signed casts or scanning
    // this share/visible namespace would lose either the index or owners.
    let q=db.prepare(c"SELECT file_id FROM main.root_entry WHERE file_id>=?1 AND file_id<=?2 ORDER BY file_id LIMIT 2")?;
    q.bind_bytes(1,&lo)?;q.bind_bytes(2,&hi)?;
    let mut first=None;
    loop{match q.step(){
        101=>return Ok(first.map_or(Owners::None,Owners::One)),
        100=>{let got=FileId(q.array(0)?);if got.0[..8]!=id.0[..8]{return Err(invalid("wrong FILE number index result"));}
            if first.replace(got).is_some(){return Ok(Owners::Multiple);}}
        rc=>{check(rc)?;return Err(invalid("cannot establish FILE number ownership"));}
    }}
}
/// Only for a not-yet-reserved original inside the CREATE writer transaction.
pub(super) fn choose(db:&Connection,candidates:&[FileId;8])->io::Result<FileId>{
    for &id in candidates{if number(id).is_some()&&matches!(owners(db,id)?,Owners::None){return Ok(id);}}
    Err(io::Error::new(io::ErrorKind::WouldBlock,"no available FILE number in this allocation; nothing reserved"))
}
/// Called only after current SQL authority, the complete birth and the real
/// FILE were verified in the same cut. Never reissue an ambiguous old name.
pub(super) fn original(db:&Connection,pool:PoolId,file:FileId)->io::Result<NativeKey>{
    if !matches!(owners(db,file)?,Owners::One(id) if id==file){return Err(invalid("native number has no unique original FILE owner"));}
    key(pool,file)
}
pub(super) fn smb_index(db:&Connection,pool:PoolId,file:FileId)->io::Result<SmbFileIndex>{
    original(db,pool,file).map(|key|SmbFileIndex(key.inode))
}
fn key(pool:PoolId,file:FileId)->io::Result<NativeKey>{
    let n=number(file).ok_or_else(||invalid("original FILE has no usable native number"))?;
    let mut high=[0;8];let mut low=[0;8];high.copy_from_slice(&pool.0[..8]);low.copy_from_slice(&pool.0[8..]);
    Ok(NativeKey{device:u64::from_be_bytes(high),inode:n,extended:u64::from_be_bytes(low)})
}

pub(super) fn available(db:&Connection,pool:PoolId,candidates:&[FileId;8])->io::Result<(FileId,NativeKey)>{
    let file=choose(db,candidates)?;Ok((file,key(pool,file)?))
}
pub(super) fn available_exact(db:&Connection,pool:PoolId,file:FileId)->io::Result<NativeKey>{
    if !matches!(owners(db,file)?,Owners::None){
        return Err(io::Error::new(io::ErrorKind::WouldBlock,"the offered native number is no longer free"));
    }
    key(pool,file)
}

#[cfg(test)]
mod tests{
    use super::*;
    use crate::system::reader::zxs_sql_template;
    #[test]
    fn blob_number_ranges_include_both_ends_and_reject_ambiguous_originals(){
        // Use the actual pinned SQLite engine and a deliberately minimal
        // index. This fixture can represent a pre-existing collision which
        // the full CREATE path cannot issue. No new production table.
        let mut ptr=std::ptr::null_mut();
        check(unsafe{zxs_sql_template(c"CREATE TABLE root_entry(file_id BLOB PRIMARY KEY) STRICT, WITHOUT ROWID".as_ptr(),&mut ptr)}).unwrap();
        assert!(!ptr.is_null());let db=Connection(ptr);
        let id=|hi:u64,lo:u64|{let mut b=[0;16];b[..8].copy_from_slice(&hi.to_be_bytes());b[8..].copy_from_slice(&lo.to_be_bytes());FileId(b)};
        let insert=|x:FileId|{let q=db.prepare(c"INSERT INTO main.root_entry VALUES(?1)").unwrap();
            q.bind_bytes(1,&x.0).unwrap();q.done().unwrap();};
        let mut p=[0x81;16];p[8..].fill(0x42);let pool=PoolId(p);
        let occupied=0xfedcba9876543210;let a=id(occupied,0);let b=id(occupied,u64::MAX);
        let fresh=id(occupied+1,3);let small=id(255,8);let last=id(u64::MAX,7);
        insert(a);assert!(original(&db,pool,b).is_err());
        assert_eq!(original(&db,pool,a).unwrap().words(),(0x8181818181818181,occupied,0x4242424242424242));
        assert_eq!(choose(&db,&[small,last,b,b,b,b,b,fresh]).unwrap(),fresh);
        assert!(choose(&db,&[b;8]).is_err());
        insert(b);assert!(original(&db,pool,a).is_err());assert!(original(&db,pool,b).is_err());
        assert!(original(&db,pool,fresh).is_err());
        insert(small);insert(last);assert!(original(&db,pool,small).is_err());assert!(original(&db,pool,last).is_err());
    }
}
