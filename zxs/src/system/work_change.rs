//! One bounded business writer for this original ordinary FILE. The per-FILE
//! native directory EX is held across all saves on this handle; compatible
//! read handles never take it. SQLite carries only the exact original decisions.
use super::{Catalog,Connection,Tree,check,sqlite3_step};
use super::super::{file_location::{Place,Placed},volume::OrdinaryRoot,work_record::{self,Edit,Issued,Members,ReadRecord}};
use crate::{
    FileId,OperationId,display,fs::{WitnessRead,invalid},
    ordinary::{portable_birth::{Birth,Installed},working_round::{Draft,End,Key,Record,Selected,Span}},
};
use std::{fs::File,io};
use super::first_work::Current;


pub(crate) struct ReservedEdit<'a>{installed:&'a Installed,record:Record}
impl ReservedEdit<'_>{
    pub(crate) fn original(&self)->(&Installed,Record){(self.installed,self.record)}
}
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(crate) struct Receipt {pub(crate) key:Key,pub(crate) size:u64}
impl From<Record> for Receipt{
    fn from(r:Record)->Self{Self{key:r.key(),size:r.logical()}}
}
#[derive(Clone,Copy,Eq,PartialEq)]
pub(in crate::system::reader) enum Cut{Reserved,BytesSynced}
fn busy(message:&str)->io::Error{io::Error::new(io::ErrorKind::WouldBlock,message.to_owned())}

#[derive(Eq,PartialEq)]
struct State {
    selected:Option<(ReadRecord,End)>,
    latest:Option<Issued>,
    pending:Option<Issued>,
    manual:Option<(OperationId,OperationId,OperationId,i64)>,
    members:Members,
    alias:Option<super::file_create::Alias>,
}
impl State{
    fn at(db:&Connection,c:&Catalog,b:&Birth,at:&Place)->io::Result<Self>{
        let selected=work_record::selected(db,b)?.map(|r|work_record::ceiling(db,b,r).map(|end|(r,end))).transpose()?;
        Ok(Self{selected,latest:work_record::last_issue(db,b)?,pending:work_record::pending_current(db,b)?,
            manual:work_record::latest_commit(db,b)?,members:work_record::members(db,c,b)?,
            alias:super::file_create::display_owner_at(db,c,at.volume,&at.name)?})
    }
    fn base(&self)->Option<OperationId>{self.selected.map(|(r,_)|r.record.key().save)}
    fn parent(&self)->Option<OperationId>{self.manual.map(|(op,_,_,_)|op)}
    fn pending(&self)->Option<Issued>{self.pending}
}

