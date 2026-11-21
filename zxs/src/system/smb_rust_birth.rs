//! Real ordinary FILE birth uses the same original installer and kind-5
//! SQL confirmation as Native. Nothing here resembles a Native session.
use super::{Catalog,CreateAt,CreateCut,OrdinaryRoot,Route,Setting,name_on,placed_at};
use crate::{FileId,OperationId,display,identity::AccountId,smb::{FileBirthVote,RustSessionWitness},
    system::reader::smb::{RustStagedWriter,denied,runtime::RustRoot,smb_stage::Scratch,
        work_change::{RustWorkWriter,RustWriterSelection}}};
use std::io;

type Authority=(AccountId,OperationId,i64,Setting);

#[must_use]
pub(crate) enum RustNamedWrite<'a>{
    Existing(RustWriterSelection<'a>),
    Vacant(RustFileBirth<'a>),
}

#[must_use]
pub(crate) struct RustFileBirth<'a>{
    tree:&'a RustRoot,request:&'a RustSessionWitness,
    catalog:Catalog,label:String,route:Route,leaf:String,authority:Authority,
}

impl RustRoot{
    /// Initial CREATE/OPEN_IF classification only, under the actual current
    /// parent and its own write setting. A pending name is not a vacancy.
    pub(crate) fn choose_named_birth_or_writer<'a>(&'a self,request:&'a RustSessionWitness,
        label:&str,parents:&[&str],leaf:&str,open_if:bool,
    )->io::Result<RustNamedWrite<'a>>{
        if !request.is_create(){return Err(denied());}
        display::component(leaf.as_bytes(),222)?;
        let source=self.ordinary_reader(request)?;
        let (catalog,authority,route,existing)=source.at(|db,c|{
            let authority=self.ordinary_birth_at(&CreateCut{source,db,catalog:&c},label)?;
            let volume=authority.3.volume;
            let route=Route::by_names(db,&c,volume,parents)?;
            let named=name_on(db,&c,volume,route.parent(),leaf);
            let existing=match named{
                Ok(Some(file))if open_if=>Some(placed_at(db,&c,volume,route.parent(),leaf,Some(file))?),
                Ok(Some(_))=>return Err(io::ErrorKind::AlreadyExists.into()),
                Ok(None)=>None,
                // name_on returns this for a selected real ordinary
                // directory, which this writable FILE-only lane cannot open.
                Err(e)if open_if&&e.kind()==io::ErrorKind::AlreadyExists=>return Err(io::ErrorKind::IsADirectory.into()),
                Err(e)=>return Err(e),
            };
            Ok((c,authority,route,existing))
        })?;
        self.ordinary_reader(request)?;
        if let Some(placed)=existing{
            return self.choose_selected_writer(request,label,&placed,&route,authority.3).map(RustNamedWrite::Existing);
        }
        Ok(RustNamedWrite::Vacant(RustFileBirth{tree:self,request,catalog,authority,route,
            label:copy_name(label)?,leaf:copy_name(leaf)?}))
    }
}

fn copy_name(s:&str)->io::Result<String>{
    let mut copy=String::new();
    copy.try_reserve_exact(s.len()).map_err(|_|io::ErrorKind::OutOfMemory)?;
    copy.push_str(s);Ok(copy)
}

impl RustFileBirth<'_>{
    /// The actual client/Open coordinator supplies its provisional vote in
    /// the existing birth's offer, before any SQL reservation or filesystem
    /// creation. A successful offer is not a FILE birth or another OPEN.
    pub(crate) fn complete<'v,'r>(self,offer_file:impl FnOnce(FileId)->io::Result<FileBirthVote<'v,'r>>)
        ->io::Result<(RustStagedWriter,FileBirthVote<'v,'r>)>{
        let Self{tree,request,catalog,label,route,leaf,authority}=self;
        let source=tree.ordinary_reader(request)?;
        let media=OrdinaryRoot::at_parent(source,&catalog,&route,None)?;
        if media.sampled_capacity()?.read_only{return Err(io::ErrorKind::ReadOnlyFilesystem.into());}
        let require=|cut:&CreateCut<'_>|{
            let (current,db,c)=cut.parts();
            if !std::ptr::eq(current,source)||tree.ordinary_birth_at(cut,&label)?!=authority||
                Route::at(db,c,authority.3.volume,route.parent())?!=route{return Err(denied());}
            Ok(())
        };
        let (actor,auth,generation,setting)=authority;
        let at=CreateAt{source,setting,parent:route.parent(),actor,auth,generation,require:&require};
        let mut offered=None;
        let live=||tree.original_write_live(request);
        let existed=at.create(&media,&leaf,true,|birth,_|{
            let vote=offer_file(birth.file)?;
            if !vote.is_provisional_for(birth.pool,birth.file){return Err(denied());}
            offered=Some(vote);
            Ok(())
        },|actual|Ok((Scratch::before_birth(actual)?,OperationId::new()?)),&live);
        let (born,(scratch,nonce))=match existed{
            Ok(b)=>b,
            Err(e)if offered.is_none()=>return Err(e),
            Err(e)=>return Err(unconfirmed(e)),
        };
        // After the actual confirmed birth, a failure preparing or sending
        // this OPEN must not be reported as a definite failed creation.
        let writer=RustWorkWriter::from_newborn(born,tree,request,label,route,authority,nonce)
            .map_err(unconfirmed)?;
        let stage=RustStagedWriter::prepared(writer,tree,request,scratch).map_err(unconfirmed)?;
        let vote=offered.ok_or_else(||io::Error::from(io::ErrorKind::ConnectionAborted))?;
        Ok((stage,vote))
    }
}

fn unconfirmed(error:io::Error)->io::Error{
    io::Error::new(io::ErrorKind::ConnectionAborted,error)
}
