//! The retained branch of the one authenticated ZXS tree. SQL chooses
//! the exact original DeleteOp and display name. Owned holds never borrow
//! the tree or become an ordinary F media/FILE writer.
use super::{Catalog,Connection,Cut,Item,KeepReader,MatchRound,ReadAt,RecycleMedia,RecycleRoot,
    RetainedAt,RetainedFile,choose_file_at,delete_id,smb_buffer};
use crate::{FileId,OperationId,VolumeId,fs::invalid,ordinary::working_round::Used,smb::RustSessionWitness,
    system::reader::smb::{denied,runtime::{RustRoot,RustRootStamp}}};
use std::{io,rc::{Rc,Weak}};

#[must_use]
pub(crate) enum RustRetainedKind<'a>{Directory(SelectedDirectory<'a>),File(SelectedFile<'a>)}
struct Selection<'a>{
    tree:&'a RustRoot,scope:&'a RustSessionWitness,catalog:Catalog,volume:VolumeId,
}
pub(crate) struct SelectedDirectory<'a>{scope:Selection<'a>,round:Option<OperationId>}
pub(crate) struct SelectedFile<'a>{scope:Selection<'a>,original:Cut}

impl RustRoot{
    pub(crate) fn choose_retained<'a>(&'a self,scope:&'a RustSessionWitness,label:&str,
        rest:&[&str],
    )->io::Result<RustRetainedKind<'a>>{
        if !scope.is_create()||!self.names_retained_role(label){return Err(denied());}
        let source=self.ordinary_reader(scope)?;
        let round=rest.first().map(|n|delete_id(n.as_bytes())).transpose()?;
        if rest.len()>2{return Err(io::ErrorKind::NotADirectory.into());}
        let (catalog,volume,original)=source.at(|db,c|{
            let volume=self.retained_volume_at(db,&c)?;
            let original=match (round,rest.get(1)){
                (Some(id),Some(name))=>Some(choose_file_at(db,&c,volume,id,name)?),
                (Some(_),None)|(None,None)=>None,
                (None,Some(_))=>return Err(invalid("retained item has no original deletion")),
            };
            Ok((c,volume,original))
        })?;
        self.ordinary_reader(scope)?;
        let proof=Selection{tree:self,scope,catalog,volume};
        Ok(match original{
            Some(original)=>RustRetainedKind::File(SelectedFile{scope:proof,original}),
            None=>RustRetainedKind::Directory(SelectedDirectory{scope:proof,round}),
        })
    }
}
impl Selection<'_>{
    fn media(&self)->io::Result<RecycleMedia>{
        let source=self.tree.ordinary_reader(self.scope)?;
        let root=RecycleRoot::at(source,&self.catalog)?;
        if root.volume()!=self.volume{return Err(denied());}
        Ok(root.into_media())
    }
}
impl SelectedDirectory<'_>{
    pub(crate) fn deletion(&self)->Option<OperationId>{self.round}
    pub(crate) fn acquire(self)->io::Result<RustRetainedDirectory>{
        let media=self.scope.media()?;
        let tree=self.scope.tree;let scope=self.scope.scope;
        let mut answer=RustRetainedDirectory{media,tree:tree.stamp(),item:None,identity:Rc::new(())};
        if let Some(round)=self.round{
            let item=use_site(tree,scope,&answer.media,|at|at.deletion(round))?;
            answer.item=Some(item);
        }
        answer.require_current(tree,scope)?;
        Ok(answer)
    }
}
impl SelectedFile<'_>{
    pub(crate) fn file(&self)->FileId{self.original.place.here.file}
    pub(crate) fn acquire(self,share_delete:bool)->io::Result<RustRetainedFile>{
        let media=self.scope.media()?;
        let tree=self.scope.tree;let scope=self.scope.scope;
        let keep=if share_delete{KeepReader::ShareDelete}else{KeepReader::WhileRetained};
        let deleted=self.original.place.here.op;
        let mut name=String::new();
        let display=&self.original.place.here.name;
        name.try_reserve_exact(display.len()).map_err(|_|io::ErrorKind::OutOfMemory)?;
        name.push_str(display);
        let original=self.original;
        let (file,_key)=use_site(tree,scope,&media,|at|at.open_selected(deleted,&name,keep,original))?;
        let answer=RustRetainedFile{file,media,tree:tree.stamp(),identity:Rc::new(())};
        answer.require_current(tree,scope)?;
        Ok(answer)
    }
}

