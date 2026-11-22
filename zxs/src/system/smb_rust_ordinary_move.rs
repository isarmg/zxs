//! One original signed ordinary FILE move. This worker handoff reuses the
//! Core transfer, but cannot address a socket, reply or replace a wire OPEN.
use super::{DirectoryRoute,FreshOrdinaryOp,Handoff,OpenedOrdinaryMove,OrdinaryMoveSource,Route,
    Setting,TransferAt,TransferCut,TransferStart,Destination,work_record};
use crate::{OperationId,Role,display,identity::AccountId,smb::{OrdinaryMoveAdmission,OriginalMoveFence,RustSessionWitness},
    ordinary::portable_birth::Birth,system::reader::{file_location::Place,volume::OrdinaryMedia,
        smb::{denied,file_rename::unknown,first_work::{MoveFinalRecord,MovedFile,MoveTargetAt,PinnedMoveTarget,RustReadFile},
            runtime::{OrdinaryMoveSite,QualifiedRustInstall,RustRoot}}}};
use std::{io,sync::Arc};

#[derive(Clone,Copy,PartialEq,Eq)]
struct Actor{account:AccountId,op:OperationId,epoch:i64}
struct Endpoint{label:String,setting:Setting,route:DirectoryRoute}
impl Endpoint{
    fn require_at(&self,cut:&TransferCut<'_>,actor:Actor,current:(AccountId,OperationId,i64,Setting))->io::Result<()>{
        let (account,op,epoch,setting)=current;
        let (_,db,c)=cut.parts();
        if (Actor{account,op,epoch})!=actor||setting!=self.setting||
            DirectoryRoute::at(db,c,setting.volume,self.route.parent())?!=self.route{return Err(denied());}
        Ok(())
    }
}

struct MoveFinalBasis{
    record:MoveFinalRecord,source:Endpoint,target:Endpoint,actor:Actor,member:RustSessionWitness,
}
impl MoveFinalBasis{
    fn require_endpoints(&self,cut:&TransferCut<'_>,
        current:impl Fn(&TransferCut<'_>,&str)->io::Result<(AccountId,OperationId,i64,Setting)>)->io::Result<()>{
        self.source.require_at(cut,self.actor,current(cut,&self.source.label)?)?;
        self.target.require_at(cut,self.actor,current(cut,&self.target.label)?)
    }
    fn owner_reader<'a>(&self,tree:&'a RustRoot,member:&RustSessionWitness)->io::Result<&'a super::InitialReader>{
        if !member.is_set_info()||!member.same_session(&self.member){return Err(unknown());}
        tree.ordinary_reader(member).map_err(|_|unknown())
    }
}
/// Same allocation for worker, adopted OPEN and original wire turn. Numeric
/// operation/FILE IDs or an equal target name cannot create this identity.
#[derive(Clone)]
pub(crate) struct MoveTargetKey(Arc<MoveFinalBasis>);
impl MoveTargetKey{
    pub(crate) fn same(&self,other:&Self)->bool{Arc::ptr_eq(&self.0,&other.0)}
    pub(in crate::system::reader::smb) fn names_adopted(&self,label:&str,route:&DirectoryRoute,setting:Setting,
        place:&Place,birth:&Birth,read:Option<work_record::ReadRecord>)->bool{
        let basis=self.0.as_ref();let saved=&basis.record;
        label==basis.target.label.as_str()&&route==&basis.target.route&&setting==basis.target.setting&&
            place==&saved.result.to&&birth==&saved.birth&&read==saved.read
    }
    /// The peer's short logical cut only; it can't attest media.
    pub(in crate::system::reader::smb) fn require_adopted(&self,tree:&RustRoot,member:&RustSessionWitness,media:&OrdinaryMedia)
        ->io::Result<()>{
        let basis=self.0.as_ref();
        let reader=basis.owner_reader(tree,member)?;
        reader.at(|db,c|{
            let cut=TransferCut{source:reader,db,catalog:&c};
            basis.require_endpoints(&cut,|cut,label|tree.ordinary_transfer_at(cut,label))?;
            media.capacity_origin().require_catalog(&c,basis.target.setting.volume)?;
            basis.record.require(db,&c,basis.source.setting,basis.target.setting,basis.actor.op,basis.actor.account)
        }).map_err(|_|unknown())?;
        basis.owner_reader(tree,member).map(drop)
    }
}
/// Read-back result from this exact retained physical target. The peer
/// can pair it with the adopted OPEN but cannot manufacture one.
#[must_use]
pub(crate) struct CheckedMoveTarget(MoveTargetKey);
impl CheckedMoveTarget{
    pub(crate) fn matches(&self,key:&MoveTargetKey)->bool{self.0.same(key)}
}
/// Stays on the original mover. Its independent live SQLite reader and
/// actual duplicate FILE/parent OFDs never enter the wire owner.
#[must_use]
pub(crate) struct RustMoveTargetWorker{
    install:QualifiedRustInstall,pinned:PinnedMoveTarget,key:MoveTargetKey,
}
impl RustMoveTargetWorker{
    pub(crate) fn verify(&self,fence:&OriginalMoveFence<'_>,expected:&MoveTargetKey)->io::Result<CheckedMoveTarget>{
        if !self.key.same(expected){return Err(unknown());}
        let basis=self.key.0.as_ref();
        let install=&self.install;
        let reader=install.move_reader(&basis.member).map_err(|_|unknown())?;
        let check=||fence.check_selected_member(&basis.member,install).map_err(|_|unknown());
        check()?;
        self.pinned.check(reader,&basis.record,|db,c|{
            check()?;
            let cut=TransferCut{source:reader,db,catalog:c};
            basis.require_endpoints(&cut,|cut,label|install.move_folder_at(cut,label))?;
            basis.record.require(db,c,basis.source.setting,basis.target.setting,basis.actor.op,basis.actor.account)
        }).map_err(|_|unknown())?;
        check()?;
        Ok(CheckedMoveTarget(self.key.clone()))
    }
}

