//! Independent raw FULL plus the small local node/index in the same FILE.
//! A temporary part has no name until the genuine original SYSTEM reservation
//! selects both names. There is never an independent destination container.
use super::{allocation::FormalAllocation,types::{Commit,Materials,Original}};
use crate::{fs::{self,HeldDir,invalid,stream::{self,PinnedRegular}},ordinary::{content::{self,BLOCK},draft::material::Reader as Working,wire::NativeObject}};
use std::{ffi::CString,fs::File,io::{self,Write}};

#[derive(Clone,Copy)]
enum Kind{Node,Raw}
impl Kind{
    fn name(self,o:&Original)->CString{match self{Self::Node=>o.node_name(),Self::Raw=>o.body_name()}}
    fn size(self,o:&Original)->io::Result<u64>{
        match self{Self::Raw=>Ok(o.content.bytes),Self::Node=>content::chunks(o.content.bytes).checked_mul(32)
            .and_then(|n|n.checked_add(o.node_header().len() as u64)).filter(|n|*n<=i64::MAX as u64)
            .ok_or_else(||invalid("formal node index exceeds native offsets"))}
    }
    fn token(self,a:&FormalAllocation,n:&NativeObject)->io::Result<[u8;32]>{
        let mut b=b"ZXS/FIRST-FORMAL/PREPARED\0".to_vec();b.extend_from_slice(&a.digest);
        b.push(match self{Self::Node=>1,Self::Raw=>2});n.put(&mut b);crate::hash::digest(&b)
    }
}
pub(in crate::ordinary) fn index_size(o:&Original)->io::Result<u64>{Kind::Node.size(o)}
fn check_header(file:&File,o:&Original)->io::Result<Vec<u8>>{
    let wanted=o.node_header();let mut actual=vec![0;wanted.len()];stream::read_at(file,&mut actual,0)?;
    if wanted!=actual{return Err(invalid("local FILE node/branch/version/source or creation-time name differs"));}Ok(actual)
}
fn visit(raw:&File,index:&File,o:&Original,check:impl FnMut()->io::Result<()>,mut out:impl Write)->io::Result<[u8;32]>{
    let actual_header=check_header(index,o)?;let start=actual_header.len() as u64;
    let mut exact_index=crate::hash::Sha256::new();exact_index.update(&actual_header)?;
    content::visit(o.content,o.chain,|i,bytes,next|{
        stream::read_at(raw,bytes,i*BLOCK)?;stream::read_at(index,next,start+32*i)?;exact_index.update(next)
    },check,|_,part,_|out.write_all(part))?;
    Ok(exact_index.finish())
}
fn named(dir:&HeldDir,o:&Original,k:Kind)->io::Result<Option<PinnedRegular>>{
    let Some(pin)=dir.pin_stream(&k.name(o),k.size(o)?)? else{return Ok(None)};
    if matches!(k,Kind::Node){check_header(&pin.file,o)?;}Ok(Some(pin))
}
pub(in crate::ordinary) struct HistoryBody{pub(super) node:PinnedRegular,pub(super) raw:PinnedRegular,pub(super) original:Original,pub(super) index_digest:[u8;32]}
impl HistoryBody{
    /// These are the two *handled original* files actually checked by
    /// write_to; a transfer cannot substitute a current path or remote blob.
    pub(in crate::ordinary) fn native_files(&self)->(&PinnedRegular,&PinnedRegular){(&self.node,&self.raw)}
    /// A user may continue reading the frozen old version after a placement
    /// decision. A transfer therefore must not unlink either exact source
    /// leaf until these two shared leases are absent. Don't lock the generic
    /// FILE working-record here: a historical read never needs spent drafts.
    pub(in crate::ordinary) fn user_read(self)->io::Result<Self>{
        self.node.file.try_lock_shared().map_err(io::Error::from)?;
        self.raw.file.try_lock_shared().map_err(io::Error::from)?;
        self.unchanged()?;Ok(self)
    }
    fn facts(&self)->Materials{Materials{node:NativeObject::stamp(&self.node.stamp),raw:NativeObject::stamp(&self.raw.stamp),index_digest:self.index_digest}}
    pub(in crate::ordinary) fn current(dir:&HeldDir,f:&Commit)->io::Result<Self>{
        Self::at_originals(dir,f,&f.materials.node,&f.materials.raw)
    }
    /// Addressing is separate from the immutable initial publication. A moved
    /// graph supplies the two original inodes from its admitted custody cut;
    /// byte commitments and stable IDs always come from the content decision.
    pub(in crate::ordinary) fn at_originals(dir:&HeldDir,f:&Commit,want_node:&NativeObject,want_raw:&NativeObject)->io::Result<Self>{
        let (node,raw)=(named(dir,&f.original,Kind::Node)?.ok_or_else(||invalid("original local node unavailable"))?,
            named(dir,&f.original,Kind::Raw)?.ok_or_else(||invalid("original independent FULL unavailable"))?);
        if &NativeObject::stamp(&node.stamp)!=want_node||&NativeObject::stamp(&raw.stamp)!=want_raw{
            return Err(invalid("original formal material replaced"));
        }
        // A read handle carries the expected digest; opening the header is
        // not evidence that its index was read. Only a fresh Verified object
        // below can issue preparation feedback to the original writer.
        let b=Self{node,raw,original:f.original.clone(),index_digest:f.materials.index_digest};b.require(dir)?;Ok(b)
    }
    pub(in crate::ordinary) fn unchanged(&self)->io::Result<()>{self.raw.unchanged()?;self.node.unchanged()}
    pub(in crate::ordinary) fn require(&self,dir:&HeldDir)->io::Result<()>{
        self.raw.named(dir,&self.original.body_name())?;self.node.named(dir,&self.original.node_name())
    }
    pub(in crate::ordinary) fn write_to(&self,out:impl Write)->io::Result<()>{
        let exact=visit(&self.raw.file,&self.node.file,&self.original,||self.unchanged(),out)?;
        if exact!=self.index_digest{return Err(invalid("complete formal node index differs from original publication"));}Ok(())
    }
}
struct Before{node:Option<PinnedRegular>,raw:Option<PinnedRegular>}
impl Before{
    fn read(dir:&HeldDir,a:Option<&FormalAllocation>,o:&Original)->io::Result<Self>{
        let node=named(dir,o,Kind::Node)?;let raw=named(dir,o,Kind::Raw)?;
        if raw.is_some()&&node.is_none(){return Err(invalid("independent FULL is named without the original local node"));}
        for (k,p) in [(Kind::Node,&node),(Kind::Raw,&raw)]{
            if let Some(p)=p{
                let a=a.ok_or_else(||invalid("unreserved formal name already exists"))?;
                fs::require_material_token(&p.file,&k.token(a,&NativeObject::stamp(&p.stamp))?)?;
            }
        }
        Ok(Self{node,raw})
    }
}
pub(in crate::ordinary) fn inspect_prepared(dir:&HeldDir,a:&FormalAllocation,sealed:Option<&Commit>)->io::Result<()>{
    let seen=Before::read(dir,Some(a),&a.original)?;
    if let Some(f)=sealed{
        if !a.matches(f)||seen.node.as_ref().map(|x|NativeObject::stamp(&x.stamp))!=Some(f.materials.node.clone())||
            seen.raw.as_ref().map(|x|NativeObject::stamp(&x.stamp))!=Some(f.materials.raw.clone()){
            return Err(invalid("sealed formal FILE differs from its original native operands"));
        }
    }Ok(())
}
enum Part{Temp(File),Named(PinnedRegular)}
impl Part{
    fn setup(dir:&HeldDir,p:Option<PinnedRegular>)->io::Result<Self>{
        match p{Some(n)=>Ok(Self::Named(n)),None=>dir.anonymous_record().map(|(f,_)|Self::Temp(f))}
    }
    fn file(&self)->&File{match self{Self::Temp(f)=>f,Self::Named(p)=>&p.file}}
    fn temp(&self)->bool{matches!(self,Self::Temp(_))}
    fn allocate(&self,size:u64)->io::Result<()>{if self.temp()&&size!=0{stream::allocate_stream(self.file(),size)?;}Ok(())}
    fn stamp(&self)->io::Result<fs::OriginalStamp>{
        match self{Self::Temp(f)=>stream::anonymous_stamp(f),Self::Named(p)=>{p.unchanged()?;Ok(p.stamp.clone())}}
    }
    fn place(&self,dir:&HeldDir,k:Kind,a:&FormalAllocation,stamp:&fs::OriginalStamp)->io::Result<()>{
        if &self.stamp()?!=stamp{return Err(invalid("prepared FULL/node changed before naming"));}
        let name=k.name(&a.original);
        match self{
            Self::Named(p)=>p.named(dir,&name),
            Self::Temp(f)=>{
                fs::set_preparation_token(f,&k.token(a,&NativeObject::stamp(stamp))?)?;
                dir.link_stream(&name,f,stamp,k.size(&a.original)?)
            }
        }
    }
}
pub(super) struct Input{dir:HeldDir,pub original:Original,work:Option<Working>,node:Part,raw:Part}
pub(super) struct Verified{input:Input,node:fs::OriginalStamp,raw:fs::OriginalStamp,index_digest:[u8;32]}
impl Input{
    pub(super) fn open(dir:HeldDir,o:Original,work:Option<Working>,a:Option<&FormalAllocation>,decided:Option<&Commit>)->io::Result<Self>{
        dir.exclusive()?;
        let before=Before::read(&dir,a,&o)?;
        if work.is_none()&&decided.is_none(){return Err(invalid("cannot synthesize a first formal commit without its true working source"));}
        if let Some(f)=decided{
            if f.original!=o||before.node.as_ref().map(|p|NativeObject::stamp(&p.stamp))!=Some(f.materials.node.clone())||
                before.raw.as_ref().map(|p|NativeObject::stamp(&p.stamp))!=Some(f.materials.raw.clone()){
                return Err(invalid("a decided FULL can never be rebuilt from a missing or replacement body"));
            }
        }
        let node=Part::setup(&dir,before.node)?;let raw=Part::setup(&dir,before.raw)?;
        Ok(Self{dir,original:o,work,node,raw})
    }
    /// After short SYSTEM/root leases are released, verify the real whole
    /// source and materialize only missing original operands. A partial retry
    /// never recopies an already named FULL to another temporary FULL.
    pub(super) fn verify(self)->io::Result<Verified>{
        let o=&self.original;let header=o.node_header();let start=header.len() as u64;
        self.raw.allocate(Kind::Raw.size(o)?)?;self.node.allocate(Kind::Node.size(o)?)?;
        if self.node.temp(){stream::write_at(self.node.file(),&header,0)?;}
        if let Some(work)=&self.work{
            work.visit(|i,data,next|{
                if self.raw.temp(){stream::write_at(self.raw.file(),data,i*BLOCK)?;}
                if self.node.temp(){stream::write_at(self.node.file(),&next,start+i*32)?;}
                Ok(())
            })?;
        }
        self.raw.file().sync_all()?;self.node.file().sync_all()?;
        let raw=self.raw.stamp()?;let node=self.node.stamp()?;
        if raw.stat.size()!=Kind::Raw.size(o)?||node.stat.size()!=Kind::Node.size(o)?{return Err(invalid("prepared FULL or local index length differs"));}
        let index_digest=visit(self.raw.file(),self.node.file(),o,||Ok(()),io::sink())?;
        if self.raw.stamp()?!=raw||self.node.stamp()?!=node{return Err(invalid("prepared FULL or node changed during physical verification"));}
        Ok(Verified{input:self,raw,node,index_digest})
    }
}
impl Verified{
    pub(super) fn original(&self)->&Original{&self.input.original}
    pub(super) fn source_current(&self,dir:&HeldDir,o:&Original)->io::Result<()>{
        if self.input.original!=*o||self.input.dir.entity!=dir.entity||
            fs::Birth::read(&self.input.dir.file)?!=fs::Birth::read(&dir.file)?{return Err(invalid("formal preparation selects a different original FILE"));}
        self.input.dir.require_named()?;
        if let Some(ref s)=self.input.work{s.require(dir)?;}
        Ok(())
    }
    pub(super) fn install(&self,dir:&HeldDir,a:&FormalAllocation,after_node:impl FnOnce()->io::Result<()>)->io::Result<VerifiedBody>{
        self.source_current(dir,&a.original)?;
        self.input.node.place(dir,Kind::Node,a,&self.node)?;after_node()?;self.input.raw.place(dir,Kind::Raw,a,&self.raw)?;
        let actual=Before::read(dir,Some(a),&a.original)?;
        let body=HistoryBody{node:actual.node.ok_or_else(||invalid("formal node missing after preparation"))?,
            raw:actual.raw.ok_or_else(||invalid("formal FULL missing after preparation"))?,original:a.original.clone(),index_digest:self.index_digest};
        let actual=body.facts();
        if actual.node!=NativeObject::stamp(&self.node)||actual.raw!=NativeObject::stamp(&self.raw){return Err(invalid("different formal operands installed"));}
        body.require(dir)?;dir.file.sync_all()?;Ok(VerifiedBody(body))
    }
    pub(super) fn retire(self,dir:&HeldDir,decision:&Commit,after_unlink:impl FnOnce()->io::Result<()>)->io::Result<()>{
        self.source_current(dir,&decision.original)?;
        HistoryBody::current(dir,decision)?.require(dir)?;
        if let Some(old)=self.input.work{
            let (name,pin)=old.into_retirement(dir)?;
            dir.release_verified_work(&name,&pin,after_unlink)?;
        }
        dir.confirm_absence(&crate::ordinary::draft::material::leaf(decision.original.save))?;
        HistoryBody::current(dir,decision)?.require(dir)
    }
}
pub(super) struct VerifiedBody(HistoryBody);
impl VerifiedBody{
    pub(super) fn feedback(&self,dir:&HeldDir)->io::Result<Materials>{self.0.require(dir)?;dir.file.sync_all()?;Ok(self.0.facts())}
}