fn use_site<T>(tree:&RustRoot,scope:&RustSessionWitness,media:&RecycleMedia,
    action:impl FnOnce(RetainedAt<'_>)->io::Result<T>,
)->io::Result<T>{
    let source=tree.ordinary_reader(scope)?;
    let require=|db:&Connection,c:&Catalog|{
        if tree.retained_volume_at(db,c)?!=media.volume(){return Err(denied());}
        Ok(())
    };
    let answer=action(RetainedAt{source,media,require:&require})?;
    tree.ordinary_reader(scope)?;
    Ok(answer)
}

#[must_use]
pub(crate) struct RustRetainedDirectory{media:RecycleMedia,tree:RustRootStamp,item:Option<Item>,identity:Rc<()>}
#[must_use]
pub(crate) struct RustRetainedPage{
    origin:Weak<()>,revision:i64,children:Vec<(String,Option<(u64,u64)>)>,opened_child:Option<RetainedFile>,
}
impl RustRetainedPage{
    pub(crate) fn rows(&self)->impl Iterator<Item=(&str,Option<(u64,u64)>)>+'_{
        self.children.iter().map(|(name,file)|(name.as_str(),*file))
    }
}
impl RustRetainedDirectory{
    pub(crate) fn belongs_to(&self,tree:&RustRoot)->bool{tree.matches_stamp(&self.tree)}
    pub(crate) fn deletion(&self)->Option<OperationId>{self.item.as_ref().map(|x|x.op)}
    pub(crate) fn require_current(&self,tree:&RustRoot,scope:&RustSessionWitness)->io::Result<()>{
        if !self.belongs_to(tree){return Err(denied());}
        use_site(tree,scope,&self.media,|at|{
            if let Some(expected)=&self.item{
                if at.deletion(expected.op)?!=*expected{return Err(denied());}
            }else{
                // A zero bound still qualifies the real R and active
                // credential held by this root; it is not an empty list.
                at.page(None,0)?;
            }
            Ok(())
        })
    }
    pub(crate) fn page(&self,tree:&RustRoot,scope:&RustSessionWitness,after:Option<&str>,limit:u8,
        matching:Option<MatchRound<'_>>,
    )->io::Result<RustRetainedPage>{
        if !scope.is_query_directory()||!self.belongs_to(tree){return Err(denied());}
        if limit==0||limit>16{return Err(invalid("invalid retained query limit"));}
        let source=tree.ordinary_reader(scope)?;
        let revision=smb_buffer::data_version(source)?;
        let (children,opened_child)=use_site(tree,scope,&self.media,|site|{
            let mut children=Vec::new();
            if let Some(round)=&self.item{
                if site.deletion(round.op)?!=*round{return Err(denied());}
                let mut work=0;
                let include=match matching{Some(f)=>f(&round.name,&mut work)?,None=>true};
                if include&&after.is_none_or(|name|round.name.as_bytes()>name.as_bytes()){
                    let (held,key)=site.open(round.op,&round.name,KeepReader::WhileRetained)?;
                    if held.place.here.file!=round.file{return Err(denied());}
                    let eof=held.view.map_or(0,|r|r.record.logical());
                    children.try_reserve(1).map_err(|_|io::ErrorKind::OutOfMemory)?;
                    let mut name=String::new();
                    name.try_reserve_exact(round.name.len()).map_err(|_|io::ErrorKind::OutOfMemory)?;
                    name.push_str(&round.name);
                    children.push((name,Some((key.words().1,eof))));
                    Ok((children,Some(held)))
                }else{Ok((children,None))}
            }else{
                // These are two different cursors: Rust selects from the
                // exact OperationId byte ordering; only the owned sender's
                // proposal later advances the OPEN to an emitted label.
                let after=after.map(|s|delete_id(s.as_bytes())).transpose()?;
                let rows=site.page_matching(after,limit,matching)?;
                children.try_reserve_exact(rows.len()).map_err(|_|io::ErrorKind::OutOfMemory)?;
                for row in rows{
                    let name=row.op.to_string();
                    if delete_id(name.as_bytes())?!=row.op{return Err(invalid("noncanonical retained operation"));}
                    children.push((name,None));
                }
                Ok((children,None))
            }
        })?;
        let page=RustRetainedPage{origin:Rc::downgrade(&self.identity),revision,children,opened_child};
        self.require_page(tree,scope,&page)?;
        Ok(page)
    }
    pub(crate) fn require_page(&self,tree:&RustRoot,scope:&RustSessionWitness,page:&RustRetainedPage)->io::Result<()>{
        if !scope.is_query_directory()||!page.origin.ptr_eq(&Rc::downgrade(&self.identity)){return Err(denied());}
        self.require_current(tree,scope)?;
        if let Some(real)=&page.opened_child{
            use_site(tree,scope,&self.media,|site|ReadAt{site,file:real,previous_after_restore:false}.finish(&Used::default()))?;
        }
        if smb_buffer::data_version(tree.ordinary_reader(scope)?)?!=page.revision{return Err(io::ErrorKind::WouldBlock.into());}
        Ok(())
    }
}

