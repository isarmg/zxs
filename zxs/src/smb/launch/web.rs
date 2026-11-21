//! The existing Web admin in its own lowered process of the same ZXS ELF.
//! Its only new data is the original HTTP listener cookie and SYSTEM mount
//! mode. The site, S, plan and subject are exactly the earlier approved cut.
#![allow(dead_code)] // The matching parent owns activation, not the parsed frame.
use super::{Fault,Result,entry::InstalledTerm,low::{FRAME,PreflightQuestion,QUESTION_MAX,require_reduced},
    cohort::{PairOffer,WIRE_BYTES},pair_gate::WaitingGate,transport::InheritedControl};
use crate::hash::digest;
use std::{io,time::{Duration,Instant}};

pub(super) const START:&[u8;16]=b"ZXS-WEB/START-2\0";
pub(super) const PREPARED:&[u8;16]=b"ZXS-WEB/PREP-02\0";
const BODY:&[u8]=b"ZXS-WEB/ENV-2\0";
const FIXED:usize=32+8+1+WIRE_BYTES;
pub(super) const MAX:usize=BODY.len()+FIXED+QUESTION_MAX;
const _:()=assert!(MAX<300*1024);

pub(super) struct WebStart{
    original:PreflightQuestion,
    prepared:[u8;FRAME],
    pair:PairOffer,
    read_only:bool,
    cookie:u64,
}

/// Only the fixed Web role may consume the inherited descriptors. It
/// takes no command/environment configuration, vault, SMB port, or R.
pub(super) unsafe fn enter(term:InstalledTerm)->Result<()>{
    if term.stopping(){return Err(Fault::Handoff);}
    let until=Instant::now().checked_add(Duration::from_secs(10)).ok_or(Fault::Handoff)?;
    // SAFETY: the dedicated argv0 path has sole early ownership of fd3/fd4.
    let inherited=unsafe{InheritedControl::inherit()}?;
    let (verified,frame,rights)=inherited.recv_web(until)?;
    require_reduced(verified.subject())?;
    let live=||->io::Result<()>{
        if term.stopping()||Instant::now()>=until{return Err(io::ErrorKind::PermissionDenied.into());}
        verified.require_live().map_err(|_|io::ErrorKind::PermissionDenied.into())
    };
    live().map_err(|_|Fault::Handoff)?;
    let (public,original_listener,gate)=rights.only_web::<MAX>()?;
    let boot=WebStart::decode(frame,&public,verified.subject())?;
    let listener=original_listener.match_cookie(boot.cookie)?;
    let until_pair=boot.pair.until()?;
    let waiting=WaitingGate::from_start(gate,boot.pair.gate,boot.pair.nonce,boot.pair.cohort)?;
    let (source,epoch)=boot.original.web_reader(boot.read_only,live)?;
    if boot.original.cohort_for(&epoch)?!=boot.pair.cohort{return Err(Fault::Handoff);}
    live().map_err(|_|Fault::Handoff)?;
    // The already held reader and sole original listener stay the same.
    // The HTTP accept/dispatch starts only after the authenticated original
    // parent has set the shared terminal seal, not after our PREPARED alone.
    let stopping=&term;
    crate::management::serve_ready_original(&source,&listener,move||{
        let denied=||io::Error::from(io::ErrorKind::PermissionDenied);
        if stopping.stopping(){return Err(denied());}
        let serving=verified.publish_web_prepared(&boot.prepared,until).map_err(|_|denied())?;
        waiting.wait(until_pair,||{
            if stopping.stopping(){return Err(Fault::Handoff);}
            serving.require_live()
        }).map_err(|_|denied())?;
        Ok(move||stopping.stopping()||serving.require_live().is_err())
    }).map_err(|_|Fault::Handoff)?;
    if term.stopping(){Ok(())}else{Err(Fault::Handoff)}
}

fn bound(nonce:[u8;32],raw:&[u8])->Result<[u8;FRAME]>{
    if nonce==[0;32]||raw.len()>MAX{return Err(Fault::Shape);}
    let sum=digest(raw).map_err(|_|Fault::Shape)?;
    if sum==[0;32]{return Err(Fault::Shape);}
    let mut frame=[0;FRAME];
    frame[..16].copy_from_slice(START);
    frame[16..48].copy_from_slice(&nonce);
    frame[48..].copy_from_slice(&sum);
    Ok(frame)
}

