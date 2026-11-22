//! The exact native kind-13 directory engine under one real Rust CREATE.
//! No earlier request can be recovered or abandoned by merely making a
//! new request for the same name.
use super::{Catalog,DirectoryAt,DirectoryCreateCut,DirectoryPlace,EarlierBirth,OrdinaryRoot,Place,Route,Setting,
    denied,directory_vacant,exists,lookup};
use crate::{DirectoryId,OperationId,display,identity::AccountId,smb::{DirectoryBirthVote,RustSessionWitness},
    system::reader::smb::{directory::{RustDirectorySelection,RustOrdinaryDirectory},runtime::RustRoot}};
use std::io;

type Authority=(AccountId,OperationId,i64,Setting);
#[must_use]
pub(crate) enum RustNamedDirectory<'a>{
    Existing(RustDirectorySelection<'a>),
    Vacant(RustDirectoryBirth<'a>),
}
#[must_use]
pub(crate) struct RustDirectoryBirth<'a>{
    tree:&'a RustRoot,request:&'a RustSessionWitness,authority:Authority,
    catalog:Catalog,label:String,route:Route,leaf:String,
}
impl RustRoot{
    pub(crate) fn choose_directory_birth_or_existing<'a>(&'a self,request:&'a RustSessionWitness,
        label:&str,parents:&[&str],leaf:&str,open_if:bool,
    )->io::Result<RustNamedDirectory<'a>>{
        if !request.is_create()||parents.len()>=crate::fs::MAX_PATH_DEPTH{return Err(denied());}
        display::component(leaf.as_bytes(),255)?;
        let source=self.ordinary_reader(request)?;
        let (catalog,authority,mut route,existing)=source.at(|db,c|{
            let authority=self.ordinary_directory_birth_at(&DirectoryCreateCut{source,db,catalog:&c},label)?;
            let route=Route::by_names(db,&c,authority.3.volume,parents)?;
            let dir=lookup(db,&c,route.volume,route.parent(),leaf)?;
            let file=if leaf.len()<=222{Place::named(db,&c,route.volume,route.parent(),leaf)?}else{None};
            match (&dir,&file){
                (Some(_),None)if open_if=>{},
                (None,None)=>directory_vacant(db,&c,&route,leaf)?,
                (None,Some(_))if open_if=>return Err(io::ErrorKind::NotADirectory.into()),
                (Some(_),Some(_))=>return Err(crate::fs::invalid("two original types claim this directory name")),
                _=>return Err(exists()),
            }
            Ok((c,authority,route,dir))
        })?;
        self.ordinary_reader(request)?;
        let label=copy_name(label)?;
        if let Some(dir)=existing{
            route.steps.try_reserve(1).map_err(|_|io::ErrorKind::OutOfMemory)?;
            route.steps.push(dir);
            route.geometry(&catalog)?;
            return Ok(RustNamedDirectory::Existing(RustDirectorySelection::selected(
                self,request,label,authority.3,catalog,route,
            )));
        }
        route.steps.try_reserve(1).map_err(|_|io::ErrorKind::OutOfMemory)?;
        Ok(RustNamedDirectory::Vacant(RustDirectoryBirth{tree:self,request,authority,catalog,label,route,leaf:copy_name(leaf)?}))
    }
}
fn copy_name(name:&str)->io::Result<String>{
    let mut out=String::new();
    out.try_reserve_exact(name.len()).map_err(|_|io::ErrorKind::OutOfMemory)?;
    out.push_str(name);Ok(out)
}
impl RustDirectoryBirth<'_>{
    pub(crate) fn complete<'v,'r>(self,offer_directory:impl FnOnce(DirectoryId)->io::Result<DirectoryBirthVote<'v,'r>>)
        ->io::Result<(RustOrdinaryDirectory,DirectoryBirthVote<'v,'r>)>{
        let Self{tree,request,authority,catalog,label,mut route,leaf}=self;
        let source=tree.ordinary_reader(request)?;
        let media=OrdinaryRoot::at_parent(source,&catalog,&route,None)?;
        if media.sampled_capacity()?.read_only{return Err(io::ErrorKind::ReadOnlyFilesystem.into());}
        let require=|cut:&DirectoryCreateCut<'_>|{
            let (src,db,c)=cut.parts();
            if !std::ptr::eq(src,source)||tree.ordinary_directory_birth_at(cut,&label)?!=authority||
               Route::at(db,c,route.volume,route.parent())?!=route{return Err(denied());}
            Ok(())
        };
        let (actor,auth,epoch,setting)=authority;
        let at=DirectoryAt{source,setting,actor,auth,epoch,require:&require,earlier:EarlierBirth::Fresh};
        let mut offered=None;
        let op=OperationId::new()?;
        let attempt=at.make(&route,op,&leaf,|birth|{
            let vote=offer_directory(birth.id)?;
            if !vote.is_provisional_for(birth.pool,birth.id){return Err(denied());}
            offered=Some(vote);Ok(())
        },&||tree.original_write_live(request),|_|Ok(()));
        let (birth,held)=match attempt{
            Ok(genuine)=>genuine,
            Err(e)if offered.is_none()=>return Err(e),
            Err(e)=>return Err(uncertain(e)),
        };
        let media=media.adopt_directory(&birth,held).map_err(uncertain)?;
        // Keep the same marker and already held FD. `from_birth` independently
        // compares this exact position (not just DirectoryId) against the live Route.
        route.steps.push(DirectoryPlace::born(birth));
        route.geometry(&catalog).map_err(uncertain)?;
        let actual=RustOrdinaryDirectory::from_birth(tree,request,label,setting,route,media).map_err(uncertain)?;
        let vote=offered.ok_or_else(||io::Error::from(io::ErrorKind::ConnectionAborted))?;
        Ok((actual,vote))
    }
}
fn uncertain(cause:io::Error)->io::Error{io::Error::new(io::ErrorKind::ConnectionAborted,cause)}