/// Exact source OPEN material and both selected Core grants, and a fresh
/// one-use engine start. Neither a durable reservation nor a success receipt.
#[must_use]
pub(crate) struct RustOrdinaryMove{
    opened:OrdinaryMoveSource,source:Endpoint,target:Endpoint,actor:Actor,
    request:Handoff,start:FreshOrdinaryOp,site:OrdinaryMoveSite,member:RustSessionWitness,
}

/// Real held target of the original confirmed move, including its
/// previously selected read view and both original authorities. Still
/// neither the rebinding of the original OPEN nor a wire completion.
#[must_use]
pub(crate) struct ConfirmedOrdinaryMove{
    ready:MovedFile,key:MoveTargetKey,
}
impl ConfirmedOrdinaryMove{
    pub(in crate::system::reader::smb) fn held_target(&self)->&MovedFile{&self.ready}
    pub(in crate::system::reader::smb) fn for_adoption(self,tree:&RustRoot,member:&RustSessionWitness,
        label:&str,route:&DirectoryRoute,grant:Setting)->io::Result<(MovedFile,String,Setting,MoveTargetKey)>{
        let basis=self.key.0.as_ref();
        if label!=basis.source.label.as_str()||route!=&basis.source.route||grant!=basis.source.setting{
            return Err(unknown());
        }
        let reader=basis.owner_reader(tree,member)?;
        reader.at(|db,c|{
            let cut=TransferCut{source:reader,db,catalog:&c};
            basis.require_endpoints(&cut,|cut,label|tree.ordinary_transfer_at(cut,label))?;
            self.ready.require_adoptable(db,&c,&basis.target.route,basis.source.setting,basis.target.setting,
                basis.actor.op,basis.actor.account)
        }).map_err(|_|unknown())?;
        basis.owner_reader(tree,member)?;
        let label=text(&basis.target.label).map_err(|_|unknown())?;
        let setting=basis.target.setting;
        Ok((self.ready,label,setting,self.key))
    }
}
impl RustOrdinaryMove{
    pub(crate) fn is_original_member(&self,member:&RustSessionWitness)->bool{
        member.is_set_info()&&self.member.same_session(member)
    }
    /// Only the original mover's private boundary may call this. The
    /// selected site and account are fixed by the former live OPEN, not
    /// by a reader, path, file identifier or credentials supplied by a worker.
    pub(crate) fn run(self,fence:&OriginalMoveFence<'_>)->io::Result<(ConfirmedOrdinaryMove,RustMoveTargetWorker)>{
        fence.check_progress()?;
        let Self{opened,source,target,actor,request,start,site,member}=self;
        let install=site.attach()?;
        let reader=install.move_reader(&member)?;
        let live=||fence.check_member(&member,&install);
        let choose=||fence.before_placement(&member,&install);
        let require=|cut:&TransferCut<'_>|{
            source.require_at(cut,actor,install.move_folder_at(cut,&source.label)?)?;
            target.require_at(cut,actor,install.move_folder_at(cut,&target.label)?)
        };
        let at=TransferAt{source:reader,setting:source.setting,parent:source.route.parent(),
            destination:Some(Destination{setting:target.setting,parent:target.route.parent()}),
            actor:actor.account,auth:actor.op,epoch:actor.epoch,require:&require};
        let Handoff::MoveOrdinary{target:leaf,..}=&request else{return Err(denied());};
        let op=start.op;
        let original=OpenedOrdinaryMove{original:&opened.physical,select:&choose};
        let moved=at.transfer_started(&request,TransferStart::Fresh(start),Some(original),live,false,|_|Ok(()))?;
        // A result can only describe the exact consumed original Op. No
        // current path lookup or later stable-looking placement qualifies.
        if moved.op!=op||moved.from!=opened.place||moved.to.op!=op||
            moved.to.file!=moved.from.file||moved.to.role!=Role::File||
            moved.to.volume!=target.setting.volume||moved.to.parent!=target.route.parent()||
            &moved.to.name!=leaf||moved.from.generation.checked_add(1)!=Some(moved.to.generation){
            return Err(io::ErrorKind::ConnectionAborted.into());
        }
        // Still on this worker's independent real S: verify and carry the
        // exact authoritative destination, not a path for the peer to reopen.
        // Any late fault is unknown to the client; selection already happened.
        let require_target=|db:&super::Connection,c:&super::Catalog|{
            fence.check_selected_member(&member,&install)?;
            require(&TransferCut{source:reader,db,catalog:c})
        };
        fence.check_selected_member(&member,&install).map_err(|_|unknown())?;
        let ready=MoveTargetAt{source:reader,setting:target.setting,parent:target.route.parent(),actor:actor.account,
            auth:actor.op,require:&require_target}.prepare(moved,&opened,Some(&target.route))?;
        fence.check_selected_member(&member,&install).map_err(|_|unknown())?;
        let (record,pinned)=ready.retain_for_terminal().map_err(|_|unknown())?;
        let key=MoveTargetKey(Arc::new(MoveFinalBasis{record,source,target,actor,member}));
        Ok((ConfirmedOrdinaryMove{ready,key:key.clone()},RustMoveTargetWorker{install,pinned,key}))
    }
}