/// Parent-side shape only; its caller must prove the real original site,
/// parent, projected S and actual Web socket before sending this envelope.
pub(super) fn encode(original:&PreflightQuestion,nonce:[u8;32],cookie:u64,read_only:bool,pair:PairOffer)
    ->Result<([u8;FRAME],Vec<u8>)>{
    if cookie==0||nonce==original.start()[16..48]||nonce==pair.nonce{return Err(Fault::Shape);}
    let total=BODY.len()+FIXED+original.bytes().len();
    if total>MAX{return Err(Fault::Shape);}
    let mut bytes=Vec::new();
    bytes.try_reserve_exact(total).map_err(|_|Fault::Resources)?;
    bytes.extend_from_slice(BODY);
    bytes.extend_from_slice(&nonce);
    bytes.extend_from_slice(&cookie.to_be_bytes());
    bytes.push(u8::from(read_only));
    bytes.extend_from_slice(&pair.encode());
    bytes.extend_from_slice(original.bytes());
    Ok((bound(nonce,&bytes)?,bytes))
}
impl WebStart{
    /// Only call after the kernel authenticated that same original parent
    /// and produced exactly two expected FDs. This parser grants nothing.
    pub(super) fn decode(frame:[u8;FRAME],raw:&[u8],subject:(u32,u32))->Result<Self>{
        let raw_body=raw.strip_prefix(BODY).ok_or(Fault::Shape)?;
        let (head,source)=raw_body.split_at_checked(FIXED).ok_or(Fault::Shape)?;
        let nonce=head[..32].try_into().expect("checked fixed Web prefix");
        if frame!=bound(nonce,raw)?{return Err(Fault::Handoff);}
        let cookie=u64::from_be_bytes(head[32..40].try_into().expect("checked fixed Web cookie"));
        let read_only=match head[40]{0=>false,1=>true,_=>return Err(Fault::Shape)};
        if cookie==0{return Err(Fault::Shape);}
        let pair=PairOffer::decode(head[41..].try_into().map_err(|_|Fault::Shape)?)?;
        let original=PreflightQuestion::web_scope(source.to_vec(),subject)?;
        if nonce==original.start()[16..48]||nonce==pair.nonce{return Err(Fault::Shape);}
        let mut prepared=frame;prepared[..16].copy_from_slice(PREPARED);
        Ok(Self{original,prepared,pair,cookie,read_only})
    }
}

#[cfg(test)]
mod tests{
    use super::*;
    use super::super::low::tests::question;
    #[test]
    fn web_boot_requires_the_same_typed_original_and_distinct_exact_listener_context(){
        let origin=question();
        let (mut packet,mut bytes)=encode(&origin,[33;32],42,true,super::super::cohort::fixture()).unwrap();
        let selected=WebStart::decode(packet,&bytes,origin.subject()).unwrap();
        assert_eq!(selected.original.bytes(),origin.bytes());
        assert_eq!((selected.cookie,selected.read_only),(42,true));
        assert_eq!(&selected.prepared[..16],PREPARED);
        assert_eq!(&selected.prepared[16..],&packet[16..]);
        assert!(WebStart::decode(origin.start(),&bytes,origin.subject()).is_err());
        assert!(WebStart::decode(packet,&bytes,(origin.subject().0,0)).is_err());
        assert!(encode(&origin,[33;32],0,false,super::super::cohort::fixture()).is_err());
        assert!(WebStart::decode(packet,&bytes[..bytes.len()-1],origin.subject()).is_err());
        // Even if the parent accidentally hashes an unknown projection mode
        // or dormant tail, the corresponding parsed request cannot run.
        bytes[BODY.len()+40]=2;packet=bound([33;32],&bytes).unwrap();
        assert!(WebStart::decode(packet,&bytes,origin.subject()).is_err());
        bytes[BODY.len()+40]=0;bytes.push(0);packet=bound([33;32],&bytes).unwrap();
        assert!(WebStart::decode(packet,&bytes,origin.subject()).is_err());
    }
}
