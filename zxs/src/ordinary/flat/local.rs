//! The physical half of a portable version lookup. Start from the caller's
//! *already selected* ordinary FILE container; no user path, guessed name,
//! old volume fallback, write permission or network authority is returned.
use super::{Material,MaterialRef,body::{Contents,ReadBudget},stored::{Descriptor,VersionKey}};
use crate::{VersionStorageId,fs::{HeldBytes,HeldDir,WitnessRead,invalid},ordinary::storage_name::FileName};
#[cfg(test)]
use std::fs::File;
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

    fn names(&self)->(CString,CString,CString,CString){
        let m=self.original.material;let id=format!("{}-{:08x}",m.issuer,m.slot);
        let n=|s:String|CString::new(s).expect("only fixed literals and encoded IDs");
        (self.raw_name.clone().unwrap_or_else(||n(format!("change-{id}"))),n(format!("pin-data-{id}")),
         n(format!("checks-{id}")),n(format!("pin-checks-{id}")))
    }
    fn open_pair(&self,scope:&WitnessRead<'_>)->io::Result<(HeldBytes,HeldBytes)>{
        let(data,wd,checks,wc)=self.names();
        Ok((scope.open(&data,&wd,self.original.material.size)?,
            scope.open(&checks,&wc,self.original.table_len()?)?))
    }
}

fn index_names(key:VersionKey)->(CString,CString){
    let suffix=format!("{}-{:016x}",key.node,key.representation);
    (CString::new(format!("index-{suffix}")).expect("encoded index name"),
     CString::new(format!("pin-index-{suffix}")).expect("encoded index witness"))
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
