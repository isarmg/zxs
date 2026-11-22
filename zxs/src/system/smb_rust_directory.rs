//! The original ordinary directory pin for the Rust SMB family. The F
//! root and every nested parent use the same existing Core media proof.
use super::{Catalog,Child,Connection,InitialReader,NameFilter,PageAt,
    ordinary_directory::{self,ConfirmedRelabel,NamedKind,Route},read_page_bound};
use crate::{
    DirectoryId,OperationId,VolumeId,fs::{directory_birth::DirectoryParent,invalid},smb::{OrdinaryDirectoryRenameAdmission,RustSessionWitness},
    system::reader::{account::share_mode::{Mode,Setting},smb::{denied,first_work::RustFileName,runtime::{RustRoot,RustRootStamp},smb_buffer},
        volume::{OrdinaryMedia,OrdinaryRoot}},
};
use std::{io,rc::{Rc,Weak}};

pub(crate) enum RustOrdinaryKind<'a>{Directory(RustDirectorySelection<'a>),File}

/// Original name/type/authority selection, not an actual opened directory.
/// The same global share coordinator must reserve the identity before we hold
/// its media and recheck that nothing selected has been substituted.
#[must_use]
pub(crate) struct RustDirectorySelection<'a>{
    tree:&'a RustRoot,member:&'a RustSessionWitness,label:String,
    grant:Setting,catalog:Catalog,route:Route,
}
impl RustRoot{
    pub(crate) fn choose_ordinary_kind<'a>(&'a self,member:&'a RustSessionWitness,
        label:&str,names:&[&str])->io::Result<RustOrdinaryKind<'a>>{
        if !member.is_create(){return Err(denied());}
        let source=self.ordinary_reader(member)?;
        let (catalog,grant,kind)=source.at(|db,c|{
            let grant=self.ordinary_folder_at(db,&c,label)?;
            let kind=match names.split_last(){
                None=>NamedKind::Directory(Route::at(db,&c,grant.volume,DirectoryParent::Root)?),
                Some((leaf,parents))=>ordinary_directory::kind_at(db,&c,grant.volume,parents,leaf)?,
            };
            Ok((c,grant,kind))
        })?;
        self.ordinary_reader(member)?;
        let NamedKind::Directory(route)=kind else{return Ok(RustOrdinaryKind::File);};
        let mut name=String::new();
        name.try_reserve_exact(label.len()).map_err(|_|io::ErrorKind::OutOfMemory)?;
        name.push_str(label);
        Ok(RustOrdinaryKind::Directory(RustDirectorySelection{
            tree:self,member,label:name,grant,catalog,route,
        }))
    }
}
impl<'a> RustDirectorySelection<'a>{
    pub(in crate::system::reader::smb) fn selected(
        tree:&'a RustRoot,member:&'a RustSessionWitness,label:String,
        grant:Setting,catalog:Catalog,route:Route,
    )->Self{Self{tree,member,label,grant,catalog,route}}
    pub(crate) fn volume(&self)->VolumeId{self.grant.volume}
    pub(crate) fn nested(&self)->Option<DirectoryId>{nested(&self.route)}
    // A volume label has the immutable ZXS root as its parent, even on a RW share.
    pub(crate) fn parent_allows_file_write(&self)->bool{allows_parent_write(&self.route,self.grant)}
    pub(crate) fn acquire(self)->io::Result<RustOrdinaryDirectory>{
        let Self{tree,member,label,grant,catalog,route}=self;
        let source=tree.ordinary_reader(member)?;
        let media=OrdinaryRoot::at_parent(source,&catalog,&route,None)
            .map_err(|e|match e.kind(){
                io::ErrorKind::NotFound|io::ErrorKind::NotADirectory=>io::Error::new(io::ErrorKind::NotConnected,
                    "original ordinary directory medium is offline"),
                _=>e,
            })?.into_media();
        Ok(RustOrdinaryDirectory{tree:tree.stamp(),label,grant,route,media:Some(media),open:Rc::new(())})
    }
}
fn nested(route:&Route)->Option<DirectoryId>{
    match route.parent(){DirectoryParent::Root=>None,DirectoryParent::Directory(id)=>Some(id)}
}
fn allows_parent_write(route:&Route,grant:Setting)->bool{
    nested(route).is_some()&&grant.mode==Mode::ReadWrite
}

