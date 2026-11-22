//! One actual Rust SMB writer for an existing ordinary FILE. CREATE takes
//! the original exclusive FILE hold; all subsequent SAVE operations use
//! the same native business engine and its single original working round.
use super::{Catalog,EditAuthority,Place,Placed,Receipt,State,WriteAt,WriteFile,WriterCut,UnconfirmedSave,busy,file_create};
use crate::{FileId,OperationId,display,fs::{HeldDir,invalid},identity::AccountId,
    smb::RustSessionWitness,
    ordinary::working_round::{Draft,Span},
    system::reader::{account::share_mode::{Mode,Setting},smb::{denied,file_number::SmbFileIndex,ordinary_directory::Route,runtime::{RustRoot,RustRootStamp}},
        volume::{OrdinaryMedia,OrdinaryRoot}}};
use std::{fs::File,io};

/// A prospective ordinary FILE in one real paid CREATE, not an OPEN. The
/// protocol side reserves its existing FILE-scoped share vote first.
#[must_use]
pub(crate) struct RustWriterSelection<'a>{
    tree:&'a RustRoot,member:&'a RustSessionWitness,label:String,
    catalog:Catalog,route:Route,placed:Placed,selected:State,authority:EditAuthority,
}
impl RustRoot{
    pub(crate) fn choose_existing_writer<'a>(&'a self,member:&'a RustSessionWitness,
        label:&str,parents:&[&str],leaf:&str)->io::Result<RustWriterSelection<'a>>{
        if !member.is_create(){return Err(denied());}
        display::component(leaf.as_bytes(),222)?;
        self.select_writer(member,label,|db,c,volume|{
            let route=Route::by_names(db,c,volume,parents)?;
            let placed=file_create::placed_at(db,c,volume,route.parent(),leaf,None)?;
            Ok((route,placed))
        })
    }
    // MAXIMUM_ALLOWED cannot turn a checked filename into a second name
    // lookup against another identity or regain an older RW generation.
    pub(in crate::system::reader::smb) fn choose_selected_writer<'a>(&'a self,
        member:&'a RustSessionWitness,label:&str,expected:&Placed,parent:&Route,setting:Setting,
    )->io::Result<RustWriterSelection<'a>>{
        self.select_writer(member,label,|db,c,volume|{
            if volume!=setting.volume{return Err(denied());}
            let route=Route::at(db,c,volume,parent.parent())?;
            if &route!=parent{return Err(denied());}
            let placed=file_create::placed_at(db,c,volume,parent.parent(),&expected.here.name,Some(expected.here.file))?;
            if placed.here!=expected.here||placed.birth!=expected.birth{return Err(denied());}
            Ok((route,placed))
        }).and_then(|r|if r.authority.setting==setting{Ok(r)}else{Err(denied())})
    }
    fn select_writer<'a>(&'a self,member:&'a RustSessionWitness,label:&str,
        select:impl FnOnce(&super::Connection,&Catalog,crate::VolumeId)->io::Result<(Route,Placed)>,
    )->io::Result<RustWriterSelection<'a>>{
        if !member.is_create(){return Err(denied());}
        let source=self.ordinary_reader(member)?;
        let (catalog,route,placed,selected,authority)=source.at(|db,c|{
            let author=self.ordinary_writer_at(&WriterCut{source,db,catalog:&c},label)?;
            let authority=EditAuthority{account:author.0,op:author.1,generation:author.2,setting:author.3};
            let (route,placed)=select(db,&c,authority.setting.volume)?;
            let selected=State::at(db,&c,&placed.birth,&placed.here)?;
            Ok((c,route,placed,selected,authority))
        })?;
        self.ordinary_reader(member)?;
        let mut name=String::new();
        name.try_reserve_exact(label.len()).map_err(|_|io::ErrorKind::OutOfMemory)?;
        name.push_str(label);
        Ok(RustWriterSelection{tree:self,member,label:name,catalog,route,placed,selected,authority})
    }
}
impl RustWriterSelection<'_>{
    pub(crate) fn file(&self)->FileId{self.placed.birth.file}
    pub(crate) fn acquire(self)->io::Result<RustWorkWriter>{
        let Self{tree,member,label,catalog,route,placed,selected,authority}=self;
        let source=tree.ordinary_reader(member)?;
        let ordinary=OrdinaryRoot::at_parent(source,&catalog,&route,None).map_err(|e|match e.kind(){
            io::ErrorKind::NotFound|io::ErrorKind::NotADirectory=>io::Error::new(io::ErrorKind::NotConnected,
                "original writable ordinary FILE parent is offline"),
            _=>e,
        })?;
        // The SQL RW policy doesn't turn a truly read-only mount into
        // writable media. A possible RAM staging fallback cannot do that.
        if ordinary.sampled_capacity()?.read_only{
            return Err(io::ErrorKind::ReadOnlyFilesystem.into());
        }
        let original=WriteFile::acquire(placed,ordinary.folder(),&selected)?;
        let held=RustWorkWriter{original,media:ordinary.into_media(),tree:tree.stamp(),authority,label,route};
        held.with_write(tree,member,|writer|{
            writer.physical(&selected)?;
            source.at(|db,c|{
                if writer.state(db,&c)?!=selected{return Err(busy("work changed during writer admission"));}
                Ok(())
            })
        })?;
        Ok(held)
    }
}

