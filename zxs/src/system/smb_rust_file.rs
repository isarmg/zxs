//! Same ordinary stored FILE and selected current work as the native reader.
//! No alternative login, pathname join, revision or writer is defined here.
use super::{Catalog,Connection,MovedFile,ReadAt,ReadCut,ReadFile,Placed,file_create,work_record};
use crate::{FileId,OperationId,display,fs::invalid,ordinary::working_round::Used,smb::{FileRenameTarget,OrdinaryRenameAdmission,RustSessionWitness},
    system::reader::{account::share_mode::{Mode,Setting},smb::{ConfirmedOrdinaryMove,MoveTargetKey,denied,file_number::{self,SmbFileIndex},file_rename,ordinary_directory::Route,
        runtime::{RustRoot,RustRootStamp},smb_buffer,work_change::{RustWorkWriter,RustWriterSelection}},volume::{OrdinaryMedia,OrdinaryRoot}}};
use std::{io::{self,Write},rc::{Rc,Weak}};

/// Only the exact current paid CREATE's original logical selection. The
/// server must reserve its FILE-scoped share vote before consuming this
/// with the actual read hold, and before any on-wire OPEN succeeds.
#[must_use]
pub(crate) struct RustFileSelection<'a>{
    tree:&'a RustRoot,member:&'a RustSessionWitness,label:String,grant:Setting,
    catalog:Catalog,route:Route,placed:Placed,before:ReadCut,
}
impl RustRoot{
    pub(crate) fn choose_read_file<'a>(&'a self,member:&'a RustSessionWitness,label:&str,parents:&[&str],leaf:&str)
        ->io::Result<RustFileSelection<'a>>{
        if !member.is_create(){return Err(denied());}
        let source=self.ordinary_reader(member)?;
        display::component(leaf.as_bytes(),222)?;
        let (catalog,grant,route,placed,before)=source.at(|db,c|{
            let grant=self.ordinary_folder_at(db,&c,label)?;
            let route=Route::by_names(db,&c,grant.volume,parents)?;
            let placed=file_create::placed_at(db,&c,grant.volume,route.parent(),leaf,None)?;
            let selected=work_record::selected(db,&placed.birth)?;
            let before=ReadCut::gather(db,&c,&placed.birth,&placed.here,selected)?;
            Ok((c,grant,route,placed,before))
        })?;
        self.ordinary_reader(member)?;
        let mut name=String::new();
        name.try_reserve_exact(label.len()).map_err(|_|io::ErrorKind::OutOfMemory)?;
        name.push_str(label);
        Ok(RustFileSelection{tree:self,member,label:name,grant,catalog,route,placed,before})
    }
}
impl<'a> RustFileSelection<'a>{
    pub(crate) fn parent_allows_file_write(&self)->bool{self.grant.mode==Mode::ReadWrite}
    pub(crate) fn select_writer(self)->io::Result<RustWriterSelection<'a>>{
        self.tree.choose_selected_writer(self.member,&self.label,&self.placed,&self.route,self.grant)
    }
    pub(crate) fn file(&self)->FileId{self.placed.birth.file}
    pub(crate) fn acquire(self)->io::Result<RustReadFile>{
        let Self{tree,member,label,grant,catalog,route,placed,before}=self;
        let source=tree.ordinary_reader(member)?;
        // This is the specific registered F and selected ordinary ancestry
        // from the original cut, not a path inferred by parsing the name.
        let parent=OrdinaryRoot::at_parent(source,&catalog,&route,None)
            .map_err(|e|match e.kind(){
                io::ErrorKind::NotFound|io::ErrorKind::NotADirectory=>io::Error::new(io::ErrorKind::NotConnected,
                    "original ordinary FILE parent is offline"),
                _=>e,
            })?.into_media();
        let file=ReadFile::open(placed,&before,parent.folder())?;
        let result=RustReadFile{file,parent,tree:tree.stamp(),label,grant,route,mark:Rc::new(())};
        result.with_read(tree,member,|reader|reader.checked(&before,||Ok(()),|_|Ok(())))?;
        Ok(result)
    }
}

#[must_use]
pub(crate) struct RustReadFile{
    // Physical body / FILE before original parent, as in Current.
    file:ReadFile,parent:OrdinaryMedia,tree:RustRootStamp,label:String,grant:Setting,route:Route,mark:Rc<()>,
}
/// No independent authorization, application result or new OPEN. Its actual
/// per-view witnesses cannot be applied to an equal FILE on another OPEN.
#[must_use]
pub(crate) struct RustReadProof{mark:Weak<()>,used:Used}