pub(crate) struct Writer<'t,'s,'r>{
    installed:Installed,
    root:OrdinaryRoot<'r>,
    tree:&'t Tree<'s,'r>,
    birth:Birth,
    place:Place,
    nonce:OperationId,
}
impl<'s,'r> Tree<'s,'r>{
    pub(super) fn writer_from_newborn<'t>(&'t self,born:super::file_create::NativeBorn<'r>,nonce:OperationId)->io::Result<Writer<'t,'s,'r>>{
        let (root,birth,installed)=born.parts(true)?;
        let place=Place::origin(&birth);
        let handle=Writer{installed,root,tree:self,birth,place,nonce};
        handle.require_work()?;Ok(handle)
    }
    pub(super) fn open_named_writer(&self,name:&str)->io::Result<Writer<'_,'s,'r>>{
        display::component(name.as_bytes(),222)?;
        let file=self.session.source.at(|db,c|{
            self.require(db,&c,true)?;
            Ok(self.placed(db,&c,name,None)?.here.file)
        })?;
        self.open_writer(name,file)
    }
    pub(crate) fn open_writer<'t>(&'t self,name:&str,file:FileId)->io::Result<Writer<'t,'s,'r>>{
        display::component(name.as_bytes(),222)?;let src=self.session.source;
        let (catalog,located,selected)=src.at(|db,c|{
            self.require(db,&c,true)?;
            let located=self.placed(db,&c,name,Some(file))?;
            let selected=State::at(db,&c,&located.birth,&located.here)?;Ok((c,located,selected))
        })?;
        let root=OrdinaryRoot::for_body(src,&catalog,located.here.volume)?;
        let names=root.root.names()?.into_iter().collect();
        let installed=located.open(&root.root,&names,true,|s|selected.members.require(s))?;
        let Placed{birth,here:place}=located;
        let handle=Writer{installed,root,tree:self,birth,place,nonce:OperationId::new()?};
        handle.physical(&selected)?;
        src.at(|db,c|{
            if handle.state(db,&c)?!=selected{return Err(busy("work changed during writer admission"));}
            Ok(())
        })?;
        Ok(handle)
    }
}
impl<'t,'s,'r> Writer<'t,'s,'r>{
    pub(super) fn require_tree_root(&self,original:&crate::fs::HeldDir)->io::Result<()>{
        if self.root.root.entity!=original.entity||!self.root.root.same_mount(original)?{
            return Err(invalid("writer cannot borrow a replacement native tree root"));
        }
        original.require_named()?;self.root.require_streaming()
    }
    pub(super) fn work_directory(&self)->&crate::fs::HeldDir{self.installed.directory()}
    pub(super) fn staging_free(&self)->io::Result<u64>{Ok(self.root.space()?.available_bytes)}
    pub(super) fn current_read(&self)->io::Result<Current<'t,'s,'r>>{
        let tree:&'t Tree<'s,'r>=self.tree;
        let found=tree.open_current(&self.place.name)?;
        if found.file()!=self.birth.file{return Err(invalid("writer reader does not own the original FILE"));}
        Ok(found)
    }
    pub(super) fn require_work(&self)->io::Result<()>{
        let src=self.tree.session.source;
        let x=src.at(|db,c|self.state(db,&c))?;
        self.physical(&x)?;
        src.at(|db,c|{
            if self.state(db,&c)?!=x{return Err(busy("work writer lost its original cut"));}
            Ok(())
        })
    }
    fn state(&self,db:&Connection,c:&Catalog)->io::Result<State>{
        self.tree.require(db,c,true)?;
        let located=self.tree.placed(db,c,&self.place.name,Some(self.birth.file))?;
        if located.birth!=self.birth||located.here!=self.place{
            return Err(invalid("work writer lost its original FILE placement"));
        }
        State::at(db,c,&self.birth,&self.place)
    }
    fn physical(&self,x:&State)->io::Result<()>{
        let alias=super::directory::check_alias_name(&self.root,&self.place.name,x.alias.as_ref(),None)?;
        x.members.require(&WitnessRead::at(self.installed.directory())?)?;
        self.root.require_streaming()?;self.installed.require(&self.root.root)?;
        drop(alias);Ok(())
    }
    fn original(&self,db:&Connection,op:OperationId)->io::Result<Option<Issued>>{
        let auth=&self.tree.session.original;let setting=self.tree.setting;
        let row=work_record::edit(db,&self.birth,op)?;
        if row.is_some_and(|i| {
            let e=i.edit;
            (e.actor,e.credential,e.generation,e.share,e.setting,e.position,e.volume)!=
                (auth.account,auth.op,auth.generation,setting.share,setting.op,self.place.op,self.place.volume)
        }){return Err(invalid("this original SAVE was not issued by the current writer authority"));}
        Ok(row)
    }
    pub(crate) fn receipt(&self,op:OperationId)->io::Result<Option<Receipt>>{
        self.tree.session.source.at(|db,c|{
            self.state(db,&c)?;
            Ok(self.original(db,op)?.filter(|r|r.confirmed).map(|r|r.edit.record.into()))
        })
    }
    /// One actual range edit or resize. A resize uses an independently
    /// completed empty staging FD and no write offset. Neither issues a Node.
    pub(crate) fn save(&self,op:OperationId,expected:Option<OperationId>,offset:Option<u64>,final_size:u64,input:&File)->io::Result<Receipt>{
        self.save_after(op,expected,offset,final_size,input,|_|Ok(()))
    }
    pub(in crate::system::reader) fn save_after(&self,op:OperationId,expected:Option<OperationId>,offset:Option<u64>,
        final_size:u64,input:&File,point:impl FnMut(Cut)->io::Result<()>)->io::Result<Receipt>{
        self.save_staged(op,expected,|k,start,base|Draft::stage(k,start,base,offset,final_size,input),&||Ok(()),point)
    }
    /// Submit all effective intervals of one original SAVE in one native
    /// append and one metadata transition. No per-WRITE version is created.
    pub(crate) fn save_packed(&self,op:OperationId,expected:Option<OperationId>,preserved:u64,eof:u64,
        spans:&[Span],packed:&File)->io::Result<Receipt>{
        self.save_packed_guarded(op,expected,preserved,eof,spans,packed,&||Ok(()))
    }
    pub(super) fn save_packed_guarded(&self,op:OperationId,expected:Option<OperationId>,preserved:u64,eof:u64,
        spans:&[Span],packed:&File,alive:&impl Fn()->io::Result<()>)->io::Result<Receipt>{
        self.save_staged(op,expected,|k,start,base|Draft::packed(k,start,base,preserved,eof,spans,packed),alive,|_|Ok(()))
    }
    fn save_staged<'a>(&self,op:OperationId,expected:Option<OperationId>,
        draft_with:impl FnOnce(Key,End,Option<&Selected>)->io::Result<Draft<'a>>,alive:&impl Fn()->io::Result<()>,
        mut point:impl FnMut(Cut)->io::Result<()>)->io::Result<Receipt>{
        alive()?;let src=self.tree.session.source;let dir=self.installed.directory();
        let (before,known,base) = src.at(|db,c|{
            let state=self.state(db,&c)?;
            let known=self.original(db,op)?;
            let base=if let Some(old)=known{
                if expected!=old.edit.predecessor{return Err(invalid("the same original SAVE used a different predecessor"));}
                work_record::basis(db,&self.birth,old.edit.predecessor)?
            }else{
                if expected!=state.base(){return Err(busy("the expected working baseline changed"));}
                state.selected
            };
            Ok((state,known,base))
        })?;
        self.physical(&before)?;
        let selected=base.map(|(r,physical)|Selected::open(dir,r.record,physical)).transpose()?;
        let (key,start)=if let Some(old)=known{
            let r=old.edit.record;(r.key(),r.start())
        }else if let Some((previous,_))=base.filter(|(r,_)|before.manual.is_none_or(|(_,_,round,_)|r.record.key().round!=round)){
            let from=previous.record;
            let k=from.key();
            let next=k.revision.checked_add(1).filter(|r|*r<i64::MAX as u64).ok_or_else(||invalid("work revision exhausted"))?;
            let mut start=from.end();
            if let Some(p)=before.pending().filter(|r|r.edit.record.key().round==k.round){
                if p.edit.nonce==self.nonce{return Err(busy("this writer still owns its exact prior pending SAVE"));}
                start=selected.as_ref().ok_or_else(||invalid("missing selected original pair"))?.durable_observed_end(dir)?;
            }
            (Key{save:op,revision:next,..k},start)
        }else{(Key{pool:self.birth.pool,file:self.birth.file,round:op,save:op,revision:1},End::ZERO)};
        let draft=draft_with(key,start,selected.as_ref())?;
        let prepared=draft.record();
        let reservation=if let Some(old)=known{
            if old.edit.record!=prepared{return Err(invalid("same SAVE operation differs in staged request"));}
            if old.confirmed{alive()?;return Ok(prepared.into());}
            if before.pending()!=Some(old){return Err(busy("this original SAVE was superseded"));}
            old.edit
        }else{
            let pending=before.pending();
            if pending.is_some_and(|p|p.edit.nonce==self.nonce){return Err(busy("complete the original pending SAVE first"));}
            let scope=WitnessRead::at(dir)?;before.members.require(&scope)?;
            let old_round=pending.filter(|p|p.edit.record.key().round!=key.round).map(|p|p.edit.record.key().round);
            let retained=old_round.map_or(0,|id|scope.present_work_roles(&crate::ordinary::working_round::names(id)));
            if key.revision==1{
                before.members.fits_next_pair(key.round,old_round,retained)?;
            }
            let need=(prepared.end().data-start.data).checked_add(prepared.end().checks-start.checks)
                .ok_or_else(||invalid("working pair space overflow"))?;
            if self.root.space()?.available_bytes<need{return Err(io::Error::new(io::ErrorKind::StorageFull,"insufficient space for the actual work change"));}
            let a=&self.tree.session.original;let issue=before.latest.map_or(0,|i|i.edit.issue).checked_add(1)
                .filter(|v|*v<i64::MAX).ok_or_else(||invalid("work issuance exhausted"))?;
            let proposed=Edit{record:prepared,actor:a.account,credential:a.op,generation:a.generation,
                share:self.tree.setting.share,setting:self.tree.setting.op,position:self.place.op,volume:self.place.volume,nonce:self.nonce,issue,
                parent:before.parent(),predecessor:before.base(),previous_pending:pending.map(|p|p.edit.record.key().save),retained};
            self.physical(&before)?;
            let w=Connection::open(&src.dir,true)?;
            let outcome=src.transaction(&w,c"BEGIN IMMEDIATE",|db,c|{
                if self.state(db,&c)?!=before{return Err(busy("work or authority changed before reservation"));}
                alive()?;proposed.insert(db,&self.birth)
            });
            drop(w);
            if let Err(err)=outcome{
                match src.at(|db,c|{self.state(db,&c)?;self.original(db,op)}){
                    Ok(Some(same))if same.edit==proposed=>{},
                    Ok(None)=>return Err(err),
                    _=>return Err(io::Error::other("SAVE reservation unconfirmed; inspect the exact original")),
                }
            }
            proposed
        };
        drop(selected);
        point(Cut::Reserved)?;
        let pending=src.at(|db,c|{
            let x=self.state(db,&c)?;
            if x.pending()!=Some(Issued{edit:reservation,confirmed:false}){
                return Err(busy("not the current exact issued SAVE"));
            }Ok(x)
        })?;
        self.physical(&pending)?;alive()?;
        let proof=draft.prepare(&ReservedEdit{installed:&self.installed,record:prepared})?;
        point(Cut::BytesSynced)?;
        self.physical(&pending)?;proof.require(dir,prepared)?;
        let w=Connection::open(&src.dir,true)?;
        let out=src.transaction(&w,c"BEGIN IMMEDIATE",|db,c|{
            if self.state(db,&c)?!=pending{return Err(busy("SAVE inputs or authority changed before confirmation"));}
            alive()?;let u=db.prepare(c"UPDATE main.operation SET state=1 WHERE op_id=?1 AND kind=10 AND state=0 RETURNING op_id")?;
            u.bind_bytes(1,&op.0)?;match unsafe{sqlite3_step(u.0)}{
                100 if u.array::<16>(0)?==op.0=>u.done()?,
                rc=>{check(rc)?;return Err(invalid("SAVE did not confirm its original row"));}
            }
            Ok(Receipt::from(prepared))
        });
        drop(w);
        match out{Ok(r)=>Ok(r),Err(err)=>match self.receipt(op){
            Ok(Some(r))if r==Receipt::from(prepared)=>Ok(r),Ok(_)=>Err(err),
            Err(_)=>Err(io::Error::other("SAVE outcome unconfirmed; inspect the exact original")),
        }}
    }
}
