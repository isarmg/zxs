//! Preparation only, for one original signed ordinary FILE move. No call
//! to copy, select, rebind or reply exists until the paid original can be
//! suspended on its connection and given a real cancellation/decision gate.
use super::{DirectoryRoute,FreshOrdinaryOp,Handoff,OrdinaryMoveSource,Route,Setting,TransferAt,
    TransferCut,TransferStart,Destination,work_record};
use crate::{OperationId,display,identity::AccountId,smb::OrdinaryMoveAdmission,
    system::reader::smb::{denied,first_work::RustReadFile,runtime::RustRoot}};
use std::io;

#[derive(Clone,Copy,PartialEq,Eq)]
struct Actor{account:AccountId,op:OperationId,epoch:i64}
struct Endpoint{label:String,setting:Setting,route:DirectoryRoute}
impl Endpoint{
    fn require_at(&self,tree:&RustRoot,cut:&TransferCut<'_>,actor:Actor)->io::Result<()>{
        let (account,op,epoch,setting)=tree.ordinary_transfer_at(cut,&self.label)?;
        let (_,db,c)=cut.parts();
        if (Actor{account,op,epoch})!=actor||setting!=self.setting||
            DirectoryRoute::at(db,c,setting.volume,self.route.parent())?!=self.route{return Err(denied());}
        Ok(())
    }
}

/// Exact source OPEN material and both selected Core grants, and a fresh
/// one-use engine start. Neither a durable reservation nor a success receipt.
#[must_use]
#[allow(dead_code,reason="pending original connection and target rebind are not routed")]
pub(crate) struct RustOrdinaryMove{
    opened:OrdinaryMoveSource,source_label:String,target_label:String,request:Handoff,start:TransferStart,
}

impl RustRoot{
    pub(crate) fn bind_ordinary_move(&self,held:&RustReadFile,admission:&OrdinaryMoveAdmission<'_>)
        ->io::Result<RustOrdinaryMove>{
        let (member,destination_label,parents,leaf)=admission.held(self.pool(),held.file())?;
        display::component(leaf.as_bytes(),222)?;
        let reader=self.ordinary_reader(member)?;
        let (source_label,old_route,birth,opened)=held.ordinary_move_source(self,member)?;
        let request=Handoff::MoveOrdinary{source:opened.place.clone(),target:text(leaf)?};
        let op=OperationId::new()?;
        if op.0==[0;16]||op.0==[0xff;16]{return Err(io::ErrorKind::WouldBlock.into());}
        let (from,to,selected)=reader.at(|db,c|{
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
                from.require_at(self,cut,actor)?;
                to.require_at(self,cut,actor)
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
            Ok((from,to,selected))
        })?;
        self.ordinary_reader(member)?;
        let start=TransferStart::Fresh(FreshOrdinaryOp{op,selected,from:from.route,to:to.route});
        Ok(RustOrdinaryMove{opened,source_label:from.label,target_label:to.label,request,start})
    }
}

fn text(s:&str)->io::Result<String>{
    let mut out=String::new();
    out.try_reserve_exact(s.len()).map_err(|_|io::ErrorKind::OutOfMemory)?;
    out.push_str(s);Ok(out)
}