impl RustRoot{
    pub(crate) fn bind_ordinary_move(&self,held:&RustReadFile,admission:OrdinaryMoveAdmission<'_>)
        ->io::Result<RustOrdinaryMove>{
        let (member,destination_label,parents,leaf)=admission.into_original(self.pool(),held.file())?;
        display::component(leaf.as_bytes(),222)?;
        let reader=self.ordinary_reader(&member)?;
        let (source_label,old_route,birth,opened)=held.ordinary_move_source(self,&member)?;
        let request=Handoff::MoveOrdinary{source:opened.place.clone(),target:text(leaf)?};
        let op=OperationId::new()?;
        if op.0==[0;16]||op.0==[0xff;16]{return Err(io::ErrorKind::WouldBlock.into());}
        let (from,to,actor,selected)=reader.at(|db,c|{
            let cut=TransferCut{source:reader,db,catalog:&c};
            let (account,auth,epoch,setting)=self.ordinary_transfer_at(&cut,source_label)?;
            let (other,other_auth,other_epoch,destination)=self.ordinary_transfer_at(&cut,destination_label)?;
            if (account,auth,epoch)!=(other,other_auth,other_epoch)||setting!=opened.grant||
                destination.volume==setting.volume||opened.place.parent!=old_route.parent(){return Err(denied());}
            let actor=Actor{account,op:auth,epoch};
            let from=Endpoint{label:text(source_label)?,setting,route:old_route.clone()};
            let to=Endpoint{label:text(destination_label)?,setting:destination,
                route:DirectoryRoute::by_names(db,&c,destination.volume,parents)?};
            let require=|cut:&TransferCut<'_>|{
                from.require_at(cut,actor,self.ordinary_transfer_at(cut,&from.label)?)?;
                to.require_at(cut,actor,self.ordinary_transfer_at(cut,&to.label)?)
            };
            let at=TransferAt{source:reader,setting,parent:from.route.parent(),
                destination:Some(Destination{setting:destination,parent:to.route.parent()}),
                actor:account,auth,epoch,require:&require};
            let selected=Route::select(&at,db,&c,&request)?;
            if &selected.from.birth!=birth||selected.from.here!=opened.place{return Err(denied());}
            if let Some(view)=opened.view{
                if work_record::confirmed(db,birth,view.record.key().save)?!=view{return Err(denied());}
            }
            selected.require_fresh(db,op)?;
            selected.vacant(db,None)?;
            selected.geometry(&c,op,&to.route)?;
            Ok((from,to,actor,selected))
        })?;
        let site=self.ordinary_move_site(&member)?;
        let start=FreshOrdinaryOp{op,selected,from:from.route.clone(),to:to.route.clone()};
        Ok(RustOrdinaryMove{opened,source:from,target:to,actor,request,start,site,member})
    }
}

fn text(s:&str)->io::Result<String>{
    let mut out=String::new();
    out.try_reserve_exact(s.len()).map_err(|_|io::ErrorKind::OutOfMemory)?;
    out.push_str(s);Ok(out)
}
