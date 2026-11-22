//! Fixed public comparison facts for the *one* pair of same-ELF roles.
//! A digest, a deadline, or an FD description is never local parent proof.
use super::{Fault,Result,pair_gate::{GateStamp,MAX_PAIR_WAIT}};
use crate::{Entity,hash::Sha256,system::reader::smb::runtime::PreflightEpoch};
use std::{ffi::c_int,time::{Duration,Instant}};

unsafe extern "C"{
    fn zxs_monotonic_sample(seconds:*mut i64,nanos:*mut i64)->c_int;
}

pub(super) const WIRE_BYTES:usize=120;
#[derive(Clone,Copy,Debug,Eq,PartialEq)]
pub(super) struct PairOffer{
    pub(super) nonce:[u8;32],pub(super) cohort:[u8;32],pub(super) gate:GateStamp,
    pub(super) until_tick:u64,
}
impl PairOffer{
    pub(super) fn encode(&self)->[u8;WIRE_BYTES]{
        let mut b=[0;WIRE_BYTES];
        b[..32].copy_from_slice(&self.nonce);b[32..64].copy_from_slice(&self.cohort);
        b[64..72].copy_from_slice(&self.gate.dev.to_be_bytes());
        b[72..80].copy_from_slice(&self.gate.ino.to_be_bytes());
        b[80..112].copy_from_slice(&self.gate.digest);
        b[112..].copy_from_slice(&self.until_tick.to_be_bytes());b
    }
    pub(super) fn decode(b:[u8;WIRE_BYTES])->Result<Self>{
        let word=|i:usize|u64::from_be_bytes(b[i..i+8].try_into().expect("fixed pair word"));
        let v=Self{nonce:b[..32].try_into().expect("fixed pair nonce"),
            cohort:b[32..64].try_into().expect("fixed pair scope"),
            gate:GateStamp{dev:word(64),ino:word(72),digest:b[80..112].try_into().expect("fixed gate SHA")},
            until_tick:word(112)};
        if v.gate.ino==0||v.until_tick==0||[v.nonce,v.cohort,v.gate.digest].contains(&[0;32]){
            return Err(Fault::Shape);
        }
        Ok(v)
    }
    /// The Linux roles share the same monotonic time namespace. The
    /// anchored Instant is captured *before* reading it, so conversion
    /// shortens rather than extends the one supervisor deadline.
    pub(super) fn until(&self)->Result<Instant>{
        let anchor=Instant::now();
        let left=Duration::from_nanos(self.until_tick.checked_sub(ticks()?).ok_or(Fault::Handoff)?);
        if left.is_zero()||left>MAX_PAIR_WAIT{return Err(Fault::Handoff);}
        anchor.checked_add(left).ok_or(Fault::Handoff)
    }
}
fn ticks()->Result<u64>{
    let (mut seconds,mut ns)=(0,0);
    // SAFETY: one local CLOCK_MONOTONIC sample; the two writable outputs are distinct.
    if unsafe{zxs_monotonic_sample(&mut seconds,&mut ns)}!=0{return Err(Fault::Handoff);}
    let seconds=u64::try_from(seconds).map_err(|_|Fault::Handoff)?;
    let ns=u64::try_from(ns).ok().filter(|&n|n<1_000_000_000).ok_or(Fault::Handoff)?;
    seconds.checked_mul(1_000_000_000).and_then(|s|s.checked_add(ns)).ok_or(Fault::Handoff)
}
#[cfg(all(target_arch="x86_64",target_pointer_width="64"))]
pub(super) fn new_deadline()->Result<u64>{
    ticks()?.checked_add(MAX_PAIR_WAIT.as_nanos() as u64).ok_or(Fault::Handoff)
}
pub(super) fn digest(release:[u8;32],installed:[u8;32],source:Entity,e:&PreflightEpoch)->Result<[u8;32]>{
    if [release,installed].contains(&[0;32])||source.inode==0||[e.account,e.op,e.record].contains(&[0;16])||
        e.generation<=0||crate::password::login_name(e.login.as_bytes()).is_err(){return Err(Fault::Shape);}
    let mut s=Sha256::new();
    let mut put=|part:&[u8]|s.update(part).map_err(|_|Fault::Shape);
    put(b"ZXS-SHARED-ADMISSION-1\0")?;
    put(&release)?;put(&installed)?;
    put(&source.device.to_be_bytes())?;put(&source.inode.to_be_bytes())?;
    for id in [e.account,e.op,e.record]{put(&id)?;}
    put(&e.generation.to_be_bytes())?;put(&e.sid)?;
    put(&[e.login.len() as u8])?;put(e.login.as_bytes())?;
    let stamp=s.finish();
    if stamp==[0;32]{return Err(Fault::Shape);}
    Ok(stamp)
}

#[cfg(test)]
pub(super) fn fixture()->PairOffer{
    PairOffer{nonce:[61;32],cohort:[62;32],gate:GateStamp{dev:33,ino:44,digest:[63;32]},until_tick:0x102030405060708}
}
#[cfg(test)]
mod tests{
    use super::*;
    #[test]
    fn paired_offer_only_fixed_shape_never_interprets_reserved_modes(){
        let v=fixture();
        assert_eq!(PairOffer::decode(v.encode()),Ok(v));
        for (start,len) in [(0,32),(32,32),(72,8),(80,32),(112,8)]{
            let mut b=v.encode();b[start..start+len].fill(0);
            assert!(PairOffer::decode(b).is_err());
        }
    }
    #[test]
    fn paired_epoch_digest_changes_for_source_scope_and_authenticated_generation(){
        let mut e=PreflightEpoch{account:[9;16],op:[10;16],record:[11;16],sid:[12;28],generation:1,login:"fixture".to_owned()};
        let source=Entity{device:22,inode:23};
        let old=digest([24;32],[25;32],source,&e).unwrap();
        assert_ne!(digest([24;32],[26;32],source,&e).unwrap(),old);
        assert_ne!(digest([24;32],[25;32],Entity{inode:28,..source},&e).unwrap(),old);
        e.generation=2;
        assert_ne!(digest([24;32],[25;32],source,&e).unwrap(),old);
    }
}