/// Owns the real F, marker and exact selected ancestry; the SMB tree still
/// owns the real SQL reader. No self-reference, pathname open or inferred grant.
#[must_use]
pub(crate) struct RustOrdinaryDirectory{
    tree:RustRootStamp,label:String,grant:Setting,route:Route,media:Option<OrdinaryMedia>,open:Rc<()>,
}
impl RustOrdinaryDirectory{
    pub(in crate::system::reader::smb) fn from_birth(
        tree:&RustRoot,member:&RustSessionWitness,label:String,grant:Setting,route:Route,media:OrdinaryMedia,
    )->io::Result<Self>{
        if !member.is_create()||grant.mode!=Mode::ReadWrite||media.parent()!=route.parent(){return Err(denied());}
        let original=Self{tree:tree.stamp(),label,grant,route,media:Some(media),open:Rc::new(())};
        original.require_current(tree,member)?;
        Ok(original)
    }
    pub(crate) fn volume(&self)->VolumeId{self.grant.volume}
    pub(crate) fn nested(&self)->Option<DirectoryId>{nested(&self.route)}
    pub(crate) fn parent_allows_file_write(&self)->bool{allows_parent_write(&self.route,self.grant)}
    pub(crate) fn belongs_to(&self,tree:&RustRoot)->bool{tree.matches_stamp(&self.tree)}
    fn media(&self)->io::Result<&OrdinaryMedia>{
        // A lost leaf after an ambiguous relabel never turns the retained
        // parent FD into a usable OPEN on some other directory.
        self.media.as_ref().ok_or_else(||io::ErrorKind::ConnectionAborted.into())
    }
    fn source<'t>(&self,tree:&'t RustRoot,member:&RustSessionWitness)->io::Result<&'t InitialReader>{
        if !self.belongs_to(tree){return Err(denied());}
        tree.ordinary_reader(member)
    }
    fn require_at(&self,tree:&RustRoot,db:&Connection,c:&Catalog)->io::Result<()>{
        if tree.ordinary_folder_at(db,c,&self.label)?!=self.grant{return Err(denied());}
        if Route::at(db,c,self.grant.volume,self.route.parent())?!=self.route{
            return Err(invalid("original ordinary directory ancestry changed"));
        }
        self.media()?.capacity_origin().require_catalog(c,self.grant.volume)
    }
    pub(crate) fn require_current(&self,tree:&RustRoot,member:&RustSessionWitness)->io::Result<()>{
        let source=self.source(tree,member)?;
        self.media()?.view(source).require_streaming()?;
        source.at(|db,c|self.require_at(tree,db,&c))?;
        self.source(tree,member)?;
        Ok(())
    }
}

/// Normalized logical path of one exact ordinary directory OPEN. The
/// optional selected place is absent only for the linked F volume label.
/// Neither a host pathname nor an independent OPEN can mint this result.
#[must_use]
pub(crate) struct RustDirectoryName{
    open:Weak<()>,position:Option<(OperationId,i64)>,path:String,
}
impl RustDirectoryName{
    pub(crate) fn path(&self)->&str{&self.path}
    fn capture<'a>(open:&Rc<()>,position:Option<(OperationId,i64)>,label:&'a str,
        steps:impl Iterator<Item=&'a str>)->io::Result<Self>{
        let path=RustFileName::format(std::iter::once(label).chain(steps))?;
        Ok(Self{open:Rc::downgrade(open),position,path})
    }
    fn ties(&self,open:&Rc<()>,position:Option<(OperationId,i64)>)->bool{
        self.open.ptr_eq(&Rc::downgrade(open))&&self.position==position
    }
}
impl RustOrdinaryDirectory{
    fn name_position(&self)->Option<(OperationId,i64)>{
        self.route.selected().map(|place|(place.position(),place.generation()))
    }
    pub(crate) fn normalized_name(&self,tree:&RustRoot,member:&RustSessionWitness)->io::Result<RustDirectoryName>{
        if !member.is_query_info(){return Err(denied());}
        self.require_current(tree,member)?;
        RustDirectoryName::capture(&self.open,self.name_position(),&self.label,self.route.components())
    }
    pub(crate) fn require_normalized_name(&self,tree:&RustRoot,member:&RustSessionWitness,
        proof:&RustDirectoryName)->io::Result<()>{
        if !member.is_query_info()||!proof.ties(&self.open,self.name_position()){return Err(denied());}
        // Only a relabel of this same OPEN can replace its selected Route.
        // Its leaf place then changes; any independently changed ancestor
        // or setting instead fails the existing exact live route comparison.
        self.require_current(tree,member)
    }
}

