use std::{fmt, io, str::FromStr};
unsafe extern "C" { fn zxs_random(bytes:*mut u8, len:usize) -> std::os::raw::c_int; }

fn random_bytes(value:&mut [u8])->io::Result<()> {
    // SAFETY: this writable array lives throughout the blocking getrandom call.
    if unsafe { zxs_random(value.as_mut_ptr(),value.len()) } != 0 {return Err(io::Error::last_os_error());}
    Ok(())
}
fn random() -> io::Result<[u8;16]> {
    let mut value=[0u8;16];random_bytes(&mut value)?;
    if value==[0;16] { return Err(io::Error::other("random source returned an all-zero ID")); }
    Ok(value)
}
fn parse(s:&str) -> io::Result<[u8;16]> {
    fn nibble(b:u8)->Option<u8>{match b {b'0'..=b'9'=>Some(b-b'0'),b'a'..=b'f'=>Some(b-b'a'+10),_=>None}}
    let bad=||crate::fs::invalid("expected a nonzero 32-digit lower-case hexadecimal ID");
    if s.len()!=32{return Err(bad());}
    let mut v=[0u8;16];
    for (dst,bytes) in v.iter_mut().zip(s.as_bytes().as_chunks::<2>().0){
        *dst=(nibble(bytes[0]).ok_or_else(bad)?<<4)|nibble(bytes[1]).ok_or_else(bad)?;
    }
    if v==[0;16]{Err(bad())}else{Ok(v)}
}
macro_rules! id {
    ($kind:ident) => {
        #[derive(Clone,Copy,Debug,Eq,Ord,PartialEq,PartialOrd,Hash)]
        pub struct $kind(pub(crate) [u8;16]);
        impl $kind {pub fn new()->io::Result<Self>{random().map(Self)}}
        impl fmt::Display for $kind { fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{
            for b in self.0 { write!(f,"{b:02x}")?; } Ok(())
        }}
        impl FromStr for $kind { type Err=io::Error;fn from_str(s:&str)->io::Result<Self>{parse(s).map(Self)}}
    }
}
id!(OperationId); id!(PoolId); id!(VolumeId); id!(AccountId); id!(ShareId);
id!(FileId); id!(EntryId); id!(HistoryId); id!(DirectoryId);
id!(BranchId); id!(NodeId); id!(VersionId);

impl FileId{
    /// One bounded entropy draw before the SQLite writer transaction. Only
    /// the selected original is persisted, and only after its number is free.
    pub(crate) fn birth_candidates()->io::Result<[Self;8]>{
        let mut bytes=[0u8;128];random_bytes(&mut bytes)?;
        Ok(std::array::from_fn(|i|{let mut id=[0;16];id.copy_from_slice(&bytes[16*i..16*i+16]);Self(id)}))
    }
}

/// Native FULL basename prefix. This is a distinct storage ID, not the
/// business VersionId, NodeId, a path component chosen by a client, or a hash.
#[derive(Clone,Copy,Debug,Eq,Ord,PartialEq,PartialOrd,Hash)]
pub struct VersionStorageId(pub(crate) [u8;15]);
impl VersionStorageId{
    pub(crate) fn fresh()->io::Result<Self>{
        let bits=random()?;let hex=b"0123456789abcdef";let mut key=[0;15];
        for (i,c) in key.iter_mut().enumerate(){
            let b=bits[i/2];*c=hex[usize::from(if i%2==0{b>>4}else{b&15})];
        }
        Self::checked(key)
    }
    pub(crate) fn checked(key:[u8;15])->io::Result<Self>{
        if key.iter().all(|b|*b==b'0')||!key.iter().all(|b|b.is_ascii_digit()||(b'a'..=b'f').contains(b)){
            return Err(crate::fs::invalid("invalid 15-character FULL storage identifier"));
        }
        Ok(Self(key))
    }
}
impl fmt::Display for VersionStorageId{
    fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result{f.write_str(std::str::from_utf8(&self.0).expect("checked ASCII storage ID"))}
}
