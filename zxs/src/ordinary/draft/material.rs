//! A first SAVE streams the new range directly into the one eventual working
//! file. Four MiB is a verification buffer, not a file-size ceiling. Each
//! block carries the commitment to the next block: an authentic head permits
//! verified forward reads without loading a file-size-dependent checksum table.
use super::{allocation::SaveAllocation,types::{DRAFT_CHUNK,DraftClaim,Material,Original}};
use crate::{OperationId,fs::{self,Birth,HeldDir,invalid,stream::{self,PinnedRegular}}};
use crate::ordinary::{wire::NativeObject,content::{self,chunks,end,link}};
use std::{ffi::CString,fs::File,io::{self,Read,Write}};

const MAGIC:&[u8]=b"ZXS/FILE/WORKING/INITIAL-RANGE\0";
const FIXED:usize=MAGIC.len()+16*3+8*4+32;
const HEADER:u64=FIXED as u64+32;
const CHUNK:u64=DRAFT_CHUNK as u64;
pub(in crate::ordinary) fn leaf(op:OperationId)->CString{CString::new(format!("working-{op}")).expect("fixed native working name")}
fn claim_buffer(c:DraftClaim)->usize{c.bytes.clamp(1,CHUNK) as usize}
pub(in crate::ordinary) fn physical_size(bytes:u64)->io::Result<u64>{
    if bytes>u64::MAX/8{return Err(invalid("content exceeds SHA-256 representable length"));}
    chunks(bytes).checked_mul(32).and_then(|n|n.checked_add(bytes)).and_then(|n|n.checked_add(HEADER))
        .filter(|n|*n<=i64::MAX as u64).ok_or_else(||invalid("working extent exceeds native signed file offsets"))
}
fn location(claim:DraftClaim,i:u64)->(u64,usize){(HEADER+i*(CHUNK+32),(claim.bytes-i*CHUNK).min(CHUNK) as usize)}
fn header(o:&Original)->Vec<u8>{
    let mut b=MAGIC.to_vec();b.extend_from_slice(&o.file.0);b.extend_from_slice(&o.history.0);b.extend_from_slice(&o.op.0);
    for n in [0u64,1,0,o.claim.bytes]{b.extend_from_slice(&n.to_be_bytes());} // from, to, logical offset, length
    b.extend_from_slice(&o.claim.sha256);b
}
fn read_header(file:&File,o:&Original)->io::Result<[u8;32]>{
    let mut bytes=vec![0;HEADER as usize];stream::read_at(file,&mut bytes,0)?;
    if &bytes[..FIXED]!=header(o).as_slice(){return Err(invalid("working body is not the original FILE, operation and revision"));}
    Ok(bytes[FIXED..].try_into().expect("exact working chain head"))
}
fn stream_content(file:&File,o:&Original,head:[u8;32],mut out:impl Write,check:impl Fn()->io::Result<()>)->io::Result<()>{
    visit_work(file,o,head,check,|_,part,_|out.write_all(part))
}
fn visit_work(file:&File,o:&Original,head:[u8;32],check:impl Fn()->io::Result<()>,
    action:impl FnMut(u64,&[u8],[u8;32])->io::Result<()>)->io::Result<()>{
    content::visit(o.claim,head,|i,part,next|{
        let (at,n)=location(o.claim,i);stream::read_at(file,part,at)?;stream::read_at(file,next,at+n as u64)
    },check,action)
}

