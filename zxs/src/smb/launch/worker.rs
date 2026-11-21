//! One low-privilege runtime. Only fresh unread sockets cross to a bounded
//! lane; every negotiation and all of that lane's live authority stay there.
use super::{Fault,InheritedRun,OwnedAccepted,Result,entry::{InstalledTerm,stopping}};
use crate::{
    smb::{
        connection::{self,accepted::{Peer,incoming::{Outstanding,Turn}},
            sessions::{RunningServerGuid,ordinary::Admission}},
        session::ChallengeService,
    },
    system::reader::smb::runtime::QualifiedRustInstall,
};
use std::{rc::Rc,sync::Arc};

#[must_use]
pub(super) struct Worker {
    server:Arc<RunningServerGuid>,
    selected:Rc<QualifiedRustInstall>,
    original:Arc<InheritedRun>,
}
#[must_use]
pub(super) struct Lane {
    original:Arc<InheritedRun>,server:Arc<RunningServerGuid>,
}
impl Worker {
    pub(super) fn prepare(original:Arc<InheritedRun>)->Result<Self>{
        let selected=Rc::new(QualifiedRustInstall::attach(Arc::clone(&original)).map_err(|_|Fault::Handoff)?);
        let verifier=ChallengeService::from_qualified(&original,&selected).map_err(|_|Fault::Handoff)?;
        let server=Arc::new(RunningServerGuid::from_start(&original,verifier).map_err(|_|Fault::Handoff)?);
        Ok(Self{original,selected,server})
    }
    pub(super) fn ready(&self,term:&InstalledTerm)->Result<()>{
        self.original.publish_ready(&self.selected,&self.server,term)
    }
    pub(super) fn first_ready(&self,from:usize,have_room:bool)->Result<Option<usize>>{
        self.original.next_listener(from,have_room)
    }
    pub(super) fn accept(&self,index:usize)->Result<Option<(OwnedAccepted,Lane)>>{
        if stopping(){return Ok(None);}
        let accepted=self.original.accept_original(&self.selected,index).map_err(|_|Fault::Handoff)?;
        Ok(accepted.map(|a|(a,Lane{original:Arc::clone(&self.original),server:Arc::clone(&self.server)})))
    }
}
impl Lane {
    pub(super) fn serve(self,accepted:OwnedAccepted)->Result<()>{
        if stopping(){return Ok(());}
        // A SQLite reader never crosses threads. The lane independently
        // qualifies the same attested Arc and holds its own short cuts.
        let selected=Rc::new(QualifiedRustInstall::attach(Arc::clone(&self.original)).map_err(|_|Fault::Handoff)?);
        let mut peer=match self.server.negotiate_accepted(accepted,&selected){
            Ok(peer)=>peer,Err(connection::Fault::NoVerifier)=>return Err(Fault::Handoff),Err(_)=>return Ok(()),
        };
        while !stopping(){
            self.original.require_live_source().map_err(|_|Fault::Handoff)?;
            if turn(&selected,&mut peer).is_err(){break;} // isolated client fault
        }
        drop(peer);Ok(())
    }
}
fn turn(selected:&Rc<QualifiedRustInstall>,peer:&mut Peer)->connection::Result<Turn>{
    peer.poll(|work|match work{
        Outstanding::Ordinary(Admission::Ready(mut batch))=>{
            if batch.try_send_qualified_tree(Rc::clone(selected))?.is_some()||
                batch.try_send_root_probe()?||batch.try_send_root_open_close_probe()?||batch.try_send_related_browse()?||
                batch.try_send_related_rename()?||batch.try_send_related_disposition()?||
                batch.try_send_open_close()?||batch.try_send_file_read()?||batch.try_send_named_rename()?||batch.try_send_file_write()?||batch.try_send_root_query()?||
                batch.try_send_open_info()?||batch.try_send_exit()?{Ok(())}
            else{Err(connection::Fault::WrongReply)}
        },
        _=>Err(connection::Fault::WrongReply),
    })
}
