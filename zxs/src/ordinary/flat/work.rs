//! The first manual working revision: one existing material/check-table
//! format selected by SQL, not a formal node. The caller supplies a completed
//! staging FD: its measured bytes/stamps are checked, not OS write sealing.
//! A network transport must not accept a native path from the SMB client.
use super::{Index,Leaf,Material,blocks::{self,Buffers,Reader,Ref},local::Located};
use crate::{FileId,OperationId,PoolId,fs::{HeldDir,LiveStamp,WitnessRead,WorkPair,invalid},hash::Sha256,
    ordinary::output::CheckedOutput,system::reader::smb::first_work::Reserved};
use std::{collections::BTreeSet,ffi::CString,fs::File,io::{self,Write}};

/// Precise names left behind when a pending attempt lost its original grant.
/// They are not read, modified, unlinked or interpreted as another working revision.
pub(crate) type Retained=BTreeSet<Vec<u8>>;
pub(crate) fn include_retained(set:&mut Retained,op:OperationId,mask:u8)->io::Result<()>{
    if mask>15||op.0==[0;16]{return Err(invalid("invalid abandoned first working attempt"));}
    for(i,n)in Located::change_names(op,0).into_iter().enumerate(){
        if mask&(1<<i)!=0&&(!set.insert(n.into_bytes())||set.len()>crate::fs::DIRECTORY_NAMES){return Err(invalid("duplicate or excessive retained working names"));}
    }Ok(())
}
pub(crate) fn room(kept:&Retained,s:Spec)->io::Result<()>{
    let roles=s.names()?;
    let count=1+4+kept.len();let sum=5+roles.iter().map(|n|1+n.to_bytes().len()).sum::<usize>()+kept.iter().map(|n|1+n.len()).sum::<usize>();
    if count>crate::fs::DIRECTORY_NAMES||sum>crate::fs::DIRECTORY_BYTES||roles.iter().any(|n|kept.contains(n.to_bytes())){
        return Err(invalid("no room for both preserved prior attempts and all four new working names"));}
    Ok(())
}

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(crate) struct Spec{pub(crate) pool:PoolId,pub(crate) file:FileId,pub(crate) op:OperationId,
    pub(crate) bytes:u64,pub(crate) content_sha:[u8;32],pub(crate) directory_sha:[u8;32]}
impl Spec{
    pub(crate) fn checked(pool:PoolId,file:FileId,op:OperationId,bytes:u64,content_sha:[u8;32],directory_sha:[u8;32])->io::Result<Self>{
        let s=Self{pool,file,op,bytes,content_sha,directory_sha};s.reference()?;Ok(s)
    }
    fn reference(self)->io::Result<Ref>{Ref::declared(self.pool,Material::declared(self.file,self.op,0,self.bytes)?,self.directory_sha)}
    pub(crate) fn table_len(self)->io::Result<u64>{self.reference()?.table_len()}
    pub(crate) fn names(self)->io::Result<[CString;4]>{
        let(a,b,c,d)=Located::change(self.reference()?).names();Ok([a,b,c,d])
    }
    pub(crate) fn members(self,scope:&WitnessRead<'_>,all:bool,kept:&Retained)->io::Result<()>{scope.work_members(&self.names()?,all,kept)}
}

/// Scan a completed staging file; independently compare its measured contents again on output.
/// A single bounded traversal creates both independent claims; an untrusted
/// request cannot ask SQLite to reserve a fantastically large invented body.
pub(crate) struct Incoming<'a>{file:&'a File,stamp:LiveStamp,spec:Spec}
impl<'a> Incoming<'a>{
    pub(crate) fn observe(pool:PoolId,file:FileId,op:OperationId,input:&'a File)->io::Result<Self>{
        let stamp=LiveStamp::read(input)?;let m=Material::declared(file,op,0,stamp.len)?;let mut all=Sha256::new();
        let reference=blocks::walk(input,pool,m,|_,bytes|all.update(bytes),|_,_|Ok(()))?;
        stamp.require(input)?;
        let spec=Spec::checked(pool,file,op,stamp.len,all.finish(),reference.directory_sha())?;
        Ok(Self{file:input,stamp,spec})
    }
    pub(crate) fn spec(&self)->Spec{self.spec}
    pub(crate) fn unchanged(&self)->io::Result<()>{self.stamp.require(self.file)}

