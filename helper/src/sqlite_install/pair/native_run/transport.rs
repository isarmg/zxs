//! Dedicated native-maintenance control, not an externally callable service.
//! Every receive is from the other still-live process of this one invocation,
//! with SCM_CREDENTIALS and exactly the descriptors fixed by this stage.
use crate::sqlite_install::bad;
use rustix::{fd::{AsFd,OwnedFd},net::{AddressFamily,RecvAncillaryBuffer,RecvAncillaryMessage,RecvFlags,ReturnFlags,
    SendAncillaryBuffer,SendAncillaryMessage,SendFlags,SocketFlags,SocketType,UCred,recvmsg,sendmsg,socketpair,sockopt}};
use std::{fs::File,io::{self,IoSlice,IoSliceMut},mem::MaybeUninit,time::{Duration,Instant}};
const MAGIC:&[u8;8]=b"ZXSAMW1\0";
const WIRE:usize=8+1+32+32+65;
pub(super) type Payload=[u8;65];

#[derive(Clone,Copy)]
pub(super) enum Kind{Inspect=1,Act=2,Reserved=3,NativeSynced=4,Complete=5,Lease=6,Warm=7,Prepared=8}

pub(super) struct Channel {fd:File,peer:UCred,scope:[u8;32]}
pub(super) fn pair()->io::Result<(File,File)>{
    let (a,b)=socketpair(AddressFamily::UNIX,SocketType::SEQPACKET,SocketFlags::CLOEXEC,None)?;
    for f in [&a,&b]{sockopt::set_socket_passcred(f,true)?;}
    Ok((File::from(a),File::from(b)))
}
impl Channel {
    pub(super) fn from_owned_child_session(fd:File,peer:UCred,scope:[u8;32])->io::Result<Self>{
        // Caller verifies actual supervising spawn, user/subject and startup
        // challenge before using this transport. SO_PEERCRED alone is only
        // the original socketpair creator, not this packet's sender.
        if !sockopt::socket_passcred(&fd)?||sockopt::socket_domain(&fd)?!=AddressFamily::UNIX
            ||sockopt::socket_type(&fd)?!=SocketType::SEQPACKET||scope==[0;32] {
            return Err(bad("not the dedicated account-maintenance peer"));
        }Ok(Self{fd,peer,scope})
    }
    fn send_inner(&self,kind:Kind,digest:&[u8;32],body:&Payload,fds:&[&File])->io::Result<()>{
        if fds.len()>1{return Err(bad("invalid fixed maintenance descriptor count"));}
        let tag=[kind as u8];
        let parts=[IoSlice::new(MAGIC),IoSlice::new(&tag),IoSlice::new(&self.scope),IoSlice::new(digest),IoSlice::new(body)];
        let refs=fds.iter().map(|f|f.as_fd()).collect::<Vec<_>>();
        let mut ancillary=[MaybeUninit::uninit();rustix::cmsg_space!(ScmRights(1))];
        let mut control=SendAncillaryBuffer::new(&mut ancillary);
        if !refs.is_empty()&&!control.push(SendAncillaryMessage::ScmRights(&refs)){
            return Err(bad("cannot attach the original maintenance descriptor"));
        }
        if sendmsg(&self.fd,&parts,&mut control,SendFlags::DONTWAIT|SendFlags::NOSIGNAL)?!=WIRE{
            return Err(bad("incomplete maintenance message"));
        }Ok(())
    }
    pub(super) fn send(&self,kind:Kind,digest:&[u8;32],body:&Payload)->io::Result<()>{self.send_inner(kind,digest,body,&[])}
    pub(super) fn send_original(&self,kind:Kind,digest:&[u8;32],body:&Payload,original:&File)->io::Result<()>{
        self.send_inner(kind,digest,body,&[original])
    }
    fn receive<const K:usize>(&self,kind:Kind,digest:&[u8;32])->io::Result<(Payload,[File;K])>{
        let mut wire=[0;WIRE];
        let mut space=[MaybeUninit::uninit();rustix::cmsg_space!(ScmRights(2),ScmCredentials(1))];
        let mut ancillary=RecvAncillaryBuffer::new(&mut space);
        let msg=recvmsg(&self.fd,&mut [IoSliceMut::new(&mut wire)],&mut ancillary,
            RecvFlags::CMSG_CLOEXEC|RecvFlags::DONTWAIT)?;
        let mut credentials=None;
        let mut rights=Vec::<OwnedFd>::new();let mut submitted=false;
        for c in ancillary.drain(){match c{
            RecvAncillaryMessage::ScmCredentials(who) if credentials.is_none()=>credentials=Some(who),
            RecvAncillaryMessage::ScmRights(fds) if !submitted=>{submitted=true;rights.extend(fds);}
            _=>return Err(bad("wrong maintenance ancillary envelope")),
        }}
        let who=credentials.is_some_and(|v|v.pid==self.peer.pid&&v.uid==self.peer.uid&&v.gid==self.peer.gid);
        if !who||msg.bytes!=WIRE||msg.flags.intersects(ReturnFlags::TRUNC|ReturnFlags::CTRUNC)
            ||&wire[..8]!=MAGIC||wire[8]!=kind as u8||wire[9..41]!=self.scope||wire[41..73]!=*digest{
            return Err(bad("wrong, closed or stale offline maintenance peer"));
        }
        let fds:[OwnedFd;K]=rights.try_into().map_err(|_|bad("wrong maintenance descriptor count"))?;
        Ok((wire[73..].try_into().expect("fixed payload"),fds.map(File::from)))
    }
    pub(super) fn wait<const K:usize>(&self,kind:Kind,digest:&[u8;32],until:Instant)->io::Result<(Payload,[File;K])>{
        self.wait_checked(kind,digest,until,||Ok(()))
    }
    pub(super) fn wait_checked<const K:usize>(&self,kind:Kind,digest:&[u8;32],until:Instant,
        check:impl Fn()->io::Result<()>)->io::Result<(Payload,[File;K])>{
        loop {
            check()?;
            if Instant::now()>=until{return Err(io::Error::new(io::ErrorKind::TimedOut,"offline credential worker stopped responding"));}
            match self.receive(kind,digest){
                Err(e) if e.kind()==io::ErrorKind::WouldBlock=>std::thread::sleep(Duration::from_millis(20)),
                Err(e) if e.kind()==io::ErrorKind::Interrupted=>continue,r=>return r,
            }
        }
    }
    pub(super) fn eof(&self)->io::Result<()>{
        let mut buffer=[0];let mut ctl=[MaybeUninit::uninit();rustix::cmsg_space!(ScmRights(1),ScmCredentials(1))];
        let mut extras=RecvAncillaryBuffer::new(&mut ctl);
        let msg=recvmsg(&self.fd,&mut [IoSliceMut::new(&mut buffer)],&mut extras,RecvFlags::DONTWAIT|RecvFlags::CMSG_CLOEXEC)?;
        if msg.bytes!=0||msg.flags.intersects(ReturnFlags::TRUNC|ReturnFlags::CTRUNC)||extras.drain().next().is_some(){
            return Err(bad("offline credential worker did not close its exact result channel"));
        }Ok(())
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    use rustix::process::{getuid,getgid,getpid,Uid};
    #[test]fn a_complete_scope_operation_sender_and_stage_are_required(){
        let (a,b)=pair().unwrap();
        let c=UCred{uid:getuid(),gid:getgid(),pid:getpid()};
        let root=Channel::from_owned_child_session(a,c,[8;32]).unwrap();
        let worker=Channel::from_owned_child_session(b,c,[8;32]).unwrap();
        let data=[5;65];let digest=[4;32];
        root.send(Kind::Reserved,&digest,&data).unwrap();
        assert_eq!(worker.receive::<0>(Kind::Reserved,&digest).unwrap().0,data);
        root.send(Kind::Act,&digest,&data).unwrap();
        assert!(worker.receive::<0>(Kind::Reserved,&digest).is_err());
        root.send(Kind::Reserved,&[7;32],&data).unwrap();
        assert!(worker.receive::<0>(Kind::Reserved,&digest).is_err());
        let mut wrong=c;wrong.uid=Uid::from_raw(if c.uid.is_root(){1}else{0});
        let worker=Channel::from_owned_child_session(worker.fd,wrong,[8;32]).unwrap();
        root.send(Kind::Reserved,&digest,&data).unwrap();
        assert!(worker.receive::<0>(Kind::Reserved,&digest).is_err());
        drop(root);worker.eof().unwrap();
    }
}
