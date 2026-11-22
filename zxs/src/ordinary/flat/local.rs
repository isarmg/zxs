//! The physical half of a portable version lookup. Start from the caller's
//! *already selected* ordinary FILE container; no user path, guessed name,
//! old volume fallback, write permission or network authority is returned.
use super::{Material,MaterialRef,body::{Contents,ReadBudget},stored::{Descriptor,VersionKey}};
use crate::{VersionStorageId,fs::{HeldBytes,HeldDir,WitnessRead,invalid},ordinary::storage_name::FileName};
use std::{ffi::CString,io::{self,Write}};

/// Selected alongside the block-table commitment from the material's own
/// original issue. A complete RAW retains its own creation-time storage name;
/// a changed extent's role name does not pretend to be a complete version.
pub(in crate::ordinary) struct Located{original:MaterialRef,raw_name:Option<CString>}
impl Located{
    pub(in crate::ordinary) fn change(original:MaterialRef)->Self{Self{original,raw_name:None}}
    pub(in crate::ordinary) fn full(original:MaterialRef,storage:VersionStorageId,creation_name:&str)->io::Result<Self>{
        let storage=VersionStorageId::checked(storage.0)?;
        let name=FileName::parse(creation_name.as_bytes())?.display();
        let raw=CString::new(format!("{storage}-{name}")).map_err(|_|invalid("invalid original complete version name"))?;
        Ok(Self{original,raw_name:Some(raw)})
    }
    pub(super) fn reference(&self)->MaterialRef{self.original}

    pub(super) fn names(&self)->(CString,CString,CString,CString){
        let m=self.original.material;let [data,pin,table,pin_table]=Self::change_names(m.issuer,m.slot);
        (self.raw_name.clone().unwrap_or(data),pin,table,pin_table)
    }
    pub(super) fn change_names(op:crate::OperationId,slot:u32)->[CString;4]{
        let id=format!("{op}-{slot:08x}");
        let n=|s:String|CString::new(s).expect("only fixed literals and encoded IDs");
        [n(format!("change-{id}")),n(format!("pin-data-{id}")),n(format!("checks-{id}")),n(format!("pin-checks-{id}"))]
    }
    pub(super) fn open_pair(&self,scope:&WitnessRead<'_>)->io::Result<(HeldBytes,HeldBytes)>{
        let(data,wd,checks,wc)=self.names();
        Ok((scope.open(&data,&wd,self.original.material.size)?,
            scope.open(&checks,&wc,self.original.table_len()?)?))
    }
}

fn index_names(key:VersionKey)->(CString,CString){index_leaves(key.node,key.representation)}
fn index_leaves(node:crate::NodeId,generation:u64)->(CString,CString){
    let suffix=format!("{node}-{generation:016x}");
    (CString::new(format!("index-{suffix}")).expect("encoded index name"),
     CString::new(format!("pin-index-{suffix}")).expect("encoded index witness"))
}

/// Geometry only. The longest fixed-width native role is the index witness;
/// the alternative is a RAW's fixed storage prefix plus this exact spelling.
/// No publication, capacity or permission can be inferred from this check.
pub(in crate::ordinary) fn require_resident_names(b:crate::fs::NameBudget,display:&str)->io::Result<()>{
    let(_,longest)=index_leaves(crate::NodeId([0xff;16]),u64::MAX);
    b.child(&longest)?;
    let name=FileName::parse(display.as_bytes())?.display();
    let raw=CString::new(format!("{}-{name}",crate::VersionStorageId::checked([b'f';15])?))
        .map_err(|_|invalid("invalid complete-version storage component"))?;
    b.child(&raw)?;Ok(())
}

/// Physical read only. Whoever makes this selection must keep the original
/// registered pool/FILE, authenticated version and read authority for the
/// operation. Even matching both links cannot detect a replacement of *both*
/// by a coordinated clone, or show which of two complete media is current.
pub(in crate::ordinary) struct LocalVersion<'d>{parent:&'d HeldDir,contents:Contents}
impl<'d> LocalVersion<'d>{
    pub(in crate::ordinary) fn open(parent:&'d HeldDir,descriptor:Descriptor,
            origins:impl FnMut(Material)->io::Result<Located>)->io::Result<Self>{
        let descriptor=Descriptor::declared(descriptor.key,descriptor.logical_bytes,descriptor.index_bytes,descriptor.index_sha())?;
        let scope=WitnessRead::at(parent)?;let(main,witness)=index_names(descriptor.key);
        let table=scope.open(&main,&witness,descriptor.index_bytes)?;
        let contents=Contents::bind(descriptor,table,origins)?;WitnessRead::require_parent(parent)?;
        Ok(Self{parent,contents})
    }
    /// Equality of content commitments derived from the complete RAW's
    /// authenticated checksum metadata, not verification of every body byte.
    /// A partial RAW, RAW plus zeros, changes or an empty map never use this
    /// shortcut. The metadata I/O and FD budget still have to fit.
    pub(in crate::ordinary) fn whole_raw_commitment(&self,budget:ReadBudget)->io::Result<Option<super::logical::LogicalV1>>{
        WitnessRead::require_parent(self.parent)?;
        let Some(located)=self.contents.sole_whole_material()?.filter(|l|l.raw_name.is_some()) else{return Ok(None)};
        let original=located.reference();
        if budget.file_descriptors<2+WitnessRead::OPEN_HEADROOM || budget.buffer_bytes<original.read_bytes(0)?.0 {
            return Err(invalid("insufficient budget to verify the original complete RAW directory"));}
        let scope=WitnessRead::at(self.parent)?;let(raw,table)=located.open_pair(&scope)?;
        let reader=super::blocks::Reader::open(original,raw,table)?;
        let result=reader.material_commitment()?;self.contents.unchanged()?;WitnessRead::require_parent(self.parent)?;
        Ok(Some(result))
    }

    /// Explicit full logical read. Compare to the originating node's distinct
    /// LogicalV1, after every intersecting real block has passed verification
    /// and the complete sink has flushed. Not called by ordinary range reads
    /// or per-SAVE; explicit verification does not publish a version.
    pub(in crate::ordinary) fn verify_complete(&self,expected:super::logical::LogicalV1,budget:ReadBudget)->io::Result<()>{
        let length=self.contents.len();
        if length!=expected.length(){return Err(invalid("logical verification length differs from confirmed node"));}
        let mut whole=super::logical::Whole::new(length)?;
        self.range_to(0,length,budget,&mut whole)?;
        if whole.finish()?!=expected{return Err(invalid("complete logical content differs from confirmed node"));}
        Ok(())
    }

    pub(in crate::ordinary) fn range_to(&self,at:u64,n:u64,budget:ReadBudget,out:impl Write)->io::Result<()>{
        let mut scope=None;
        self.contents.read_with(at,n,budget,WitnessRead::OPEN_HEADROOM,|located|{
            if scope.is_none(){scope=Some(WitnessRead::at(self.parent)?);}
            located.open_pair(scope.as_ref().expect("observed the selected FILE parent"))
        },||WitnessRead::require_parent(self.parent),out)
    }
}


#[cfg(test)]
#[path="../../../tests/unit/flat_local.rs"]
mod tests;

#[cfg(test)]
include!("../../../tests/unit/attached/ordinary/flat/local/items.rs");
