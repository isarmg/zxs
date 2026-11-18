//! Original working cuts keep a constant number of live FDs. The ordered
//! census is bounded by the native SYSTEM journal, not the logical FILE length.
use super::Stored;
use crate::ordinary::output::CheckedOutput;
use crate::{fs::{self,HeldDir,invalid,stream::PinnedRegular},ordinary::{material,wire::Frame,
    interval::{types::{Material,Original,Saved},layout::{From,Layout}},formal::physical::HistoryBody}};
use std::io::{self,Write};

struct Seen{original:Original,material:Material,stamp:fs::OriginalStamp}
pub(in crate::ordinary) struct Operands{
    lease:PinnedRegular,rows:Vec<Seen>,edge:Option<Stored>,layout:Layout,
}
impl Operands{
    pub(in crate::ordinary) fn capture<'a>(dir:&HeldDir,birth:&Frame,base_len:u64,entries:impl Iterator<Item=&'a Saved>)->io::Result<Self>{
        let lease=material::read_lease(dir,&birth.create)?;
        let mut layout=Layout::base(base_len);let mut rows=Vec::new();let mut edge=None;
        for s in entries{
            let o=&s.original;let named=Stored::reopen(dir,o,&s.material)?;
            layout.apply(o.predecessor.length,o.edit,o.result,rows.len())?;
            rows.push(Seen{original:o.clone(),material:s.material.clone(),stamp:named.named.pin.stamp.clone()});
            edge=Some(named); // the preceding non-edge FD is dropped, not kept for every save
        }
        let set=Self{lease,rows,edge,layout};set.quick()?;Ok(set)
    }
    fn quick(&self)->io::Result<()>{
        self.lease.unchanged()?;if let Some(edge)=&self.edge{edge.named.pin.unchanged()?;}Ok(())
    }
    fn open(&self,dir:&HeldDir,i:usize)->io::Result<Stored>{
        let row=self.rows.get(i).ok_or_else(||invalid("unknown original working extent ordinal"))?;
        let r=Stored::reopen(dir,&row.original,&row.material)?;
        if r.named.pin.stamp!=row.stamp{return Err(invalid("original working material changed since this admitted snapshot"));}
        Ok(r)
    }
    /// No arbitrary path, current-view comparison, or raw scan. This census is
    /// against just the frozen names/stamps so a legitimate later save is fine.
    pub(super) fn require(&self,dir:&HeldDir)->io::Result<()>{
        self.lease.named(dir,c"FILE")?;
        for i in 0..self.rows.len(){drop(self.open(dir,i)?);}
        self.quick()
    }
    pub(super) fn verify_indexes(&self,dir:&HeldDir)->io::Result<()>{
        for i in 0..self.rows.len(){self.open(dir,i)?.range(0,0,io::sink())?;}
        self.require(dir)
    }
}
pub(in crate::ordinary) struct Composed{dir:HeldDir,base:HistoryBody,set:Operands}
impl Composed{
    pub(in crate::ordinary) fn new(dir:HeldDir,base:HistoryBody,set:Operands)->io::Result<Self>{
        if set.edge.is_none(){return Err(invalid("a composed FILE must have an actual confirmed working edge"));}
        Ok(Self{dir,base,set})
    }
    pub(in crate::ordinary) fn unchanged(&self)->io::Result<()>{self.base.unchanged()?;self.set.quick()}
    pub(in crate::ordinary) fn finish(&self)->io::Result<()>{self.base.require(&self.dir)?;self.set.require(&self.dir)}
    pub(in crate::ordinary) fn size(&self)->u64{self.set.layout.len()}
    fn piece(&self,from:From,offset:u64,n:u64,mut out:impl Write)->io::Result<()>{
        let mut out=CheckedOutput{out:&mut out,check:||self.unchanged()};
        match from{
            From::Zero=>{
                let zeros=[0u8;64*1024];let mut left=n;
                while left>0{let take=left.min(zeros.len() as u64) as usize;out.write_all(&zeros[..take])?;left-=take as u64;}Ok(())
            }
            From::Base=>self.base.write_range(offset,n,&mut out),
            From::Extent(i)=>{
                if i+1==self.set.rows.len(){
                    self.set.edge.as_ref().ok_or_else(||invalid("missing working edge"))?.range(offset,n,&mut out)
                }else{
                    let operand=self.set.open(&self.dir,i)?;
                    operand.range(offset,n,CheckedOutput{out:&mut out,check:||operand.named.pin.unchanged()})
                }
            }
        }
    }
    pub(in crate::ordinary) fn write_range(&self,at:u64,len:u64,mut out:impl Write)->io::Result<()>{
        self.unchanged()?;self.set.layout.visit(at,len,|from,off,n|self.piece(from,off,n,&mut out))?;
        if len==0{
            self.base.write_range(0,0,io::sink())?;
            self.set.edge.as_ref().ok_or_else(||invalid("missing working edge"))?.range(0,0,io::sink())?;
        }
        self.finish()
    }
    /// Formal submission, unlike ordinary save/read-range, checks all *contributing*
    /// original bytes. Only the true Changed slices are written into a new INCR.
    pub(in crate::ordinary) fn contributing(&self,mut accept:impl FnMut(bool,&[u8])->io::Result<()>)->io::Result<()>{
        struct Feed<'a,F>{changed:bool,accept:&'a mut F}
        impl<F:FnMut(bool,&[u8])->io::Result<()>> Write for Feed<'_,F>{
            fn write(&mut self,b:&[u8])->io::Result<usize>{
                // This callback may update a hash or a private positional target.
                // An interrupted input here is not a request to replay those effects.
                (self.accept)(self.changed,b).map_err(|e|if e.kind()==io::ErrorKind::Interrupted{io::Error::other(e)}else{e})?;Ok(b.len())
            }
            fn flush(&mut self)->io::Result<()>{Ok(())}
        }
        self.unchanged()?;self.set.layout.visit(0,self.size(),|from,off,n|{
            self.piece(from,off,n,Feed{changed:matches!(from,From::Extent(_)),accept:&mut accept})
        })?;self.finish()
    }
}
