//! Only the already-held ordinary FILE can enter the shared kind14 engine.
//! Rust borrows the same write cuts and physical locks for both parents.
use super::{Cut,RenameAt,RenameCut,Renamed,denied};
use super::super::{first_work::HeldRename,ordinary_directory::Route,runtime::RustRoot};
use crate::{OperationId,smb::RustSessionWitness,system::reader::volume::OrdinaryMedia};
use std::io;

pub(in crate::system::reader::smb) fn require_writable(media:&OrdinaryMedia)->io::Result<()>{
    if media.capacity_origin().sample()?.read_only{return Err(io::ErrorKind::ReadOnlyFilesystem.into());}
    Ok(())
}

pub(in crate::system::reader::smb) enum HeldRenameResult{
    SameParent(Renamed),
    OtherParent{renamed:Renamed,route:Route,parent:OrdinaryMedia},
}
pub(in crate::system::reader::smb) fn rename_held(
    tree:&RustRoot,member:&RustSessionWitness,held:HeldRename<'_>,parents:Option<&[&str]>,name:&str,
)->io::Result<HeldRenameResult>{
    if !member.is_set_info(){return Err(denied());}
    let (place,opened,label,route,setting,media)=held.parts();
    let source=tree.ordinary_reader(member)?;
    let select=|cut:&RenameCut<'_>|{
        let prior=tree.ordinary_rename_at(cut,label)?;
        let (src,db,c)=cut.parts();
        if !std::ptr::eq(src,source)||prior.3!=setting||Route::at(db,c,setting.volume,route.parent())?!=*route{
            return Err(denied());
        }
        media.capacity_origin().require_catalog(c,setting.volume)?;
        Ok(prior)
    };
    let (actor,auth,epoch,_)=source.at(|db,c|select(&RenameCut{source,db,catalog:&c}))?;
    require_writable(media)?;
    let op=OperationId::new()?;
    let require=|cut:&RenameCut<'_>|{
        if select(cut)?!=(actor,auth,epoch,setting){return Err(denied());}
        Ok(())
    };
    // Only the original cheap in-transaction liveness check. Physical
    // samples remain outside SQLite; no peer/session lock spans disk I/O.
    let live=||tree.original_write_live(member);
    let at=RenameAt{source,setting,parent:route.parent(),actor,auth,epoch,require:&require};
    let checked=|point|{
        if point==Cut::Reserved{require_writable(media)?;}
        Ok(())
    };
    let actual=media.view(source);
    match parents{
        None=>at.rename(actual,place,opened,op,name,&live,checked).map(HeldRenameResult::SameParent),
        Some(parts)=>{
            let moved=at.move_between(actual,place,opened,parts,op,name,&live,checked)?;
            Ok(HeldRenameResult::OtherParent{renamed:moved.renamed,route:moved.route,parent:moved.target.into_media()})
        },
    }
}