/// Can exist only after replacing the physical custody inside this already
/// open reader, not before actual target validation/adoption. Not Send and
/// no permission to sign or answer a request.
#[must_use]
pub(crate) struct RustAdoptedMove{
    mark:Weak<()>,target:MoveTargetKey,
}
impl RustAdoptedMove{
    pub(crate) fn target(&self)->&MoveTargetKey{&self.target}
}
/// Short exact source built only from the already-held FILE, not a name
/// lookup or a second authorization. No receiving module can fill it.
pub(in crate::system::reader::smb) struct HeldRename<'a>{
    file:&'a ReadFile,label:&'a str,route:&'a Route,grant:Setting,media:&'a OrdinaryMedia,
}
impl<'a> HeldRename<'a>{
    pub(in crate::system::reader::smb) fn parts(&self)->(&'a super::Place,&'a crate::ordinary::portable_birth::Installed,
        &'a str,&'a Route,Setting,&'a OrdinaryMedia){
        (&self.file.place,&self.file.container,self.label,self.route,self.grant,self.media)
    }
}
#[must_use]
pub(crate) struct RustOrdinaryRename{mark:Weak<()>,original:file_rename::ExactRename}
/// A completed ordinary rename after the originating READ-like OPEN was
/// actually closed. Retain only the exact F/ancestry and the durable fact;
/// no FILE directory, read material, cursor, writer or data access remains.
#[must_use]
pub(crate) struct RustRetiredOrdinaryRename{
    media:OrdinaryMedia,tree:RustRootStamp,label:String,grant:Setting,route:Route,
    birth:crate::ordinary::portable_birth::Birth,place:super::Place,original:file_rename::ExactRename,
}
impl RustRetiredOrdinaryRename{
    pub(crate) fn require_current(&self,tree:&RustRoot,member:&RustSessionWitness)->io::Result<()>{
        if !member.is_set_info()||self.grant.mode!=Mode::ReadWrite||!tree.matches_stamp(&self.tree){
            return Err(denied());
        }
        let source=tree.ordinary_reader(member)?;
        let require=|db:&Connection,c:&Catalog|{
            if tree.ordinary_folder_at(db,c,&self.label)?!=self.grant||
                Route::at(db,c,self.grant.volume,self.route.parent())?!=self.route||
                super::Place::current(db,c,self.place.file)?!=self.place{return Err(denied());}
            self.media.capacity_origin().require_catalog(c,self.grant.volume)?;
            self.original.require(db,&self.birth,&self.place)
        };
        source.at(|db,c|require(db,&c))?;
        self.media.view(source).require_streaming()?;
        file_rename::require_writable(&self.media)?;
        source.at(|db,c|require(db,&c))?;
        tree.ordinary_reader(member).map(drop)
    }
}
/// Fixed facts of this original, single-link ordinary FILE and held
/// confirmed view. Not a stat of the container, its material links or F.
#[must_use]
pub(crate) struct RustFileStandard{mark:Weak<()>,eof:u64}
impl RustFileStandard{
    pub(crate) fn eof(&self)->u64{self.eof}
}
/// The stored name of this exact ordinary FILE placement on this OPEN.
/// No submitted name, newly resolved route or host path is a source.
#[must_use]
pub(crate) struct RustFileName{
    // A move can rebind the same live OPEN. Its old spelling is not a
    // proof about that OPEN's new placement, even if the name repeats.
    mark:Weak<()>,placement:(OperationId,i64),path:String,
}
impl RustFileName{
    pub(crate) fn path(&self)->&str{&self.path}
    /// Shared spelling codec only; creates no FILE/placement proof.
    pub(in crate::system::reader::smb) fn format<'a>(parts:impl Iterator<Item=&'a str>)->io::Result<String>{
        normalized_share_path(parts)
    }
}

fn normalized_share_path<'a>(parts:impl Iterator<Item=&'a str>)->io::Result<String>{
    // MS-FSCC §2.1.5: include the leading separator in this upper
    // bound. A UTF-8 byte count would be wrong for display names.
    const MAX_PATH_UNITS:usize=32_760;
    let mut path=String::new();
    let mut units=0usize;
    for part in parts{
        display::component(part.as_bytes(),display::MAX)?;
        units+=1+part.encode_utf16().count();
        if units>MAX_PATH_UNITS{return Err(io::ErrorKind::Unsupported.into());}
        path.try_reserve(1+part.len()).map_err(|_|io::ErrorKind::OutOfMemory)?;
        path.push('\\');path.push_str(part);
    }
    if path.is_empty(){return Err(invalid("ordinary FILE has no logical pathname"));}
    Ok(path)
}