#[must_use]
pub(crate) struct RustRetainedFile{file:RetainedFile,media:RecycleMedia,tree:RustRootStamp,identity:Rc<()>}
#[must_use]
pub(crate) struct RustRetainedReadProof{original:Weak<()>,used:Used}
impl RustRetainedFile{
    pub(crate) fn file(&self)->FileId{self.file.place.here.file}
    pub(crate) fn belongs_to(&self,tree:&RustRoot)->bool{tree.matches_stamp(&self.tree)}
    pub(crate) fn size(&self)->u64{self.file.view.map_or(0,|r|r.record.logical())}
    pub(in crate::system::reader::smb) fn original_restore_source(&self,tree:&RustRoot,scope:&RustSessionWitness)
        ->io::Result<&super::Place>{
        if !scope.is_set_info(){return Err(denied());}
        // Mutating requires the actual current R. SHARE_DELETE can only
        // keep an already-open old read; it never resurrects a restore.
        self.require_current(tree,scope)?;
        Ok(&self.file.place.here)
    }
    pub(in crate::system::reader::smb) fn restore_identity(&self)->Weak<()>{Rc::downgrade(&self.identity)}
    pub(in crate::system::reader::smb) fn same_restore_origin(&self,tree:&RustRoot,stamp:&Weak<()>,source:&super::Place)->bool{
        self.belongs_to(tree)&&stamp.ptr_eq(&Rc::downgrade(&self.identity))&&source==&self.file.place.here
    }
    fn with_read<T>(&self,tree:&RustRoot,scope:&RustSessionWitness,
        action:impl FnOnce(ReadAt<'_>)->io::Result<T>,
    )->io::Result<T>{
        if !self.belongs_to(tree){return Err(denied());}
        // Even a SHARE_DELETE open must still select current R at CREATE
        // pre/post send. Only an already opened READ may retain old bytes.
        use_site(tree,scope,&self.media,|site|action(ReadAt{site,file:&self.file,previous_after_restore:scope.is_read()}))
    }
    pub(crate) fn require_current(&self,tree:&RustRoot,scope:&RustSessionWitness)->io::Result<()>{
        self.with_read(tree,scope,|at|at.finish(&Used::default()))
    }
    pub(crate) fn read(&self,tree:&RustRoot,scope:&RustSessionWitness,offset:u64,count:u32)
        ->io::Result<(Vec<u8>,RustRetainedReadProof)>{
        if !scope.is_read(){return Err(denied());}
        let (bytes,used)=self.with_read(tree,scope,|at|smb_buffer::prepare(offset,count,self.size(),
            |start,length,out|at.range_evidence(start,length,out),|used|at.finish(used)))?;
        Ok((bytes,RustRetainedReadProof{original:Rc::downgrade(&self.identity),used}))
    }
    pub(crate) fn require_read(&self,tree:&RustRoot,scope:&RustSessionWitness,proof:&RustRetainedReadProof)->io::Result<()>{
        if !scope.is_read()||!proof.original.ptr_eq(&Rc::downgrade(&self.identity)){return Err(denied());}
        self.with_read(tree,scope,|at|at.finish(&proof.used))
    }
}