    /// Only an actual SQLite pending operation with its original positive
    /// FILE/WRITE selection can supply this destination, even for a retry.
    pub(crate) fn prepare(&self,work:&Reserved<'_>)->io::Result<HeldWork>{
        let(installed,spec)=work.parts();let kept=work.retained();
        if spec!=self.spec{return Err(invalid("wrong original working input or operation"));}
        self.unchanged()?;
        let scope=WitnessRead::at(installed.directory())?;let names=spec.names()?;scope.work_members(&names,false,kept)?;
        scope.matching_presence(&names[0],&names[1])?;scope.matching_presence(&names[2],&names[3])?;
        let(mut data,mut table)=(WorkPair::open(work,false)?,WorkPair::open(work,true)?);
        if data.stamp().same(table.stamp())||self.stamp.same(data.stamp())||self.stamp.same(table.stamp()){
            return Err(invalid("working source, data and index must be independent actual regular files"));}
        let reference=blocks::walk(self.file,spec.pool,spec.reference()?.material,|at,b|data.put(at,b),|at,b|table.put(at,b))?;
        if reference!=spec.reference()?{return Err(invalid("input changed after reservation"));}
        self.unchanged()?;data.finish()?;table.finish()?;let dir=installed.directory();dir.file.sync_all()?;
        let held=HeldWork::open(dir,spec,kept,Use::Confirming)?;held.reader.same_issued_pair(data.stamp(),table.stamp())?;
        // Independent named source-of-truth readback, not reading the
        // incoming buffer or accepting only the table's self-declaration.
        let mut digest=Digest(Sha256::new());
        held.range(0,spec.bytes,&mut digest,||Ok(()))?;
        if digest.0.finish()!=spec.content_sha{return Err(invalid("named working bytes differ from the original input"));}
        held.require(dir)?;self.unchanged()?;Ok(held)
    }
}
struct Digest(Sha256);
impl Write for Digest{fn write(&mut self,b:&[u8])->io::Result<usize>{self.0.update(b)?;Ok(b.len())}fn flush(&mut self)->io::Result<()>{Ok(())}}

/// A fixed, already selected logical view, inseparable from its real
/// material and original checksum directory. No caller may give range()
/// another map, learn its EOF from a disk name, or select today's head.
pub(crate) struct HeldWork{view:Index,material:Ref,reader:Reader,roles:[CString;4]}
enum Use{Confirming,ConfirmedReader}
impl HeldWork{
    pub(crate) fn confirmed(dir:&HeldDir,spec:Spec,kept:&Retained)->io::Result<Self>{
        Self::open(dir,spec,kept,Use::ConfirmedReader)
    }
    fn open(dir:&HeldDir,spec:Spec,kept:&Retained,usage:Use)->io::Result<Self>{
        let original=spec.reference()?;let scope=WitnessRead::at(dir)?;
        let roles=spec.names()?;scope.work_members(&roles,true,kept)?;
        // The first view is still admitted at its exact registered length.
        // Once held, a later append cannot invalidate an older selected tail
        // block: validation uses its original length and checksum, not EOF.
        let raw=scope.open(&roles[0],&roles[1],spec.bytes)?;
        // A candidate supports no future append until its short confirmation
        // has succeeded. It keeps both complete, original stamps until then.
        let raw=match usage{Use::Confirming=>raw,Use::ConfirmedReader=>raw.checked_prefix()?};
        let checks=scope.open(&roles[2],&roles[3],original.table_len()?)?;
        // v7 represents this first confirmed snapshot as one entire material.
        // Construct its direct map in memory; no new disk bytes or DB column.
        let view=Index::full(original.material)?;
        let h=Self{view,material:original,reader:Reader::open(original,raw,checks)?,roles};h.require(dir)?;Ok(h)
    }
    pub(crate) fn size(&self)->u64{self.view.len()}
    pub(crate) fn require(&self,dir:&HeldDir)->io::Result<()>{
        WitnessRead::require_parent(dir)?;self.reader.require_names(dir,&self.roles)
    }
    pub(crate) fn range(&self,at:u64,n:u64,mut out:impl Write,check:impl Fn()->io::Result<()>)->io::Result<()>{
        if at.checked_add(n).is_none_or(|end|end>self.size()){return Err(invalid("read past original working EOF"));}
        let(page,block)=if n==0{(0,0)}else{let(_,p,b)=self.material.read_bytes(at)?;(p,b)};
        let mut scratch=Buffers::new(page,block)?;
        let mut checked=CheckedOutput{out:&mut out,check};
        self.reader.unchanged()?;
        self.view.visit(at,n,|leaf,offset,len|match leaf{
            Leaf::Data(m) if m==self.material.material=>self.reader.range(offset,len,&mut scratch,&mut checked),
            _=>Err(invalid("selected first view refers to unissued material")),
        })?;
        self.reader.unchanged()?;checked.flush()?;self.reader.unchanged()
    }
}
