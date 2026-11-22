//! Source-only crash recovery for a completed inter-volume handoff.
//! These proof bytes may be installed only with that transfer's SQL receipt.
//! No client path, anonymous claim or filesystem-specific handle is used.
use super::*;
use std::{collections::BTreeSet,os::unix::fs::FileExt};

const WIDTH:usize=56;
pub(crate) const MAX_RETIRE_PROOF:usize=10+8+8+2+WIDTH+(DIRECTORY_NAMES-1)/2*(17+WIDTH);
const TAG:&[u8;10]=b"ZXS-TRIM1\0";
fn stale()->io::Error{invalid("old FILE retirement cannot prove the original isolated object")}
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
struct Edge([u8;WIDTH]);
impl Edge{
    fn read(file:&File,links:u64)->io::Result<Self>{
        let m=file.metadata()?;
        if !m.is_file()||m.nlink()!=links||m.ino()==0||m.uid()!=local_uid()||m.mode()&0o7777!=0o600{
            return Err(stale());
        }
        let mut b=[0;WIDTH];
        for (i,v) in [m.ino(),m.len(),m.mtime() as u64,m.mtime_nsec() as u64,m.ctime() as u64,m.ctime_nsec() as u64].into_iter().enumerate(){
            b[i*8..i*8+8].copy_from_slice(&v.to_be_bytes());
        }
        b[48..52].copy_from_slice(&m.uid().to_be_bytes());b[52..56].copy_from_slice(&m.gid().to_be_bytes());Ok(Self(b))
    }
    fn recheck(self,file:&File,links:u64)->io::Result<()>{
        let next=Self::read(file,links)?;
        // Only this retirement's ordered first unlink may change ctime.
        if self.0[..32]!=next.0[..32]||self.0[48..]!=next.0[48..]||
           (links==2&&self.0[32..48]!=next.0[32..48]){return Err(stale());}Ok(())
    }
    fn still_target(self,file:&File)->io::Result<Self>{
        let now=Self::read(file,2)?;
        let was=u64::from_be_bytes(self.0[8..16].try_into().unwrap());
        let len=u64::from_be_bytes(now.0[8..16].try_into().unwrap());
        // A subsequent confirmed version or staged tail may only append
        // to this same working pair. It cannot replace its sealed prefix.
        if self.0[..8]!=now.0[..8]||self.0[48..]!=now.0[48..]||len<was||(len==was&&self!=now){return Err(stale());}
        Ok(now)
    }
    fn pin(self,parent:&HeldDir,name:&CStr,listed:bool,links:u64)->io::Result<Option<File>>{
        let f=parent.pin_record(name)?;
        if f.is_some()!=listed{return Err(stale());}
        if let Some(f)=f.as_ref(){
            if !parent.shares_file_mount(f)?{return Err(stale());}
            self.recheck(f,links)?;
        }
        Ok(f)
    }
}
fn set(dir:&HeldDir)->io::Result<BTreeSet<Vec<u8>>>{Ok(dir.names()?.into_iter().collect())}
type RoleEdge=(crate::OperationId,u8,Edge);
#[derive(Clone,Copy,Eq,PartialEq)]
pub(crate) enum RetireCut{AfterRenameBeforeSync,AfterFirstNameBeforeSync}
#[derive(Clone,Copy)]
pub(crate) struct RetireSite<'a>{
    pub(crate) root:&'a HeldDir,pub(crate) original:&'a CStr,pub(crate) isolation:&'a CStr,
    pub(crate) portable:&'a CStr,pub(crate) witness:&'a CStr,pub(crate) header:&'a [u8],pub(crate) has_parent:bool,
}
#[derive(Eq,PartialEq)]
pub(crate) struct RetireProof{
    // Inodes here supplement an already qualified root marker, exact private
    // namespace and originally held paired FILE proof. They are not volume
    // identity; unstable remount geometry never triggers removal by name.
    container:u64,outer:u64,header:Edge,parts:Vec<RoleEdge>,
}
fn roles(op:crate::OperationId,class:u8)->[CString;2]{
    let [a,b,c,d]=crate::ordinary::working_round::names(op);if class==0{[a,b]}else{[c,d]}
}
impl RetireProof{
    pub(crate) fn capture(parent:&HeldDir,dir:&HeldDir,header:&HeldBytes,outside:&CStr)->io::Result<Self>{
        header.require_name(dir,c"FILE")?;header.require_name(parent,outside)?;
        let header=Edge::read(header.file(),2)?;
        let mut unselected=set(dir)?;
        if !unselected.remove(c"FILE".to_bytes()){return Err(stale());}
        let mut parts=Vec::new();let scope=WitnessRead::at(dir)?;
        for raw in unselected.clone(){
            let Some(rest)=raw.strip_prefix(b"change-").or_else(||raw.strip_prefix(b"checks-")) else{continue;};
            let Some(id)=rest.strip_suffix(b"-00000000") else{return Err(stale());};
            let op=std::str::from_utf8(id).map_err(|_|stale())?.parse()?;
            let class=u8::from(raw.starts_with(b"checks-"));
            let [a,b]=roles(op,class);
            if !unselected.remove(a.to_bytes())||!unselected.remove(b.to_bytes()){return Err(stale());}
            let pin=dir.pin_record(&a)?.ok_or_else(stale)?;
            let pair=scope.open(&a,&b,pin.metadata()?.len())?;
            parts.push((op,class,Edge::read(pair.file(),2)?));
        }
        if !unselected.is_empty(){return Err(stale());} // no removal of an unpaired/foreign name
        parts.sort_unstable_by_key(|&(o,r,_)|(o,r));
        Ok(Self{container:dir.entity.inode,outer:parent.entity.inode,header,parts})
    }
    pub(crate) fn from_verified(parent:&HeldDir,dir:&HeldDir,header:&HeldBytes,outside:&CStr,
        used:&crate::ordinary::working_round::Used)->io::Result<Self>{
        header.require_name(dir,c"FILE")?;header.require_name(parent,outside)?;
        let mut parts=Vec::new();
        // Exactly the stamps from the independently reopened, verified
        // destination, never a second fstat baseline from the same spelling.
        for (round,data,checks) in used.witnessed_rounds(){
            parts.push((round,0,Edge(data.retirement_stamp())));
            parts.push((round,1,Edge(checks.retirement_stamp())));
        }
        let out=Self{container:dir.entity.inode,outer:parent.entity.inode,header:Edge(header.stamp().retirement_stamp()),parts};
        if out.parts.len()>(DIRECTORY_NAMES-1)/2{return Err(stale());}
        Ok(out)
    }
    pub(crate) fn encode(&self)->Vec<u8>{
        let mut b=Vec::with_capacity(84+self.parts.len()*73);
        b.extend(TAG);b.extend(self.container.to_be_bytes());b.extend(self.outer.to_be_bytes());
        b.extend((self.parts.len() as u16).to_be_bytes());b.extend(self.header.0);
        for &(op,role,stamp) in &self.parts{b.extend(op.0);b.push(role);b.extend(stamp.0);}
        b
    }
    pub(crate) fn decode(bytes:&[u8])->io::Result<Self>{
        let bytes=bytes.strip_prefix(TAG).ok_or_else(stale)?;
        let (fixed,roles)=bytes.split_at_checked(18+WIDTH).ok_or_else(stale)?;
        let n=u16::from_be_bytes(fixed[16..18].try_into().unwrap()) as usize;
        if n>(DIRECTORY_NAMES-1)/2||roles.len()!=n*(17+WIDTH){return Err(stale());}
        let mut parts=Vec::with_capacity(n);
        for b in roles.as_chunks::<{17+WIDTH}>().0{
            let op=crate::OperationId(b[..16].try_into().unwrap());let class=b[16];
            if op.0==[0;16]||class>1||parts.last().is_some_and(|&(o,r,_)|(o,r)>=(op,class)){return Err(stale());}
            parts.push((op,class,Edge(b[17..].try_into().unwrap())));
        }
        let out=Self{container:u64::from_be_bytes(fixed[..8].try_into().unwrap()),outer:u64::from_be_bytes(fixed[8..16].try_into().unwrap()),
            header:Edge(fixed[18..].try_into().unwrap()),parts};
        if out.container==0||out.outer==0{return Err(stale());}Ok(out)
    }
    pub(crate) fn is_body(&self,dir:&HeldDir)->bool{self.container==dir.entity.inode}
    pub(crate) fn requires_target(&self,parent:&HeldDir,body:&HeldDir,witness:&CStr)->io::Result<()>{
        self.outer_matches(parent)?;self.body_matches(body)?;
        self.header.pin(body,c"FILE",true,2)?;self.header.pin(parent,witness,true,2)?;
        let names=set(body)?;
        for &(op,class,edge) in &self.parts{
            let [a,b]=roles(op,class);
            if !names.contains(a.to_bytes())||!names.contains(b.to_bytes()){return Err(stale());}
            let get=|n:&CStr|->io::Result<Edge>{
                let f=body.pin_record(n)?.ok_or_else(stale)?;
                if !body.shares_file_mount(&f)?{return Err(stale());}
                edge.still_target(&f)
            };
            if get(&a)?!=get(&b)?{return Err(stale());}
        }
        self.header.pin(parent,witness,true,2)?;Ok(())
    }
    fn outer_matches(&self,outer:&HeldDir)->io::Result<()>{
        WitnessRead::require_parent(outer)?;
        if outer.entity.inode!=self.outer{return Err(stale());}Ok(())
    }
    fn body_matches(&self,body:&HeldDir)->io::Result<()>{
        WitnessRead::require_parent(body)?;
        if body.entity.inode!=self.container{return Err(stale());}Ok(())
    }
    fn exclusive_header(&self,outer:&HeldDir,outside:&CStr,inside:bool,bytes:&[u8])->io::Result<Option<File>>{
        self.outer_matches(outer)?;
        let ls=set(outer)?;let links=if inside{2}else{1};
        let Some(pin)=self.header.pin(outer,outside,ls.contains(outside.to_bytes()),links)? else{
            if inside{return Err(stale());}return Ok(None)
        };
        // Pin an exact private ordinary inode before reopening it for FILE EX.
        let fd=unsafe{zxs_open_pinned_record(pin.as_raw_fd(),0)};
        if fd<0{return Err(io::Error::last_os_error());}
        let held=unsafe{File::from_raw_fd(fd)};
        held.try_lock().map_err(io::Error::from)?;
        let mut check=vec![0;bytes.len()];
        if held.metadata()?.len()!=bytes.len() as u64{return Err(stale());}
        self.header.recheck(&held,links)?;held.read_exact_at(&mut check,0)?;
        if check!=bytes{return Err(stale());}
        self.header.pin(outer,outside,true,links)?;Ok(Some(held))
    }
    fn require_members(&self,body:&HeldDir)->io::Result<BTreeSet<Vec<u8>>>{
        self.body_matches(body)?;
        let ls=set(body)?;let mut expected=BTreeSet::from([b"FILE".to_vec()]);
        for &(op,class,_) in &self.parts{for n in roles(op,class){expected.insert(n.into_bytes());}}
        if !ls.is_subset(&expected){return Err(stale());}Ok(ls)
    }
    /// Only an originally paired link is removed. Naming is sampled from a
    /// private held parent; the matching O_PATH inode remains held over unlink.
    fn unlink(&self,dir:&HeldDir,name:&CStr,edge:Edge,links:u64,gate:&impl Fn()->io::Result<()>,stop:&impl Fn()->io::Result<()>,before_sync:impl FnOnce()->io::Result<()>)->io::Result<()>{
        dir.require_named()?;let f=edge.pin(dir,name,true,links)?.ok_or_else(stale)?;
        gate()?;edge.recheck(&f,links)?;edge.pin(dir,name,true,links)?;
        stop()?;
        os_result(unsafe{zxs_remove_name(dir.file.as_raw_fd(),name.as_ptr())})?;
        before_sync()?;dir.confirm_absence(name)
    }
    /// Pin the exact source FILE while it is still visible, before
    /// qualifying the current destination. Compatible old readers alone
    /// can defer retirement; the current writer never has to wait for us.
    pub(crate) fn lease(&self,s:&RetireSite<'_>)->io::Result<Option<File>>{
        let names=set(s.root)?;
        let prev=s.root.directory_in_sample(s.original,&names)?;
        let hidden=s.root.directory_in_sample(s.isolation,&names)?;
        if prev.is_some()&&hidden.is_some(){return Err(stale());}
        let held=prev.as_ref().or(hidden.as_ref());
        let outer=if s.has_parent{let Some(d)=held else{return Ok(None);};d}else{s.root};
        let nested=if s.has_parent{outer.exact_directory(s.portable)?}else{None};
        let body=if s.has_parent{nested.as_ref()}else{held};
        let inside=match body{
            Some(body)=>{
                self.body_matches(body)?;
                let h=body.entry_present(c"FILE")?;
                if !h&&prev.is_some(){return Err(stale());}h
            },
            None if prev.is_some()=>return Err(stale()),
            None=>false,
        };
        self.exclusive_header(outer,s.witness,inside,s.header)
    }
    fn require_header(&self,outer:&HeldDir,body:Option<&HeldDir>,s:&RetireSite<'_>,lease:Option<&File>)->io::Result<()>{
        self.outer_matches(outer)?;
        let inside=match body{
            Some(body)=>{
                let ls=self.require_members(body)?;
                let h=ls.contains(b"FILE".as_slice());
                if h{self.header.pin(body,c"FILE",true,2)?;}else if !ls.is_empty(){return Err(stale());}h
            },
            None=>false,
        };
        if let Some(lease)=lease{
            let links=if inside{2}else{1};
            self.header.recheck(lease,links)?;
            self.header.pin(outer,s.witness,true,links)?;
        }else if inside||outer.entry_present(s.witness)?{return Err(stale());}
        Ok(())
    }
    /// For gen1 isolate the old body on the same F without adding a new
    /// path depth; for later generations rename its private parent so the
    /// portable inner FILE container name stays byte-for-byte unchanged.
    pub(crate) fn isolate(&self,s:&RetireSite<'_>,lease:Option<&File>,gate:&impl Fn()->io::Result<()>,
        stop:&impl Fn()->io::Result<()>,point:&impl Fn(RetireCut)->io::Result<()>)->io::Result<()>{
        let names=set(s.root)?;
        let original=s.root.directory_in_sample(s.original,&names)?;
        let isolated=s.root.directory_in_sample(s.isolation,&names)?;
        match (original.as_ref(),isolated.as_ref()){
            (Some(old),None)=>{
                let nested=if s.has_parent{old.exact_directory(s.portable)?}else{None};
                let body=if s.has_parent{nested.as_ref().ok_or_else(stale)?}else{old};
                let outer=if s.has_parent{old}else{s.root};
                self.require_header(outer,Some(body),s,lease)?;
                let names=self.require_members(body)?;
                if names.len()!=1+self.parts.len()*2{return Err(stale());}
                for &(op,class,edge) in &self.parts{
                    for n in roles(op,class){edge.pin(body,&n,true,2)?;}
                }
                gate()?;s.root.require_named()?;
                let now=s.root.exact_directory(s.original)?.ok_or_else(stale)?;
                if now.entity!=old.entity||s.root.entry_present(s.isolation)?{return Err(stale());}
                // After both relative names and the original lease, there
                // is no potentially blocking proof between stop and rename.
                stop()?;
                os_result(unsafe{zxs_rename_fresh(s.root.file.as_raw_fd(),s.original.as_ptr(),s.root.file.as_raw_fd(),s.isolation.as_ptr())})?;
                point(RetireCut::AfterRenameBeforeSync)?;
                Self::sync_rename(s,old)
            },
            (None,Some(hidden))=>{
                // A visible prior rename might not have reached its
                // parent fsync. Reprove original physical identity and
                // complete that sync before the first destructive unlink.
                let nested=if s.has_parent{hidden.exact_directory(s.portable)?}else{None};
                let body=if s.has_parent{nested.as_ref()}else{Some(hidden)};
                self.require_header(if s.has_parent{hidden}else{s.root},body,s,lease)?;
                gate()?;Self::sync_rename(s,hidden)
            },
            (None,None)=>Ok(()), // interrupted only after all named source data is gone
            (Some(_),Some(_))=>Err(stale()),
        }
    }
    fn sync_rename(s:&RetireSite<'_>,old:&HeldDir)->io::Result<()>{
        s.root.file.sync_all()?;
        let got=s.root.exact_directory(s.isolation)?.ok_or_else(stale)?;
        if got.entity!=old.entity||s.root.entry_present(s.original)?{return Err(stale());}Ok(())
    }
    /// The first name's absence is durable before removing its witness.
    pub(crate) fn remove(&self,s:&RetireSite<'_>,lease:Option<&File>,gate:&impl Fn()->io::Result<()>,
        stop:&impl Fn()->io::Result<()>,point:&impl Fn(RetireCut)->io::Result<()>)->io::Result<()>{
        let isolated=s.root.exact_directory(s.isolation)?;
        let outer=if s.has_parent{match &isolated{Some(i)=>i,None=>return s.root.confirm_absence(s.isolation)}}else{s.root};
        let placed=if s.has_parent{outer.exact_directory(s.portable)?}else{None};
        let inner=if s.has_parent{placed.as_ref()}else{isolated.as_ref()};
        self.require_header(outer,inner,s,lease)?;
        let name=if s.has_parent{s.portable}else{s.isolation};
        if let Some(body)=inner{
            body.exclusive()?;
            let mut remaining=self.require_members(body)?;
            let inside=remaining.contains(b"FILE".as_slice());
            if !inside&&!remaining.is_empty(){return Err(stale());}
            if inside{self.header.pin(body,c"FILE",true,2)?;}
            for &(op,class,edge) in &self.parts{
                let [a,b]=roles(op,class);
                let(a_present,b_present)=(remaining.remove(a.to_bytes()),remaining.remove(b.to_bytes()));
                if a_present&&!b_present{return Err(stale());}
                let links=if a_present{2}else{1};
                edge.pin(body,&a,a_present,links)?;edge.pin(body,&b,b_present,links)?;
                if a_present{
                    self.unlink(body,&a,edge,2,gate,stop,||point(RetireCut::AfterFirstNameBeforeSync))?;
                }else if b_present{
                    // An earlier unlink can be visible after error or
                    // process loss but still not durable across power loss.
                    body.confirm_absence(&a)?;
                }
                if b_present{self.unlink(body,&b,edge,1,gate,stop,||Ok(()))?;}
            }
            if inside{
                if lease.is_none(){return Err(stale());}
                // Cold replay may also see both roles absent before their
                // previous parent sync happened. Do not remove FILE until
                // all material-name removals are durable on this parent.
                body.file.sync_all()?;body.only(Some(c"FILE"))?;
                self.unlink(body,c"FILE",self.header,2,gate,stop,||point(RetireCut::AfterFirstNameBeforeSync))?;
            }
            self.rmdir(outer,name,body,gate,stop)?;
        }else{
            outer.confirm_absence(name)?; // container removal before outer witness
        }
        if lease.is_some(){self.unlink(outer,s.witness,self.header,1,gate,stop,||Ok(()))?;}
        if s.has_parent{self.rmdir(s.root,s.isolation,outer,gate,stop)?;}
        Ok(())
    }
    fn rmdir(&self,parent:&HeldDir,name:&CStr,old:&HeldDir,gate:&impl Fn()->io::Result<()>,stop:&impl Fn()->io::Result<()>)->io::Result<()>{
        let named=parent.exact_directory(name)?.ok_or_else(stale)?;
        if named.entity!=old.entity||!named.same_mount(old)?{return Err(stale());}
        old.only(None)?;old.file.sync_all()?;gate()?;
        if parent.child_directory(name)?.is_none_or(|n|n.entity!=old.entity){return Err(stale());}
        stop()?;
        os_result(unsafe{zxs_remove_empty_dir(parent.file.as_raw_fd(),name.as_ptr())})?;
        parent.confirm_absence(name)
    }
}
