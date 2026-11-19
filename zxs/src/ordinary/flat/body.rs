//! Physical bytes of one selected portable index. No lookup by filename, no
//! authority to issue or adopt materials and no Web/SMB entry point.
#[cfg(test)]
use super::MaterialRef;
use super::{Index,Leaf,Material,blocks::{Buffers,Reader},stored::Descriptor,local::Located};
use crate::{OperationId, fs::{HeldBytes,invalid}, ordinary::output::CheckedOutput};
#[cfg(test)]
use std::fs::File;
use std::{collections::BTreeMap,io::{self,Write}};

/// Per-request allowance for *additional* raw/check-table file descriptors
/// and checksum directories plus the shared page/block buffers. It is neither
/// an on-disk limit nor a claim about total process RSS or available OS FDs.
#[derive(Clone,Copy)]
pub(in crate::ordinary) struct ReadBudget {
    pub(in crate::ordinary) file_descriptors:usize, pub(in crate::ordinary) buffer_bytes:usize,
}

pub(super) struct Contents {
    table: HeldBytes, index: Index,
    sources: BTreeMap<(OperationId,u32),Located>,
}
impl Contents {
    /// The immutable material references and the index descriptor must be
    /// selected from one original FILE cut. The catalog is metadata only;
    /// we open no payloads, directories or previous versions here.
    pub(super) fn bind(descriptor:Descriptor,table:HeldBytes,
            mut original:impl FnMut(Material)->io::Result<Located>)->io::Result<Self>{
        table.unchanged()?;let index=descriptor.read(table.file())?;
        let mut sources=BTreeMap::new();
        for &m in &index.materials {
            let receipt=original(m)?;
            if receipt.reference().pool!=descriptor.key.pool||receipt.reference().material!=m{
                return Err(invalid("original block-table reference is not this indexed FILE material"));
            }
            if sources.insert(m.key(),receipt).is_some(){return Err(invalid("duplicate original material in portable version"));}
        }
        table.unchanged()?;Ok(Self{table,index,sources})
    }

    pub(super) fn len(&self)->u64{self.index.len()}
    pub(super) fn unchanged(&self)->io::Result<()>{self.table.unchanged()}
    /// A single canonical segment must cover exactly the entire original
    /// extent at physical offset zero. An empty index has NO source; do not
    /// treat it as evidence that any particular empty RAW exists.
    pub(super) fn sole_whole_material(&self)->io::Result<Option<&Located>>{
        self.unchanged()?;
        let [r]=self.index.rows.as_slice() else{return Ok(None)};
        let Some(i)=r.terminal else{return Ok(None)};
        let m=self.index.materials[i as usize];
        if r.offset!=0||r.end!=m.size{return Ok(None);}
        self.sources.get(&m.key()).map(Some).ok_or_else(||invalid("missing original of the complete logical material"))
    }

    /// Select and pin every material this request intersects before emitting
    /// any data, including leading zeroes. Release them after the final flush
    /// and checks (or any error). Another request can reopen only the *same*
    /// original, under its enclosing registration, placement and access lease.
    pub(super) fn read_with(&self,at:u64,n:u64,budget:ReadBudget,headroom:usize,
        mut open:impl FnMut(&Located)->io::Result<(HeldBytes,HeldBytes)>,boundary:impl Fn()->io::Result<()>,mut out:impl Write)->io::Result<()>{
        self.table.unchanged()?;
        let mut selected=BTreeMap::new();
        let(mut directory_bytes,mut page,mut block)=(0usize,0usize,0usize);
        self.index.visit(at,n,|leaf,off,_|{
            let Leaf::Data(m)=leaf else{return Ok(())};
            let original=self.sources.get(&m.key()).ok_or_else(||invalid("not an original material of the frozen version"))?;
            let(dir,p,b)=original.reference().read_bytes(off)?;
            if !selected.contains_key(&m.key()){
                if selected.len()>=budget.file_descriptors.saturating_sub(headroom)/2{return Err(invalid("not enough descriptors for this portable range"));}
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
            readers.insert(key,Reader::open(source.reference(),raw,checks)?);
        }
        let check_all=||->io::Result<()>{
            boundary()?;self.table.unchanged()?;for source in readers.values(){source.unchanged()?;}Ok(())
        };
        check_all()?;
        let mut out=CheckedOutput{out:&mut out,check:||self.table.unchanged()};
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

// File-level algorithms can be exercised independently of name selection.
// No production consumer receives a caller-swappable data/metadata opener.
#[cfg(test)]
impl Contents{
    fn open(descriptor:Descriptor,table:File,mut get:impl FnMut(Material)->io::Result<MaterialRef>)->io::Result<Self>{
        Self::bind(descriptor,HeldBytes::separate_format_fixture(table)?,|m|get(m).map(Located::change))
    }
    fn range_to(&self,at:u64,n:u64,budget:ReadBudget,mut get:impl FnMut(MaterialRef)->io::Result<(File,File)>,out:impl Write)->io::Result<()>{
        self.read_with(at,n,budget,0,|v|{
            let(a,b)=get(v.reference())?;Ok((HeldBytes::separate_format_fixture(a)?,HeldBytes::separate_format_fixture(b)?))
        },||Ok(()),out)
    }
}
#[cfg(test)]
#[path="../../../tests/unit/flat_body.rs"]
mod tests;