/// Owns the genuine Native-equivalent FILE EX and original writer nonce,
/// then its ordinary physical parent. The tree owns the original reader;
/// this hold contains no self-reference, protocol route or alternate DB.
#[must_use]
pub(crate) struct RustWorkWriter{
    original:WriteFile,media:OrdinaryMedia,tree:RustRootStamp,
    authority:EditAuthority,label:String,route:Route,
}
impl RustWorkWriter{
    pub(in crate::system::reader::smb) fn from_newborn(
        born:super::super::file_create::NativeBorn<'_>,tree:&RustRoot,member:&RustSessionWitness,
        label:String,route:Route,who:(AccountId,OperationId,i64,Setting),nonce:OperationId,
    )->io::Result<Self>{
        if !member.is_create(){return Err(denied());}
        let (account,op,generation,setting)=who;
        let (root,birth,installed)=born.parts(true)?;
        if !root.for_tree(tree.ordinary_reader(member)?,setting.volume)||
           birth.volume!=setting.volume||root.parent()!=route.parent(){return Err(denied());}
        let place=Place::origin(&birth,root.parent());
        // Same FILE EX that the real kind-5 birth already owns. Do not
        // reopen by name, release/reacquire, or start a second write owner.
        let writer=Self{original:WriteFile{installed,birth,place,nonce},media:root.into_media(),tree:tree.stamp(),
            authority:EditAuthority{account,op,generation,setting},label,route};
        writer.require_current(tree,member)?;
        Ok(writer)
    }
    pub(crate) fn file(&self)->FileId{self.original.birth.file}
    pub(crate) fn belongs_to(&self,tree:&RustRoot)->bool{tree.matches_stamp(&self.tree)}
    pub(in crate::system::reader::smb) fn actual_staging_parent(&self)->&HeldDir{self.original.installed.directory()}
    pub(in crate::system::reader::smb) fn original_place(&self)->&Place{&self.original.place}
    pub(in crate::system::reader::smb) fn label(&self)->&str{&self.label}
    pub(in crate::system::reader::smb) fn route(&self)->&Route{&self.route}
    pub(in crate::system::reader::smb) fn setting(&self)->Setting{self.authority.setting}
    pub(in crate::system::reader::smb) fn work_space(&self,tree:&RustRoot,member:&RustSessionWitness)->io::Result<u64>{
        self.with_write(tree,member,|w|{
            w.require_work()?;
            let s=self.media.capacity_origin().sample()?;
            if s.read_only{return Err(io::ErrorKind::ReadOnlyFilesystem.into());}
            Ok(s.available)
        })
    }
    fn with_write<T>(&self,tree:&RustRoot,member:&RustSessionWitness,
        action:impl FnOnce(WriteAt<'_>)->io::Result<T>)->io::Result<T>{
        let answer=self.with_writer_at(tree,member,action)?;
        tree.ordinary_reader(member)?;
        Ok(answer)
    }
    fn with_writer_at<T>(&self,tree:&RustRoot,member:&RustSessionWitness,
        action:impl FnOnce(WriteAt<'_>)->io::Result<T>)->io::Result<T>{
        if !(member.is_create()||member.is_read()||member.is_write()||member.is_flush()||
             member.is_close()||member.is_set_info()||member.is_query_info()){return Err(denied());}
        self.inspect_writer_at(tree,member,action)
    }
    fn inspect_writer_at<T>(&self,tree:&RustRoot,member:&RustSessionWitness,
        inspect:impl FnOnce(WriteAt<'_>)->io::Result<T>)->io::Result<T>{
        if !self.belongs_to(tree){return Err(denied());}
        let source=tree.ordinary_reader(member)?;
        let require=|cut:&WriterCut<'_>|{
            let who=tree.ordinary_writer_at(cut,&self.label)?;
            let current=EditAuthority{account:who.0,op:who.1,generation:who.2,setting:who.3};
            if current!=self.authority{return Err(denied());}
            let (db,c)= (cut.db,cut.catalog);
            if Route::at(db,c,self.authority.setting.volume,self.route.parent())?!=self.route{
                return Err(invalid("original writable ordinary route changed"));
            }
            self.media.capacity_origin().require_catalog(c,current.setting.volume)
        };
        inspect(WriteAt{original:&self.original,root:self.media.view(source),source,
            authority:self.authority,parent:self.route.parent(),require:&require})
    }
    pub(crate) fn require_current(&self,tree:&RustRoot,member:&RustSessionWitness)->io::Result<()>{
        let require=|w:WriteAt<'_>|{
            w.require_work()?;
            if self.media.capacity_origin().sample()?.read_only{return Err(io::ErrorKind::ReadOnlyFilesystem.into());}
            Ok(())
        };
        if member.is_lock(){
            // A paid LOCK can observe the same actual writer; it is not
            // admitted to the staging, write or save command paths.
            self.inspect_writer_at(tree,member,require)?;
            tree.ordinary_reader(member).map(drop)
        }else{
            self.with_write(tree,member,require)
        }
    }
    pub(in crate::system::reader::smb) fn smb_index(&self,tree:&RustRoot,member:&RustSessionWitness)->io::Result<SmbFileIndex>{
        if !(member.is_create()||member.is_query_info()){return Err(denied());}
        self.with_write(tree,member,|w|{
            let found=w.smb_index()?;
            if self.media.capacity_origin().sample()?.read_only{return Err(io::ErrorKind::ReadOnlyFilesystem.into());}
            Ok(found)
        })
    }
    pub(in crate::system::reader::smb) fn save_packed(&self,tree:&RustRoot,member:&RustSessionWitness,
        op:OperationId,predecessor:Option<OperationId>,retained:u64,eof:u64,spans:&[Span],packed:&File)->io::Result<Receipt>{
        if !(member.is_write()||member.is_flush()||member.is_close()){return Err(denied());}
        // Only the live original session/source is sampled inside the
        // write transaction; do not perform a nested SQL read there.
        let alive=||tree.original_write_live(member);
        // Once the shared engine proved a durable SAVE, subsequent loss
        // of permission/media is a response fence, not a storage failure
        // that permits another SAVE. Preserve the original outcome.
        self.with_writer_at(tree,member,|w|w.save_staged(op,predecessor,
            |key,start,old|Draft::packed(key,start,old,retained,eof,spans,packed),&alive,|_|Ok(())))
            .map_err(|e|if e.get_ref().is_some_and(|cause|cause.is::<UnconfirmedSave>()){
                io::Error::new(io::ErrorKind::ConnectionAborted,e)
            }else{e})
    }
}