pub(super) struct InputHandle{file:File,lease:HeldDir,directory:NativeObject,original:Original}
pub(super) struct Input{file:File,_lease:HeldDir,directory:NativeObject,original:Original,stamp:fs::OriginalStamp,chain:[u8;32]}
impl InputHandle{
    pub(super) fn open(volume:&crate::pool::lease::FileVolume<'_>,dir:HeldDir,original:Original)->io::Result<Self>{
        if !matches!(volume.pool.access,crate::pool::lease::Access::Incoming)||volume.id()!=original.volume{
            return Err(invalid("a read-only FILE selection cannot stage incoming bytes"));
        }
        // Serialize competing slow incoming saves for this single FILE only.
        // Other FILEs and read handles do not acquire this exclusive lock.
        dir.exclusive()?;let (file,_)=dir.anonymous_record()?;
        Ok(Self{file,directory:NativeObject{entity:dir.entity,birth:Birth::read(&dir.file)?},lease:dir,original})
    }
    /// No SYSTEM or volume lock accompanies this anonymous FD into a slow or
    /// disconnected reader; only the actual intended FILE has an incoming lease. No named work, original reservation or ack yet.
    pub(super) fn capture(self,mut input:impl Read)->io::Result<Input>{
        let o=&self.original;let claim=o.claim;let size=physical_size(claim.bytes)?;
        stream::allocate_stream(&self.file,size)?;stream::write_at(&self.file,&header(o),0)?;
        let mut hash=crate::hash::Sha256::new();let mut buf=vec![0;claim_buffer(claim)];
        for i in 0..chunks(claim.bytes){
            let (at,n)=location(claim,i);let part=&mut buf[..n];input.read_exact(part)?;
            hash.update(part)?;stream::write_at(&self.file,part,at)?;
            // Unpublished, private and not authoritative. Replaced below with the next-chain link.
            stream::write_at(&self.file,&crate::hash::digest(part)?,at+n as u64)?;
        }
        loop{match input.read(&mut buf[..1]){
            Ok(0)=>break,Ok(_)=>return Err(invalid("first SAVE has trailing input")),
            Err(e) if e.kind()==io::ErrorKind::Interrupted=>continue,Err(e)=>return Err(e),
        }}
        if hash.finish()!=claim.sha256{return Err(invalid("first SAVE content checksum differs"));}
        drop(buf);
        let mut root=end(claim)?;
        for i in (0..chunks(claim.bytes)).rev(){
            let (at,n)=location(claim,i);let mut digest=[0;32];stream::read_at(&self.file,&mut digest,at+n as u64)?;
            stream::write_at(&self.file,&root,at+n as u64)?;root=link(i,n,digest,root)?;
        }
        stream::write_at(&self.file,&root,FIXED as u64)?;self.file.sync_all()?;
        let stamp=stream::anonymous_stamp(&self.file)?;
        if stamp.stat.size()!=size||read_header(&self.file,o)?!=root{return Err(invalid("anonymous working file differs at seal"));}
        stream_content(&self.file,o,root,io::sink(),||Ok(()))?; // reread actual destination bytes before any named effect
        if stream::anonymous_stamp(&self.file)?!=stamp{return Err(invalid("anonymous working file changed at seal"));}
        Ok(Input{file:self.file,_lease:self.lease,directory:self.directory,original:self.original,stamp,chain:root})
    }
}
fn token(a:&SaveAllocation,n:&NativeObject,chain:[u8;32])->io::Result<[u8;32]>{
    let mut b=b"ZXS/INITIAL-DRAFT-PREPARED\0".to_vec();b.extend_from_slice(&a.digest);n.put(&mut b);b.extend_from_slice(&chain);crate::hash::digest(&b)
}

/// A named, exact saved body selected at this read cut. Pins alone never
/// authorize publication; output authenticates every chunk before writing it.
pub(in crate::ordinary) struct Reader{pin:PinnedRegular,original:Original,material:Material}
impl Reader{
    fn capture(dir:&HeldDir,o:&Original)->io::Result<Option<Self>>{
        let name=leaf(o.op);let Some(pin)=dir.pin_stream(&name,physical_size(o.claim.bytes)?)? else{return Ok(None)};
        // Every original named-working reader, including a recovery verifier
        // running outside SYSTEM, pins the same retirement lease until drop.
        pin.file.try_lock_shared().map_err(io::Error::from)?;pin.unchanged()?;
        let material=Material{object:NativeObject::stamp(&pin.stamp),chain:read_header(&pin.file,o)?};
        pin.named(dir,&name)?;Ok(Some(Self{pin,original:o.clone(),material}))
    }
    pub(in crate::ordinary) fn current(dir:&HeldDir,o:&Original,m:&Material)->io::Result<Self>{
        Self::current_if_named(dir,o,m)?.ok_or_else(||invalid("published working body is unavailable"))
    }
    pub(in crate::ordinary) fn current_if_named(dir:&HeldDir,o:&Original,m:&Material)->io::Result<Option<Self>>{
        let Some(r)=Self::capture(dir,o)? else{return Ok(None)};
        if &r.material!=m{return Err(invalid("published working body native identity or commitment differs"));}
        Ok(Some(r))
    }
    pub(in crate::ordinary) fn visit(&self,action:impl FnMut(u64,&[u8],[u8;32])->io::Result<()>)->io::Result<()>{
        visit_work(&self.pin.file,&self.original,self.material.chain,||self.pin.unchanged(),action)
    }
    pub(in crate::ordinary) fn require(&self,dir:&HeldDir)->io::Result<()>{
        self.pin.named(dir,&leaf(self.original.op))
    }
    pub(in crate::ordinary) fn into_retirement(self,dir:&HeldDir)->io::Result<(CString,PinnedRegular)>{
        let name=leaf(self.original.op);self.pin.named(dir,&name)?;
        self.pin.file.unlock()?;self.pin.file.try_lock().map_err(io::Error::from)?;
        self.pin.named(dir,&name)?;Ok((name,self.pin))
    }
    pub(in crate::ordinary) fn write_to(&self,out:impl Write)->io::Result<()>{
        stream_content(&self.pin.file,&self.original,self.material.chain,out,||self.pin.unchanged())
    }
}