impl RustReadFile {
    pub(crate) fn file(&self)->FileId{self.file.birth.file}
    pub(crate) fn eof(&self)->u64{self.file.size()}
    pub(crate) fn parent_allows_file_write(&self)->bool{self.grant.mode==Mode::ReadWrite}
    pub(in crate::system::reader::smb) fn deletion_coordinates(&self)->(&super::Place,&str,&Route,Setting){
        (&self.file.place,&self.label,&self.route,self.grant)
    }
    pub(crate) fn belongs_to(&self,tree:&RustRoot)->bool{tree.matches_stamp(&self.tree)}

    fn with_read<T>(&self,tree:&RustRoot,member:&RustSessionWitness,action:impl FnOnce(ReadAt<'_>)->io::Result<T>)
        ->io::Result<T>{
        if !self.belongs_to(tree){return Err(denied());}
        let source=tree.ordinary_reader(member)?;
        let require=|db:&Connection,c:&Catalog|{
            // Same immutable approved folder and exact current setting
            // revision. RW->RO->RW never restores an old OPEN.
            if tree.ordinary_folder_at(db,c,&self.label)?!=self.grant{return Err(denied());}
            let found=Route::at(db,c,self.grant.volume,self.route.parent())?;
            if found!=self.route{return Err(invalid("original ordinary FILE ancestry changed"));}
            self.parent.capacity_origin().require_catalog(c,self.grant.volume)
        };
        let result=action(ReadAt{file:&self.file,root:self.parent.view(source),source,
            volume:self.grant.volume,parent:self.route.parent(),require:&require})?;
        tree.ordinary_reader(member)?;
        Ok(result)
    }
    pub(crate) fn require_current(&self,tree:&RustRoot,member:&RustSessionWitness)->io::Result<()>{
        self.with_read(tree,member,|r|r.check_with(||Ok(()),|_|Ok(())))
    }
    pub(crate) fn smb_index(&self,tree:&RustRoot,member:&RustSessionWitness)->io::Result<SmbFileIndex>{
        if !(member.is_create()||member.is_query_info()){return Err(denied());}
        self.with_read(tree,member,|r|r.check_with(||Ok(()),|db|
            file_number::smb_index(db,r.file.birth.pool,r.file.birth.file)))
    }

    /// Ordinary FILE access policy, not a writer, permission upgrade or
    /// capacity answer. The old reader/ancestry must be current on both
    /// sides of sampling the already-held physical F.
    pub(crate) fn allows_writable_policy(&self,tree:&RustRoot,member:&RustSessionWitness)->io::Result<bool>{
        if !member.is_create(){return Err(denied());}
        self.require_current(tree,member)?;
        if self.grant.mode!=Mode::ReadWrite{return Ok(false);}
        let writable=!self.parent.capacity_origin().sample()?.read_only;
        self.require_current(tree,member)?;
        Ok(writable)
    }
    /// Compare a full client spelling against only our held volume
    /// label and logical parent. The subsequent rename re-qualifies this
    /// original route in the same physical and SQL cuts before mutation.
    pub(crate) fn original_parent_components(&self)->impl Iterator<Item=&str>{
        std::iter::once(self.label.as_str()).chain(self.route.components())
    }
    pub(crate) fn original_folder_label(&self)->&str{&self.label}
    /// The bound original only, not a physical refresh or new path lookup.
    /// The common move engine must still test the actual source under FILE EX.
    pub(in crate::system::reader::smb) fn ordinary_move_source(&self,tree:&RustRoot,member:&RustSessionWitness)
        ->io::Result<(&str,&Route,&crate::ordinary::portable_birth::Birth,super::super::file_transfer::OrdinaryMoveSource)>{
        if !member.is_set_info()||!self.belongs_to(tree)||self.grant.mode!=Mode::ReadWrite{return Err(denied());}
        Ok((&self.label,&self.route,&self.file.birth,self.file.ordinary_move_source(self.grant)?))
    }
    /// Keep this OPEN's logical identity, mark, birth and original read
    /// revision. The worker has already checked and left these exact F/
    /// ancestry/FILE holders open. The owner rechecks logical selection but
    /// never opens a different target by name or falls back to the source.
    pub(crate) fn adopt_ordinary_move(&mut self,tree:&RustRoot,member:&RustSessionWitness,ready:ConfirmedOrdinaryMove)
        ->io::Result<RustAdoptedMove>{
        let unknown=file_rename::unknown;
        if !self.belongs_to(tree)||!ready.held_target().matches_source(&self.file,self.grant){return Err(unknown());}
        let (ready,label,grant,target)=ready.for_adoption(tree,member,&self.label,&self.route,self.grant)?;
        let MovedFile{original,media,route,..}=ready;
        // Infallible local adoption, FILE before its old parent is released.
        self.file=original;
        self.parent=media;
        self.label=label;self.route=route;self.grant=grant;
        Ok(RustAdoptedMove{mark:Rc::downgrade(&self.mark),target})
    }
    pub(crate) fn require_adopted_move(&self,tree:&RustRoot,member:&RustSessionWitness,receipt:&RustAdoptedMove)
        ->io::Result<()>{
        if !receipt.mark.ptr_eq(&Rc::downgrade(&self.mark))||!self.belongs_to(tree)||
            !receipt.target.names_adopted(&self.label,&self.route,self.grant,&self.file.place,&self.file.birth,self.file.live){
            return Err(file_rename::unknown());
        }
        receipt.target.require_adopted(tree,member,&self.parent)
    }
    /// This exact held reader is rebound only after the common kind14
    /// engine attests the same physical FILE with its original short EX.
    /// No staging writer or other read OPEN can be replaced here.
    pub(crate) fn rename_original(&mut self,tree:&RustRoot,ask:&OrdinaryRenameAdmission<'_>)
        ->io::Result<RustOrdinaryRename>{
        let (member,target)=ask.held(tree.pool(),self.file())?;
        if self.grant.mode!=Mode::ReadWrite{return Err(denied());}
        let (parents,name)=match target{
            FileRenameTarget::SameParent(name)=>(None,*name),
            FileRenameTarget::OtherParent{label,parents,name}=>{
                if *label!=self.label.as_str(){return Err(denied());}
                (Some(parents.as_slice()),*name)
            },
        };
        self.require_current(tree,member)?;
        let view=HeldRename{file:&self.file,label:&self.label,route:&self.route,grant:self.grant,media:&self.parent};
        let original=match file_rename::rename_held(tree,member,view,parents,name)?{
            file_rename::HeldRenameResult::SameParent(renamed)=>{
                let (place,body,original)=renamed.into_parts();
                self.file.container.rebind_renamed(self.parent.folder(),&body).map_err(|_|file_rename::unknown())?;
                self.file.place=place;
                drop(body); // only the rename-time exact FILE EX is dropped
                original
            },
            file_rename::HeldRenameResult::OtherParent{renamed,route,parent}=>{
                let (place,body,original)=renamed.into_parts();
                // The shared Native verifier compares body inode, mount,
                // header and pocket. Retain the existing reader, its data
                // and its FileId; move in the actual destination hold.
                self.file.container.rebind_moved(parent.folder(),body).map_err(|_|file_rename::unknown())?;
                self.file.place=place;
                self.parent=parent;
                self.route=route;
                original
            },
        };
        let proof=RustOrdinaryRename{mark:Rc::downgrade(&self.mark),original};
        self.require_rename(tree,member,&proof).map_err(|_|file_rename::unknown())?;
        Ok(proof)
    }
    pub(crate) fn require_rename(&self,tree:&RustRoot,member:&RustSessionWitness,proof:&RustOrdinaryRename)
        ->io::Result<()>{
        if !member.is_set_info()||self.grant.mode!=Mode::ReadWrite||!proof.mark.ptr_eq(&Rc::downgrade(&self.mark)){
            return Err(denied());
        }
        self.with_read(tree,member,|r|r.check_with(||file_rename::require_writable(&self.parent),
            |db|proof.original.require(db,&r.file.birth,&r.file.place)))
    }
    pub(crate) fn retire_rename(self,tree:&RustRoot,member:&RustSessionWitness,proof:RustOrdinaryRename)
        ->io::Result<RustRetiredOrdinaryRename>{
        self.require_rename(tree,member,&proof)?;
        let Self{file,parent,tree,label,grant,route,..}=self;
        let ReadFile{body,container,birth,place,..}=file;
        drop(body);drop(container); // the exact FILE CLOSE, no data handle remains
        Ok(RustRetiredOrdinaryRename{media:parent,tree,label,grant,route,birth,place,original:proof.original})
    }
    // A writer changes only its own original FILE, so bind an independent
    // current read view to that exact held placement, not to a new name.
    // This is not a second protocol OPEN or share vote.
    pub(in crate::system::reader::smb) fn for_writer(
        writer:&RustWorkWriter,tree:&RustRoot,member:&RustSessionWitness,
    )->io::Result<Self>{
        writer.require_current(tree,member)?;
        let source=tree.ordinary_reader(member)?;
        let original=writer.original_place();
        let (catalog,grant,route,placed,before)=source.at(|db,c|{
            let grant=tree.ordinary_folder_at(db,&c,writer.label())?;
            if grant!=writer.setting(){return Err(denied());}
            let route=Route::at(db,&c,grant.volume,original.parent)?;
            if &route!=writer.route(){return Err(denied());}
            let placed=file_create::placed_at(db,&c,grant.volume,original.parent,&original.name,Some(original.file))?;
            if &placed.here!=original{return Err(denied());}
            let selected=work_record::selected(db,&placed.birth)?;
            let before=ReadCut::gather(db,&c,&placed.birth,&placed.here,selected)?;
            Ok((c,grant,route,placed,before))
        })?;
        let mut name=String::new();
        name.try_reserve_exact(writer.label().len()).map_err(|_|io::ErrorKind::OutOfMemory)?;
        name.push_str(writer.label());
        let reader=RustFileSelection{tree,member,label:name,grant,catalog,route,placed,before}.acquire()?;
        writer.require_current(tree,member)?;
        Ok(reader)
    }
    pub(in crate::system::reader::smb) fn working_original(&self)->Option<OperationId>{
        self.file.live.map(|r|r.record.key().save)
    }
    pub(in crate::system::reader::smb) fn working_range(&self,tree:&RustRoot,member:&RustSessionWitness,
        offset:u64,length:u64,out:impl Write)->io::Result<Used>{
        self.with_read(tree,member,|r|r.range_evidence(offset,length,out))
    }
    pub(in crate::system::reader::smb) fn require_working_range(&self,tree:&RustRoot,member:&RustSessionWitness,
        used:&Used)->io::Result<()>{
        self.with_read(tree,member,|r|r.finish_sources(used))
    }

    fn name_position(&self)->(OperationId,i64){(self.file.place.op,self.file.place.generation)}
    pub(crate) fn normalized_name(&self,tree:&RustRoot,member:&RustSessionWitness)->io::Result<RustFileName>{
        if !member.is_query_info(){return Err(denied());}
        self.require_current(tree,member)?;
        let path=normalized_share_path(self.original_parent_components().chain(std::iter::once(self.file.place.name.as_str())))?;
        Ok(RustFileName{mark:Rc::downgrade(&self.mark),placement:self.name_position(),path})
    }
    pub(crate) fn require_normalized_name(&self,tree:&RustRoot,member:&RustSessionWitness,proof:&RustFileName)->io::Result<()>{
        if !member.is_query_info()||!proof.mark.ptr_eq(&Rc::downgrade(&self.mark))||proof.placement!=self.name_position(){
            return Err(denied());
        }
        self.require_current(tree,member)
    }

    pub(crate) fn standard(&self,tree:&RustRoot,member:&RustSessionWitness)->io::Result<RustFileStandard>{
        if !member.is_query_info(){return Err(denied());}
        self.require_current(tree,member)?;
        Ok(RustFileStandard{mark:Rc::downgrade(&self.mark),eof:self.eof()})
    }
    pub(crate) fn require_standard(&self,tree:&RustRoot,member:&RustSessionWitness,proof:&RustFileStandard)->io::Result<()>{
        if !member.is_query_info()||!proof.mark.ptr_eq(&Rc::downgrade(&self.mark))||proof.eof!=self.eof(){
            return Err(denied());
        }
        self.require_current(tree,member)
    }

    pub(crate) fn read_window(&self,tree:&RustRoot,member:&RustSessionWitness,offset:u64,length:u32)
        ->io::Result<(Vec<u8>,RustReadProof)>{
        if !member.is_read(){return Err(denied());}
        let progress=||tree.ordinary_read_progress(member);
        let (bytes,used)=self.with_read(tree,member,|r|smb_buffer::prepare(offset,length,self.eof(),
            |at,n,b|r.range_evidence_while(at,n,b,&progress),|used|{
                progress()?;r.finish_sources(used)?;progress()
            }))?;
        Ok((bytes,RustReadProof{mark:Rc::downgrade(&self.mark),used}))
    }
    pub(crate) fn require_read(&self,tree:&RustRoot,member:&RustSessionWitness,proof:&RustReadProof)->io::Result<()>{
        if !member.is_read()||!proof.mark.ptr_eq(&Rc::downgrade(&self.mark)){return Err(denied());}
        self.with_read(tree,member,|r|r.finish_sources(&proof.used))
    }
}

#[cfg(test)]
#[path="../test/smb/core_file_normalized_name.rs"]
mod normalized_name_tests;