#[must_use]
pub(crate) struct RustDirectoryRename{original:ConfirmedRelabel,open:Weak<()>}
impl RustOrdinaryDirectory{
    /// Only explicit DELETE on an actual nested RW directory may acquire
    /// the rename lane. No namespace root gains a DELETE grant.
    pub(crate) fn require_relabel_grant(&self,tree:&RustRoot,member:&RustSessionWitness)->io::Result<()>{
        if !member.is_create()||!self.parent_allows_file_write(){return Err(denied());}
        self.require_current(tree,member)?;
        if self.media()?.capacity_origin().sample()?.read_only{return Err(io::ErrorKind::ReadOnlyFilesystem.into());}
        self.require_current(tree,member)
    }
    pub(crate) fn original_parent_components(&self)->impl Iterator<Item=&str>{
        std::iter::once(self.label.as_str()).chain(self.route.parent_components())
    }
    pub(crate) fn original_leaf(&self)->Option<&str>{self.route.selected().map(|p|p.name())}

    pub(crate) fn rename_in_parent(&mut self,tree:&RustRoot,ask:&OrdinaryDirectoryRenameAdmission<'_>)
        ->io::Result<RustDirectoryRename>{
        let (member,_)=ask.held(tree.pool(),self.nested().ok_or_else(denied)?)?;
        if !self.parent_allows_file_write(){return Err(denied());}
        self.require_current(tree,member)?;
        let source=self.source(tree,member)?;
        // Move, never duplicate, this directory's physical hold and its
        // leaf OFD. The share turn excludes direct competing OPENs, and
        // this command never yields its leaf to a query or reply.
        let media=self.media.take().ok_or_else(denied)?;
        let mut physical=OrdinaryRoot::adopt_media(source,media);
        let mut owner=Some(physical.take_directory_leaf().map_err(unknown_relabel)?);
        let (route,leaf,receipt)=match ordinary_directory::relabel_rust(tree,ask,&self.label,self.grant,&self.route,&mut owner){
            Ok(moved)=>moved,
            Err(e) if e.kind()==io::ErrorKind::ConnectionAborted=>return Err(e),
            Err(e)=>{
                let Some(unchanged)=owner else{return Err(unknown_relabel(e));};
                physical.return_directory_leaf(unchanged).map_err(unknown_relabel)?;
                self.media=Some(physical.into_media());
                return Err(e);
            },
        };
        physical.return_directory_leaf(leaf).map_err(unknown_relabel)?;
        self.route=route;
        self.media=Some(physical.into_media());
        let proof=RustDirectoryRename{original:receipt,open:Rc::downgrade(&self.open)};
        if let Err(e)=self.require_rename(tree,member,&proof){
            drop(self.media.take());
            return Err(unknown_relabel(e));
        }
        Ok(proof)
    }

    pub(crate) fn require_rename(&self,tree:&RustRoot,member:&RustSessionWitness,proof:&RustDirectoryRename)->io::Result<()>{
        if !member.is_set_info()||!self.parent_allows_file_write()||!proof.open.ptr_eq(&Rc::downgrade(&self.open)){
            return Err(denied());
        }
        let source=self.source(tree,member)?;
        self.media()?.view(source).require_streaming()?;
        if self.media()?.capacity_origin().sample()?.read_only{return Err(io::ErrorKind::ReadOnlyFilesystem.into());}
        source.at(|db,c|{self.require_at(tree,db,&c)?;proof.original.require(db,&c,&self.route)})?;
        self.source(tree,member)?;
        Ok(())
    }
}
fn unknown_relabel(cause:io::Error)->io::Error{io::Error::new(io::ErrorKind::ConnectionAborted,cause)}


/// An authenticated snapshot of checked business children; not an OPEN,
/// binding, lease or assertion that a child's later body I/O will succeed.
#[must_use]
pub(crate) struct RustDirectoryPage{
    rows:Vec<Child>,open:Weak<()>,db_version:i64,
}
impl RustDirectoryPage{
    pub(crate) fn rows(&self)->impl Iterator<Item=(&str,Option<(u64,u64)>)>+'_{
        // Optional exact (64-bit file number, selected logical EOF) is
        // present only for ordinary FILE, never a DirectoryId truncation.
        self.rows.iter().map(|row|match row{
            Child::Directory(d)=>(d.name(),None),
            Child::File(f)=>(f.name.as_str(),Some((f.number,f.size))),
        })
    }
}
impl RustOrdinaryDirectory{
    pub(crate) fn page(&self,tree:&RustRoot,member:&RustSessionWitness,after:&[u8],limit:u8,
        filter:Option<&NameFilter<'_>>)->io::Result<RustDirectoryPage>{
        if !member.is_query_directory(){return Err(denied());}
        let source=self.source(tree,member)?;
        let db_version=smb_buffer::data_version(source)?;
        let at=PageAt{source,volume:self.grant.volume,parent:self.route.parent()};
        let rows=read_page_bound(at,self.media()?.view(source),after,limit,true,filter,
            |db,c|self.require_at(tree,db,c),||self.source(tree,member).map(drop))?;
        let page=RustDirectoryPage{rows,open:Rc::downgrade(&self.open),db_version};
        self.require_page(tree,member,&page)?;
        Ok(page)
    }
    pub(crate) fn require_page(&self,tree:&RustRoot,member:&RustSessionWitness,from:&RustDirectoryPage)->io::Result<()>{
        if !member.is_query_directory()||!from.open.ptr_eq(&Rc::downgrade(&self.open)){return Err(denied());}
        let source=self.source(tree,member)?;
        self.require_current(tree,member)?;
        // Before/after a same-stream response, no stale mode, erased name
        // or confirmed new size can adopt this pending cursor. If the
        // shared DB changes, even an unrelated commit may conservatively
        // invalidate a draft. No database lock is held across the socket.
        if smb_buffer::data_version(source)?!=from.db_version{
            return Err(io::Error::new(io::ErrorKind::WouldBlock,"ordinary directory snapshot changed"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod normalized_name_tests{
    use super::*;

    #[test]
    fn directory_name_uses_the_linked_label_and_every_selected_ancestor(){
        let open=Rc::new(());
        let label=RustDirectoryName::capture(&open,None,"Data-é",std::iter::empty()).unwrap();
        assert_eq!(label.path(),r"\Data-é");
        let named=RustDirectoryName::capture(&open,Some((OperationId([1;16]),1)),
            "Data-é",["Ärchive","报告🚀"].into_iter()).unwrap();
        assert_eq!(named.path(),r"\Data-é\Ärchive\报告🚀");
        assert!(RustDirectoryName::capture(&open,None,"Data-é",["..","Leaf"].into_iter()).is_err());
    }

    #[test]
    fn directory_name_witness_rejects_an_equal_open_or_reused_spelling_after_relabel(){
        let open=Rc::new(());
        let old=Some((OperationId([1;16]),1));
        let later=Some((OperationId([3;16]),3));
        let name=RustDirectoryName::capture(&open,old,"Data",["Reports"].into_iter()).unwrap();
        let renamed_back=RustDirectoryName::capture(&open,later,"Data",["Reports"].into_iter()).unwrap();
        assert_eq!(name.path(),renamed_back.path());
        assert!(name.ties(&open,old));
        assert!(!name.ties(&Rc::new(()),old));
        assert!(!name.ties(&open,None));
        assert!(!name.ties(&open,later));
    }
}
