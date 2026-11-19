//! Exact, case-sensitive names for the single-name container layout.
//! Parsing a spelling never grants access, identifies an object on disk or
//! selects a service location. Only the current registered format uses these names.
use crate::{FileId,OperationId,fs::invalid};
use std::{ffi::{CStr,CString},io};

pub(super) const COMPONENT_MAX:usize=255;
const ID_HEX:usize=32;
pub(super) const FILE_NAME_MAX:usize=COMPONENT_MAX-1-ID_HEX;
const STAGE_PREFIX:&str=".zxs-new-";
const ISOLATE_PREFIX:&str=".zxs-moveout-";
const TICKETS:u8=8;
pub(super) const STAGE_LEN:usize=STAGE_PREFIX.len()+ID_HEX+2;
pub(super) const ISOLATE_LEN:usize=ISOLATE_PREFIX.len()+ID_HEX+1;
const _:()=assert!(FILE_NAME_MAX==222&&STAGE_LEN==43&&ISOLATE_LEN==46);
const _:()=assert!(TICKETS==super::allocation::SLOTS);

fn component(raw:&[u8],max:usize)->io::Result<&str>{
    if raw.is_empty()||raw.len()>max{return Err(invalid("display name exceeds its component byte budget or is empty"));}
    let s=std::str::from_utf8(raw).map_err(|_|invalid("display name is not strict UTF-8"))?;
    if s=="."||s==".."||s.ends_with('.')||s.ends_with(' ')||s.encode_utf16().count()>COMPONENT_MAX||
        s.chars().any(|c|c.is_control()||matches!(c,'<'|'>'|':'|'"'|'/'|'\\'|'|'|'?'|'*')){
        return Err(invalid("not a single supported business display name"));}
    Ok(s)
}
/// From a persistent FILE field or an explicitly typed client operation.
/// Eq/Ord compare the original UTF-8 bytes; there is no folded/normalized key.
#[derive(Clone,Copy,Debug,Eq,PartialEq,Ord,PartialOrd)]
pub(super) struct FileName<'a>(&'a str);
/// An ordinary directory is itself stored at its exact business spelling.
#[derive(Clone,Copy,Debug,Eq,PartialEq,Ord,PartialOrd)]
pub(super) struct DirectoryName<'a>(&'a str);
impl<'a> FileName<'a>{
    pub(super) fn parse(raw:&'a[u8])->io::Result<Self>{component(raw,FILE_NAME_MAX).map(Self)}
    pub(super) fn display(self)->&'a str{self.0}
    pub(super) fn container(self,file:FileId)->io::Result<ContainerLeaf>{
        if file.0==[0;16]{return Err(invalid("no original nonzero FILE identity"));}
        let at=self.0.rfind('.').filter(|&i|i!=0).unwrap_or(self.0.len());
        let(stem,ext)=self.0.split_at(at);
        let spelling=format!("{stem}-{file}{ext}");
        if spelling.len()!=self.0.len()+1+ID_HEX||spelling.len()>COMPONENT_MAX{
            return Err(invalid("derived FILE container exceeds its physical byte budget"));}
        Ok(ContainerLeaf(CString::new(spelling).map_err(|_|invalid("invalid physical FILE component"))?))
    }
}
impl<'a> DirectoryName<'a>{
    pub(super) fn parse(raw:&'a[u8])->io::Result<Self>{
        let s=component(raw,COMPONENT_MAX)?;
        if TicketLeaf::parse(raw).is_some(){return Err(invalid("this exact native directory spelling is reserved for a ZXS operation"));}
        Ok(Self(s))
    }
    pub(super) fn display(self)->&'a str{self.0}
    pub(super) fn leaf(self)->CString{CString::new(self.0).expect("validated directory name has no NUL")}
}
/// One value is used at either ordinary or dedicated final endpoint; it has
/// no volume, operation, delete counter, date or compatibility suffix.
#[derive(Debug,Clone,PartialEq,Eq)]
pub(super) struct ContainerLeaf(CString);
impl ContainerLeaf{pub(super) fn name(&self)->&CStr{&self.0}}
/// Only forward derivation. Supplied FileId always comes from an original
/// request/FILE identity; no suffix is ever parsed to discover an owner.
pub(super) fn file_leaf(display:&str,file:FileId)->io::Result<CString>{
    Ok(FileName::parse(display.as_bytes())?.container(file)?.0)
}

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
enum Use{Stage(u8),Isolation}
/// Public to the storage implementation, not to a client. Recognition is
/// syntax only: the issuer still needs its real sidecar, parent and object.
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(super) struct TicketLeaf{ticket:OperationId,kind:Use}
impl TicketLeaf{
    pub(super) fn stage(ticket:OperationId,slot:u8)->io::Result<Self>{
        if ticket.0==[0;16]||slot>=TICKETS{return Err(invalid("not a valid private stage slot"));}
        Ok(Self{ticket,kind:Use::Stage(slot)})
    }
    pub(super) fn isolation(ticket:OperationId)->io::Result<Self>{
        if ticket.0==[0;16]{return Err(invalid("not a valid private retirement ticket"));}
        Ok(Self{ticket,kind:Use::Isolation})
    }
    pub(super) fn parse(raw:&[u8])->Option<Self>{
        let id=|b:&[u8]|std::str::from_utf8(b).ok()?.parse::<OperationId>().ok();
        if let Some(t)=raw.strip_prefix(STAGE_PREFIX.as_bytes()){
            if t.len()!=ID_HEX+2||t[ID_HEX]!=b'-'{return None;}
            return Self::stage(id(&t[..ID_HEX])?,t[ID_HEX+1].checked_sub(b'0')?).ok();
        }
        let t=raw.strip_prefix(ISOLATE_PREFIX.as_bytes())?.strip_suffix(b"_")?;
        Self::isolation(id(t)?).ok()
    }
    pub(super) fn as_stage(self)->Option<(OperationId,u8)>{match self.kind{Use::Stage(i)=>Some((self.ticket,i)),Use::Isolation=>None}}
    pub(super) fn name(self)->CString{let s=match self.kind{
        Use::Stage(i)=>format!("{STAGE_PREFIX}{}-{i}",self.ticket),Use::Isolation=>format!("{ISOLATE_PREFIX}{}_",self.ticket)
    };CString::new(s).expect("fixed ASCII private name has no NUL")}
}

mod geometry;
pub(super) use geometry::ResidentNames;

#[cfg(test)]
#[path="../../tests/unit/storage_names.rs"]
mod tests;