/// Existing named preparation must prove actual data once (outside the global
/// writer). The returned proof can only be reused with the same original
/// reservation, held native inode, unmodified stamp and named parent.
pub(super) struct Verified{reader:Reader,allocation:[u8;32]}
pub(super) struct PendingReader{reader:Reader,allocation:[u8;32]}
impl PendingReader{
    pub(super) fn material(&self)->&Material{&self.reader.material}
    pub(super) fn open(dir:&HeldDir,a:&SaveAllocation)->io::Result<Option<Self>>{
        let Some(reader)=Reader::capture(dir,&a.original)? else{return Ok(None)};
        fs::require_material_token(&reader.pin.file,&token(a,&reader.material.object,reader.material.chain)?)?;
        Ok(Some(Self{reader,allocation:a.digest}))
    }
    pub(super) fn verify(self)->io::Result<Verified>{
        self.reader.write_to(io::sink())?;self.reader.pin.file.sync_all()?;self.reader.pin.unchanged()?;
        Ok(Verified{reader:self.reader,allocation:self.allocation})
    }
}
impl Verified{
    pub(super) fn require(&self,dir:&HeldDir,a:&SaveAllocation)->io::Result<Material>{
        if self.allocation!=a.digest||a.original!=self.reader.original{return Err(invalid("working proof belongs to another original save"));}
        self.reader.pin.file.sync_all()?;self.reader.pin.named(dir,&leaf(a.original.op))?;
        dir.file.sync_all()?;Ok(self.reader.material.clone())
    }
}
impl Input{
    pub(super) fn link(self,dir:&HeldDir,a:&SaveAllocation)->io::Result<Verified>{
        if self.original!=a.original||dir.entity!=self.directory.entity||Birth::read(&dir.file)?!=self.directory.birth||
            stream::anonymous_stamp(&self.file)?!=self.stamp{return Err(invalid("original FILE or incoming anonymous material changed"));}
        let native=NativeObject::stamp(&self.stamp);
        fs::set_preparation_token(&self.file,&token(a,&native,self.chain)?)?;
        dir.link_stream(&leaf(a.original.op),&self.file,&self.stamp,physical_size(a.original.claim.bytes)?)?;
        let r=PendingReader::open(dir,a)?.ok_or_else(||invalid("linked original preparation absent"))?;
        if r.reader.material.object!=native||r.reader.material.chain!=self.chain{return Err(invalid("different working file linked at destination"));}
        // The same anonymous file was fully reread in capture. Its only changes
        // since then are our own create-only token and no-clobber hard link.
        Ok(Verified{reader:r.reader,allocation:a.digest})
    }
}

impl Input{
    pub(super) fn require(&self,dir:&HeldDir,o:&Original)->io::Result<()>{
        if self.original!=*o||dir.entity!=self.directory.entity||Birth::read(&dir.file)?!=self.directory.birth||
            stream::anonymous_stamp(&self.file)?!=self.stamp{return Err(invalid("staged bytes no longer select their original FILE and root"));}
        Ok(())
    }
}
