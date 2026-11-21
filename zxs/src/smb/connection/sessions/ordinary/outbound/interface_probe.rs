//! Optional SMB 3.x interface probe on the original tree. The only
//! advertised capability is LARGE_MTU, not MULTI_CHANNEL (MS-SMB2 3.3.5.15.11).
use super::{CheckedBatch,Connection,Fault,Payload,Result,RustSessionWitness,SyncReply,UnsignedSmall,VerifiedMember,current,
    root_io::RequestSlot};
use crate::{smb::wire::{fixed_reply::NetworkProbeFailure,message::Routing,network_probe::NetworkProbe},
    system::reader::smb::runtime::RustRootStamp};

impl CheckedBatch<'_,'_>{
    pub(in crate::smb) fn try_send_network_interface_refusal(&mut self)->Result<bool>{
        self.before_reply()?;
        let [m]=self.original.frame.messages() else{return Ok(false);};
        let Some(probe)=m.network_interface_probe() else{return Ok(false);};
        let Routing::Sync{tree}=m.header.routing else{return Ok(false);};
        let sid=self.ids.at(0).ok_or(Fault::SessionState)?;
        let (draft,pending)={
            let member=VerifiedMember{message:m,ordinal:0,session:self.owner.member_scope(sid,&self.revision)?};
            let charge_ok=match self.original.payload(0){
                Some(Payload::Covered)=>true,
                Some(Payload::InvalidParameter)=>false,
                _=>return Err(Fault::SessionState),
            };
            let (why,pending)=match self.owner.tree_table(sid)?.root(tree){
                None=>(NetworkProbeFailure::NetworkNameDeleted,None),
                Some(root)=>{
                    let why=match probe{
                        NetworkProbe::NotFsctl=>NetworkProbeFailure::NotSupported,
                        NetworkProbe::NoHandle{max_output} if charge_ok&&max_output<=member.max_transact()
                            =>NetworkProbeFailure::NotSupported,
                        NetworkProbe::InvalidFileId|NetworkProbe::NoHandle{..}=>NetworkProbeFailure::InvalidParameter,
                    };
                    let original=RequestSlot{sid,tree,mid:m.header.message};
                    let pending=PendingProbe{original,stamp:root.stamp(),scope:member.rust_session_witness()?};
                    (why,Some(pending))
                },
            };
            let draft=UnsignedSmall::network_probe_failure(&member,sid,why).map_err(|_|Fault::WrongReply)?;
            (draft,pending)
        };
        self.next=1;
        let reply=self.small_replies(std::iter::once(draft))?;
        match pending{
            Some(pending)=>reply.send_network_probe(pending)?,
            None=>reply.send()?, // exact signed missing-tree error only
        }
        Ok(true)
    }
}

struct PendingProbe{original:RequestSlot,stamp:RustRootStamp,scope:RustSessionWitness}
impl PendingProbe{
    fn check(&self,c:&Connection)->Result<()>{
        let root=self.original.root(c)?;
        if !root.matches_stamp(&self.stamp){return Err(Fault::SessionState);}
        // Original S, installed site and current credential. No OPEN lookup,
        // F/R metadata, network query, or lock held across the TCP write.
        root.ensure_current(&self.scope).map_err(|_|Fault::SessionState)
    }
}
impl SyncReply<'_>{
    fn send_network_probe(self,pending:PendingProbe)->Result<()>{
        if self.count!=1||self.ids.at(0)!=Some(pending.original.sid){return Err(Fault::WrongReply);}
        pending.check(self.tx.plan.owner)?;
        let SyncReply{tx,ids,revision,count}=self.write_original()?.0;
        tx.publish_with(|c|{current(c,ids,revision,count)?;pending.check(c)})
    }
}

#[cfg(test)]
#[path="../../../../../test/smb/interface_probe.rs"]
mod tests;
