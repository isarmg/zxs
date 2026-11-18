//! One native working extent, not a full logical output: canonical header,
//! exactly the supplied changed bytes, then one verification link per chunk.
use super::{allocation::SpanAllocation,types::{Material,Original}};
use crate::{fs::{self,HeldDir,invalid,stream::{self,PinnedRegular}},ordinary::{content::{self,BLOCK},
    formal::physical::HistoryBody,indexed::Indexed,wire::NativeObject}};
use std::{fs::File,io::{self,Read,Write}};

fn shape(o:&Original)->io::Result<(u64,u64,u64)>{
    let h=o.header([0;32]).len() as u64;let len=o.edit.input.bytes;
    let table=h.checked_add(len).ok_or_else(||invalid("extent offset overflow"))?;
    let total=content::chunks(len).checked_mul(32).and_then(|n|n.checked_add(table)).filter(|n|*n<=i64::MAX as u64)
        .ok_or_else(||invalid("physical extent size overflow"))?;Ok((h,table,total))
}
fn header(file:&File,o:&Original)->io::Result<(Vec<u8>,[u8;32])>{
    let mut h=vec![0;shape(o)?.0 as usize];stream::read_at(file,&mut h,0)?;
    let chain=h[h.len()-32..].try_into().expect("one complete chain head");
    if h!=o.header(chain){return Err(invalid("working extent belongs to another FILE, base, range or length policy"));}
    Ok((h,chain))
}
fn token(a:&SpanAllocation,obj:&NativeObject,chain:[u8;32])->io::Result<[u8;32]>{
    let mut b=b"ZXS/WORKING/EXTENT/PREPARED\0".to_vec();b.extend_from_slice(&a.digest);obj.put(&mut b);b.extend_from_slice(&chain);crate::hash::digest(&b)
}
fn scan(file:&File,o:&Original,check:impl FnMut()->io::Result<()>)->io::Result<([u8;32],[u8;32])>{
    let (h,head)=header(file,o)?;let (data,table,_)=shape(o)?;let mut hash=crate::hash::Sha256::new();hash.update(&h)?;
    content::visit(o.edit.input,head,|i,bytes,next|{
        stream::read_at(file,bytes,data+i*BLOCK)?;stream::read_at(file,next,table+i*32)?;hash.update(next)
    },check,|_,_,_|Ok(()))?;
    Ok((head,hash.finish()))
}
struct Named{pin:PinnedRegular,original:Original,head:[u8;32]}
impl Named{
    fn open(dir:&HeldDir,o:&Original)->io::Result<Option<Self>>{
        let name=crate::ordinary::draft::material::leaf(o.op);
        let Some(pin)=dir.pin_stream(&name,shape(o)?.2)? else{return Ok(None)};
        pin.file.try_lock_shared().map_err(io::Error::from)?;let (_,head)=header(&pin.file,o)?;pin.named(dir,&name)?;
        Ok(Some(Self{pin,original:o.clone(),head}))
    }
    fn prepared(&self,a:&SpanAllocation)->io::Result<()>{
        fs::require_material_token(&self.pin.file,&token(a,&NativeObject::stamp(&self.pin.stamp),self.head)?)
    }
    fn require(&self,dir:&HeldDir)->io::Result<()>{self.pin.named(dir,&crate::ordinary::draft::material::leaf(self.original.op))}
}
pub(in crate::ordinary) struct Stored{named:Named,index:[u8;32]}
impl Stored{
    pub(in crate::ordinary) fn reopen(dir:&HeldDir,o:&Original,m:&Material)->io::Result<Self>{
        let n=Named::open(dir,o)?.ok_or_else(||invalid("confirmed working extent missing"))?;
        if NativeObject::stamp(&n.pin.stamp)!=m.object||n.head!=m.chain{return Err(invalid("working extent replaced or its header changed"));}
        Ok(Self{named:n,index:m.index})
    }
    fn facts(&self)->Material{Material{object:NativeObject::stamp(&self.named.pin.stamp),chain:self.named.head,index:self.index}}
    fn range(&self,at:u64,len:u64,out:impl Write)->io::Result<()>{
        let n=&self.named;let o=&n.original;let (raw_start,link_start,_)=shape(o)?;
        Indexed{raw:&n.pin.file,index:&n.pin.file,header:o.header(n.head),raw_start,link_start,claim:o.edit.input,
            head:n.head,digest:self.index}.write_range(at,len,out,||n.pin.unchanged())
    }
}
pub(in crate::ordinary) fn inspect(dir:&HeldDir,a:&SpanAllocation,sealed:Option<&super::types::Saved>)->io::Result<()>{
    let n=Named::open(dir,&a.original)?;
    if let Some(n)=&n{n.prepared(a)?;}
    if let Some(f)=sealed{
        if !a.matches(f)||!n.is_some_and(|n|NativeObject::stamp(&n.pin.stamp)==f.material.object&&n.head==f.material.chain){
            return Err(invalid("sealed working change has lost its actual original operand"));
        }
    }Ok(())
}
mod cut;
pub(in crate::ordinary) use cut::{Composed,Operands};
// The logical revision depends on all three held operands, even when a
// particular output slice is a pure zero gap or uses only one of the two
// sources. fstat the already pinned originals; never read unrelated BODY
// blocks or rehash an unrelated LOCAL index just to prove the cut stayed put.
struct CheckedOutput<W,C>{out:W,check:C}
fn after_output(check:&mut impl FnMut()->io::Result<()>)->io::Result<()>{
    // The sink has already accepted bytes. An interrupted fstat can retry
    // only that observation, never signal write_all to repeat the payload.
    loop{match check(){Err(e) if e.kind()==io::ErrorKind::Interrupted=>continue,r=>return r}}
}
impl<W:Write,C:FnMut()->io::Result<()>> Write for CheckedOutput<W,C>{
    fn write(&mut self,bytes:&[u8])->io::Result<usize>{
        (self.check)()?;let n=self.out.write(bytes)?;after_output(&mut self.check)?;Ok(n)
    }
    fn flush(&mut self)->io::Result<()>{
        (self.check)()?;self.out.flush()?;after_output(&mut self.check)
    }
}
enum Part{Anonymous(File),Named(Named)}
pub(super) struct Capture{dir:HeldDir,base:HistoryBody,prior:Operands,o:Original,part:Part}
pub(super) struct Verified{capture:Capture,stamp:fs::OriginalStamp,head:[u8;32],index:[u8;32]}
impl Capture{
    pub(super) fn open(dir:HeldDir,base:HistoryBody,prior:Operands,o:Original,a:Option<&SpanAllocation>,sealed:bool)->io::Result<Self>{
        dir.exclusive()?;
        let part=match Named::open(&dir,&o)?{
            Some(n)=>{n.prepared(a.ok_or_else(||invalid("unreserved existing material at the new working name"))?)?;Part::Named(n)}
            None if sealed=>return Err(invalid("decided or sealed extent cannot be rebuilt from new input")),
            None=>Part::Anonymous(dir.anonymous_record()?.0),
        };
        Ok(Self{dir,base,prior,o,part})
    }
    pub(super) fn verify(self,input:impl Read)->io::Result<Verified>{
        // Check the original formal node's exact authenticated index and named
        // raw identity. Do not read/copy unrelated 100 GiB base content merely
        // to save a few changed bytes. A future read checks contributing blocks.
        self.base.write_range(0,0,io::sink())?;self.prior.verify_indexes(&self.dir)?;
        let o=&self.o;let (body,table,total)=shape(o)?;
        let (stamp,head,index)=match &self.part{
            Part::Anonymous(f)=>{
                stream::allocate_stream(f,total)?;
                let link=content::stage(o.edit.input,input,f,|i,_|(body+i*BLOCK,table+i*32))?;
                stream::write_at(f,&o.header(link),0)?;f.sync_all()?;let stamp=stream::anonymous_stamp(f)?;
                let (head,index)=scan(f,o,||Ok(()))?;
                if head!=link||stamp.stat.size()!=total||stream::anonymous_stamp(f)?!=stamp{return Err(invalid("private changed extent unstable"));}
                (stamp,head,index)
            }
            Part::Named(n)=>{
                // Already named original ignores the caller's input, even if
                // non-seekable. Authenticate that exact retained working material.
                let (head,index)=scan(&n.pin.file,o,||n.pin.unchanged())?;n.pin.file.sync_all()?;n.pin.unchanged()?;
                if head!=n.head{return Err(invalid("changed working extent during retry"));}(n.pin.stamp.clone(),head,index)
            }
        };
        Ok(Verified{capture:self,stamp,head,index})
    }
}
impl Verified{
    pub(super) fn original(&self)->&Original{&self.capture.o}
    pub(super) fn source_current(&self,dir:&HeldDir,o:&Original)->io::Result<()>{
        if self.capture.o!=*o||self.capture.dir.entity!=dir.entity||fs::Birth::read(&self.capture.dir.file)?!=fs::Birth::read(&dir.file)?{
            return Err(invalid("working interval no longer has its selected registered FILE"));
        }
        self.capture.dir.require_named()?;self.capture.base.require(dir)?;self.capture.prior.require(dir)?;
        match &self.capture.part{
            Part::Anonymous(f)=>if stream::anonymous_stamp(f)?!=self.stamp{return Err(invalid("private incoming extent changed"));},
            Part::Named(n)=>n.require(dir)?,
        }Ok(())
    }
    pub(super) fn place(&self,dir:&HeldDir,a:&SpanAllocation)->io::Result<Bound>{
        self.source_current(dir,&a.original)?;let name=crate::ordinary::draft::material::leaf(a.original.op);
        if let Part::Anonymous(f)=&self.capture.part{
            fs::set_preparation_token(f,&token(a,&NativeObject::stamp(&self.stamp),self.head)?)?;
            dir.link_stream(&name,f,&self.stamp,shape(&a.original)?.2)?;
        }
        let bound=Named::open(dir,&a.original)?.ok_or_else(||invalid("original working interval not installed"))?;
        bound.prepared(a)?;
        if NativeObject::stamp(&bound.pin.stamp)!=NativeObject::stamp(&self.stamp)||bound.head!=self.head{return Err(invalid("different native extent was installed"));}
        dir.file.sync_all()?;Ok(Bound(Stored{named:bound,index:self.index}))
    }
    pub(super) fn base_current(&self,dir:&HeldDir)->io::Result<()>{self.capture.dir.require_named()?;self.capture.base.require(dir)?;self.capture.prior.require(dir)}
}
pub(super) struct Bound(Stored);
impl Bound{pub(super) fn feedback(&self,dir:&HeldDir)->io::Result<Material>{self.0.named.require(dir)?;dir.file.sync_all()?;Ok(self.0.facts())}}

#[cfg(test)]
mod checked_output_tests{
    use super::*;
    #[test]
    fn interrupted_post_write_check_retries_observation_not_already_delivered_bytes(){
        let mut seen=Vec::new();let mut checks=0u8;
        {
            let mut out=CheckedOutput{out:&mut seen,check:||{
                checks+=1;if checks==2{return Err(io::Error::from(io::ErrorKind::Interrupted));}Ok(())
            }};
            out.write_all(b"one payload").unwrap();
        }
        assert_eq!(checks,3);assert_eq!(seen,b"one payload");
    }
}
