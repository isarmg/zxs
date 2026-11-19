//! Physical bytes of one selected portable index. No lookup by filename, no
//! authority to issue or adopt materials and no Web/SMB entry point.
use super::{Index,Leaf,Material,MaterialRef,blocks::{Buffers,Reader,Stamp},stored::Descriptor};
use crate::{OperationId, fs::invalid, ordinary::output::CheckedOutput};
use std::{collections::BTreeMap,fs::File,io::{self,Write}};

/// Per-request allowance for *additional* raw/check-table file descriptors
/// and checksum directories plus the shared page/block buffers. It is neither
/// an on-disk limit nor a claim about total process RSS or available OS FDs.
#[derive(Clone,Copy)]
pub(in crate::ordinary) struct ReadBudget {
    pub(in crate::ordinary) file_descriptors:usize, pub(in crate::ordinary) buffer_bytes:usize,
}

pub(in crate::ordinary) struct Contents {
    table: File, stamp: Stamp, index: Index,
    sources: BTreeMap<(OperationId,u32),MaterialRef>,
}
impl Contents {
    /// The immutable material references and the index descriptor must be
    /// selected from one original FILE cut. The catalog is metadata only;
    /// we open no payloads, directories or previous versions here.
    pub(in crate::ordinary) fn open(descriptor:Descriptor,table:File,
            mut original:impl FnMut(Material)->io::Result<MaterialRef>)->io::Result<Self>{
        let stamp=Stamp::read(&table)?;let index=descriptor.read(&table)?;
        let mut sources=BTreeMap::new();
        for &m in &index.materials {
            let receipt=original(m)?;
            if receipt.pool!=descriptor.key.pool||receipt.material!=m{
                return Err(invalid("original block-table reference is not this indexed FILE material"));
            }
            if sources.insert(m.key(),receipt).is_some(){return Err(invalid("duplicate original material in portable version"));}
        }
        stamp.require(&table)?;
        Ok(Self{table,stamp,index,sources})
    }

    /// Select and pin every material this request intersects before emitting
    /// any data, including leading zeroes. Release them after the final flush
    /// and checks (or any error). Another request can reopen only the *same*
    /// original, under its enclosing registration, placement and access lease.
    pub(in crate::ordinary) fn range_to(&self,at:u64,n:u64,budget:ReadBudget,
        mut open:impl FnMut(MaterialRef)->io::Result<(File,File)>,mut out:impl Write)->io::Result<()>{
        self.stamp.require(&self.table)?;
        let mut selected=BTreeMap::new();
        let(mut directory_bytes,mut page,mut block)=(0usize,0usize,0usize);
        self.index.visit(at,n,|leaf,off,_|{
            let Leaf::Data(m)=leaf else{return Ok(())};
            let original=*self.sources.get(&m.key()).ok_or_else(||invalid("not an original material of the frozen version"))?;
            let(dir,p,b)=original.read_bytes(off)?;
            if !selected.contains_key(&m.key()){
                if selected.len()>=budget.file_descriptors/2{return Err(invalid("not enough descriptors for this portable range"));}
                directory_bytes=directory_bytes.checked_add(dir).ok_or_else(||invalid("portable directory size overflow"))?;
                selected.insert(m.key(),original);
            }
            page=page.max(p);block=block.max(b);
            if directory_bytes.checked_add(page).and_then(|v|v.checked_add(block)).is_none_or(|v|v>budget.buffer_bytes){
                return Err(invalid("not enough buffer bytes for this portable range"));}
            Ok(())
        })?;
        let mut buffers=Buffers::new(page,block)?;
        let mut readers=BTreeMap::new();
        for(key,source) in selected{
            let(raw,checks)=open(source)?;
            readers.insert(key,Reader::open(source,raw,checks)?);
        }
        let check_all=||->io::Result<()>{
            self.stamp.require(&self.table)?;for source in readers.values(){source.unchanged()?;}Ok(())
        };
        check_all()?;
        let mut out=CheckedOutput{out:&mut out,check:||self.stamp.require(&self.table)};
        self.index.visit(at,n,|leaf,off,len|{
            match leaf{
                Leaf::Zero=>{
                    static ZERO:[u8;64*1024]=[0;64*1024];let mut remain=len;
                    while remain!=0{let count=remain.min(ZERO.len() as u64) as usize;
                        out.write_all(&ZERO[..count])?;remain-=count as u64;}
                    Ok(())
                }
                Leaf::Data(m)=>readers.get(&m.key()).ok_or_else(||invalid("unselected portable material"))?
                    .range(off,len,&mut buffers,&mut out),
            }
        })?;
        check_all()?;out.flush()?;check_all()
    }
}

#[cfg(test)]
#[path="../../../tests/unit/flat_body.rs"]
mod tests;
