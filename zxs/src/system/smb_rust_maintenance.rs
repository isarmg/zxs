//! Finish only the original durable business obligation in the existing
//! Core loops. Having the inherited plan is not a user session or FILE.
use super::{Catalog,Connection,Mode,QualifiedRustInstall,Role,denied};
use crate::{VolumeId,identity::ShareId,system::reader::{file_location::Place,smb::{file_rename,ordinary_directory,retirement}}};
use std::io;

impl QualifiedRustInstall{
    /// The actual original source and parent remain held for the worker's
    /// life. No nested SQLite read or blocking provider lock in live().
    fn maintenance_live(&self,stopping:&impl Fn()->bool)->io::Result<()>{
        if stopping()||self.source.dir.root.entity!=self.origin.system_entity(){return Err(denied());}
        self.origin.require_live_source()
    }
    fn on_original_volumes(&self,c:&Catalog,place:&Place)->bool{
        match place.role{
            Role::File=>self.installed.plan.folders().iter().any(|f|f.volume()==place.volume),
            Role::Recycle=>c.rows().get(1).is_some_and(|p|
                p.ordinal==1&&p.role==Role::Recycle&&p.volume==place.volume),
            _=>false,
        }
    }
    /// An already-confirmed transfer records only its own retired source.
    /// The shared worker still compares both actual objects before cleanup.
    /// Current RW is a gate on new user work, not on removal of a proved,
    /// no-longer-selected old incarnation after an authorized transfer.
    pub(crate) fn finish_confirmed_sources(&self,stopping:impl Fn()->bool)->io::Result<()>{
        self.require_current()?;
        retirement::serve_scoped(&self.source,|db,c,from,now|{
            if self.installed.active_at(db,c)?!=self.original||
                !self.on_original_volumes(c,from)||!self.on_original_volumes(c,now){return Err(denied());}
            Ok(())
        },||self.maintenance_live(&stopping));
        Ok(())
    }
    /// The cold engine can finish only what the original FILE actually
    /// installed, abandon a proved untouched reservation, or roll back the
    /// same held object. It never starts a new rename, user, path or FileId.
    pub(crate) fn settle_interrupted_file_renames(&self,stopping:impl Fn()->bool)->io::Result<()>{
        self.require_current()?;
        file_rename::recovery_loop(&self.source,|db,c,share,volume|self.rename_scope(db,c,share,volume),
            ||self.maintenance_live(&stopping));
        Ok(())
    }
    /// Directory kind16 uses its original Core cold state machine. This
    /// lane cannot perform the first forward move or borrow Native session
    /// authority; each typed pending cut re-proves this installed RW scope.
    pub(crate) fn settle_interrupted_directory_relabels(&self,stopping:impl Fn()->bool)->io::Result<()>{
        self.require_current()?;
        ordinary_directory::recover_relabels(&self.source,|db,c,share,volume|self.rename_scope(db,c,share,volume),
            ||self.maintenance_live(&stopping));
        Ok(())
    }
    fn rename_scope(&self,db:&Connection,c:&Catalog,share:ShareId,volume:VolumeId)->io::Result<()>{
        if self.installed.active_at(db,c)?!=self.original{return Err(denied());}
        let plan=&self.installed.plan;
        let folder=plan.folders().iter().find(|f|f.share()==share&&f.volume()==volume).ok_or_else(denied)?;
        if !plan.current(db,c,folder.name())?.is_some_and(|g|
            g.share==share&&g.volume==volume&&g.mode==Mode::ReadWrite){return Err(denied());}
        Ok(())
    }
}
