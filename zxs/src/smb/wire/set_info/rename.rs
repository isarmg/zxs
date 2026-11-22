//! The basic FILE SET_INFO rename class and its original request. Parsing
//! a target in a share proves neither a source OPEN nor any right to move.
use super::{Error,INFO_FILE,Message,NO_CURRENT_TARGET,OpenReference,SET_INFO,VerifiedMember,fixed,u32le};
use crate::smb::{
    backend::path::{NameError,RenameSpelling,TreeName},
    wire::{io_fields::IoFieldError,message::Routing,path_fields::FieldError},
};

const BASIC_RENAME:u8=10;

#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(in crate::smb) enum RenameError{Fields(Error),Name(NameError)}
impl From<Error> for RenameError{
    fn from(value:Error)->Self{Self::Fields(value)}
}

struct Fields{open:OpenReference,destination:TreeName,requested_replace:bool,had_root_separator:bool}

/// Only a fresh paid, verified member can be moved into this request.
/// Keep the original exact message for its later authorization and reply.
#[must_use]
pub(in crate::smb) struct RenameRequest<'step,'raw>{
    original:VerifiedMember<'step,'raw>,fields:Fields,
}
impl<'step,'raw> RenameRequest<'step,'raw>{
    pub(in crate::smb) fn parse(original:VerifiedMember<'step,'raw>)
        ->Result<Self,(VerifiedMember<'step,'raw>,RenameError)>{
        if let Err(e)=original.session().recheck(){return Err((original,Error::Session(e).into()));}
        match decode(original.message(),original.max_transact()){
            Ok(fields)=>Ok(Self{original,fields}),
            Err(e)=>Err((original,e)),
        }
    }
    pub(in crate::smb) fn original(&self)->&VerifiedMember<'step,'raw>{&self.original}
    pub(in crate::smb) fn open(&self)->OpenReference{self.fields.open}
    pub(in crate::smb) fn destination(&self)->&TreeName{&self.fields.destination}
    pub(in crate::smb) fn had_root_separator(&self)->bool{self.fields.had_root_separator}
    /// A client preference, never authority to replace an object. The
    /// standalone two-parent FILE profile requires this to be false.
    pub(in crate::smb) fn requested_replace(&self)->bool{self.fields.requested_replace}
}
impl Message<'_>{
    /// Dispatch hint before any variable buffer or permission is consumed.
    pub(in crate::smb) fn is_basic_rename_request(&self)->bool{
        self.header.command==SET_INFO&&self.raw.get(66..68)==Some(&[INFO_FILE,BASIC_RENAME][..])
    }
}
fn decode(m:&Message<'_>,max_transact:u32)->Result<Fields,RenameError>{
    fixed(m)?;
    if !m.is_basic_rename_request()||m.header.flags&NO_CURRENT_TARGET!=0||!matches!(m.header.routing,Routing::Sync{..}){
        return Err(Error::NotInProfile.into());
    }
    let n=u32le(m.raw,68).map_err(|_|Error::Malformed)?;
    if n>max_transact{return Err(Error::ExceedsMaxTransact.into());}
    if n==0{return Err(Error::Malformed.into());}
    let open=m.set_info_target().map_err(|e|match e{
        IoFieldError::NoExplicitOpen=>Error::NoExplicitOpen,_=>Error::Malformed,
    })?;
    let p=RenameSpelling::from_message(m).map_err(|e|match e{
        NameError::Field(FieldError::WrongCommand)=>Error::WrongCommand.into(),
        NameError::Field(FieldError::Unsupported)=>Error::NotInProfile.into(),
        NameError::Field(FieldError::Malformed)=>Error::Malformed.into(),
        NameError::Field(FieldError::RenameInformationLength)=>Error::InformationLength.into(),
        name=>RenameError::Name(name),
    })?;
    Ok(Fields{open,destination:p.destination,requested_replace:p.requested_replace,had_root_separator:p.had_root_separator})
}

#[cfg(test)]
#[path="../../../../tests/smb/rename_request.rs"]
mod tests;
