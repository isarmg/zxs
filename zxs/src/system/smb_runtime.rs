//! Startup pairing for the dedicated smbd and the non-root storage service.
//! The reply contains only nonsecret original identity. It cannot construct
//! SmbSession, publish a share, validate a password or open a FILE. Only a later
//! successful managed session update can promote its own original attempt.
use super::{Credential,InitialReader,denied};
use crate::{OperationId,PoolId,fs::invalid};
use std::{fs::File,io,os::{fd::{AsRawFd,FromRawFd,OwnedFd},unix::fs::{FileExt,MetadataExt}},path::Path};

#[path="smb_attempt.rs"]
mod attempts;

const BIND:&[u8;16]=b"ZXS-SAM/BOUND-1\0";
const READY:&[u8;16]=b"ZXS-SAM/READY-2\0";
const DOMAIN:[u8;12]=[1,4,0,0,0,0,0,5,21,0,0,0];

unsafe extern "C"{
    fn zxs_smb_ctl_claim(fd:i32)->i32;
    fn zxs_smb_bound_fd(fd:i32)->i32;
    fn zxs_smb_origin_enable(fd:i32)->i32;
    fn zxs_smb_process_claim(fd:i32)->i32;
    #[cfg(test)] fn zxs_smb_ctl_read(fd:i32,p:*mut u8,n:usize)->i32;
    #[cfg(test)] fn zxs_smb_ctl_hello(fd:i32,p:*const u8)->i32;
    fn zxs_smb_ctl_bound(fd:i32,p:*mut u8)->i32;
    fn zxs_smb_ctl_write(fd:i32,p:*const u8,n:usize)->i32;
    #[cfg(test)] fn zxs_smb_ctl_live(fd:i32)->i32;
    fn zxs_smb_ctl_life(fd:i32,timeout:i32)->i32;
    fn zxs_smb_ctl_drain(fd:i32)->i32;
}
fn os(rc:i32)->io::Result<()>{
    if rc==0{Ok(())}else{Err(io::Error::last_os_error())}
}
fn hex<const N:usize>(s:&str)->io::Result<[u8;N]>{
    let bad=||invalid("invalid original installation selector");
    if s.len()!=N*2{return Err(bad());}
    let nibble=|b:u8|match b{b'0'..=b'9'=>Ok(b-b'0'),b'a'..=b'f'=>Ok(b-b'a'+10),_=>Err(bad())};
    let mut out=[0;N];
    for (i,ab) in s.as_bytes().chunks_exact(2).enumerate(){out[i]=(nibble(ab[0])?<<4)|nibble(ab[1])?;}
    Ok(out)
}
struct Expected{pool:PoolId,instance:[u8;16],provision:OperationId,config:[u8;32]}
impl Expected{
    fn checked(&self,b:&[u8;128])->io::Result<[u8;24]>{
        if self.instance==[0;16]||&b[..16]!=BIND||b[16..32]!=self.pool.0||b[32..48]!=self.instance||
            b[48..64]!=self.provision.0||b[64..96]!=self.config||&b[96..104]!=b"ZXSLOCAL"||
            b[104..116]!=DOMAIN||b[116..]==[0;12]{return Err(denied());}
        Ok(b[104..].try_into().expect("24B"))
    }
}
#[derive(Eq,PartialEq)]
struct Stamp(u64,u64,u64,u32,u32,u32,u64,i64,i64,i64,i64);
impl Stamp{
    fn read(f:&File)->io::Result<Self>{
        os(unsafe{zxs_smb_bound_fd(f.as_raw_fd())})?;
        let m=f.metadata()?;
        Ok(Self(m.dev(),m.ino(),m.len(),m.mode(),m.uid(),m.gid(),m.nlink(),m.ctime(),m.ctime_nsec(),m.mtime(),m.mtime_nsec()))
    }
}
fn original(file:&File)->io::Result<([u8;128],Stamp)>{
    let old=Stamp::read(file)?;let mut b=[0u8;128];file.read_exact_at(&mut b,0)?;
    if old!=Stamp::read(file)?{return Err(denied());}Ok((b,old))
}
#[derive(Clone,Eq,PartialEq)]
struct Slot{pool:PoolId,original:Credential}
fn expected_user(domain:[u8;24])->[u8;28]{
    let mut sid=[0;28];sid[..24].copy_from_slice(&domain);sid[1]=5;
    sid[24..].copy_from_slice(&1000u32.to_le_bytes());sid
}
/// Only the nonsecret provenance of this one live pool's SMB slot.
fn current_slot(src:&InitialReader,pool:PoolId,instance:[u8;16],sid:&[u8;28])->io::Result<Slot>{
    src.at(|db,c|{
        if c.birth.pool!=pool{return Err(denied());}
        let original=Credential::current(db,&c)?;
        if original.instance!=instance||&original.sid!=sid{return Err(denied());}Ok(Slot{pool,original})
    })
}
fn answer(src:&InitialReader,e:&Expected,host:&[u8;128],observed:&[u8;128])->io::Result<[u8;277]>{
    if host!=observed{return Err(denied());}
    let slot=current_slot(src,e.pool,e.instance,&expected_user(e.checked(host)?))?;
    let id=attempts::generation_bytes(&slot).ok_or_else(denied)?;
    let mut out=[0;277];out[..16].copy_from_slice(READY);out[16..144].copy_from_slice(host);
    // The complete 149B original tuple is shared with the attempt's late cut.
    // Its PoolId already occupies BOUND[16..32], so it is not duplicated.
    out[144..].copy_from_slice(&id[16..]);
    Ok(out)
}
fn exchange(fd:i32,src:&InitialReader,e:&Expected,host:&[u8;128],before_send:impl FnOnce()->io::Result<()>)->io::Result<OwnedFd>{
    let mut obs=[0;128];let pipe=unsafe{zxs_smb_ctl_bound(fd,obs.as_mut_ptr())};
    if pipe<0{return Err(io::Error::last_os_error());}
    // recvmsg delivered a fresh CLOEXEC fd. C has rejected every other fd,
    // extra ancillary field, named FIFO, regular file and wrong pipe direction.
    let life=unsafe{OwnedFd::from_raw_fd(pipe)};
    let reply=answer(src,e,host,&obs)?;before_send()?;
    if unsafe{zxs_smb_ctl_life(life.as_raw_fd(),0)}!=0{return Err(denied());}
    os(unsafe{zxs_smb_ctl_write(fd,reply.as_ptr(),reply.len())})?;Ok(life)
}
/// Once HELLO supplied a real lifetime pipe, a local dispatcher error is a
/// request to stop, never evidence that every smbd writer exited. The caller
/// retains the original SYSTEM SH (and SQLite connection) until this returns.
fn until_peer_drained<T>(ctl:i32,run:impl FnOnce()->io::Result<T>,mut single:impl FnMut()->i32)->io::Result<T>{
    let result=run(); // active attempts, sessions and worker admissions are dropped on errors too
    let _=unsafe{zxs_smb_ctl_drain(ctl)};
    loop{match single(){
        1=>return result,     // ONLY the actual last writer's POLLHUP
        0=>continue,         // timed out/EINTR on a still-live original
        _=>std::thread::sleep(std::time::Duration::from_millis(100)),
        // Broken/missing anchor cannot certify normal drain; remain stopped
        // holding S for the trusted supervisor's exceptional group handling.
    }}
}
/// These descriptors come independently from the trusted launcher. The
/// process pidfd is the actual foreground smbd, not its supervisor's PID,
/// a peer-provided number, or identity inferred from the control socket.
pub fn serve(system:&Path,pool:PoolId,instance:&str,provision:OperationId,config:&str,
    endpoint:OwnedFd,binding:File,master:OwnedFd,stopping:impl Fn()->bool)->io::Result<()>{
    let fd=endpoint.as_raw_fd();
    os(unsafe{zxs_smb_ctl_claim(fd)})?;os(unsafe{zxs_smb_process_claim(master.as_raw_fd())})?;
    let e=Expected{pool,instance:hex(instance)?,provision,config:hex(config)?};
    let (host,stamp)=original(&binding)?;let sid=expected_user(e.checked(&host)?);
    let src=InitialReader::open(system,pool)?; // original S SH through close
    let result=(||{
        let life=exchange(fd,&src,&e,&host,||{
            let(seen,when)=original(&binding)?;
            if seen!=host||when!=stamp||stopping(){return Err(denied());}
            os(unsafe{zxs_smb_process_claim(master.as_raw_fd())})
        })?; // never an NT secret or password response
        // No further sends on this anonymous endpoint (shutdown is not a
        // send). Authentic role frames always supplied their own kernel-
        // checked credentials, including ones already queued on this edge.
        until_peer_drained(fd,||{
        os(unsafe{zxs_smb_origin_enable(fd)})?;
        attempts::serve(fd,&life,&master,stopping,||{
            if Stamp::read(&binding)?!=stamp{return Err(denied());}
            let selected=current_slot(&src,pool,e.instance,&sid)?;
            if Stamp::read(&binding)?!=stamp{return Err(denied());}Ok(selected)
        },|native,share|{
            if Stamp::read(&binding)?!=stamp{return Err(denied());}
            let setting=native.select(&src,share)?;
            if Stamp::read(&binding)?!=stamp{return Err(denied());}Ok(setting)
        })
        },||unsafe{zxs_smb_ctl_life(life.as_raw_fd(),500)})
    })();
    drop(endpoint); // revoke the smbd end before releasing the original S, on errors too
    drop(src);result
}
#[cfg(test)]
mod tests{
    use super::*;
    use crate::{system::reader::web_pool,test_support::PrivateTemp};
    #[test]
    #[ignore="source-only: actual private pool/pipe; inject dispatcher ENOMEM, not kernel memory pressure"]
    fn local_dispatch_failure_does_not_release_s_before_last_lifetime_writer(){
        use crate::fs::HeldDir;
        unsafe extern "C"{fn socketpair(d:i32,k:i32,p:i32,out:*mut i32)->i32;fn pipe2(f:*mut i32,flags:i32)->i32;}
        let tmp=PrivateTemp::under(&std::env::temp_dir());let(path,pool)=web_pool(&tmp);
        let src=InitialReader::open(&path,pool).unwrap();
        let other=HeldDir::trusted_system(&path).unwrap();
        let mut s=[-1;2];os(unsafe{socketpair(1,5|0o2000000,0,s.as_mut_ptr())}).unwrap();
        let (ctl,peer)=unsafe{(OwnedFd::from_raw_fd(s[0]),OwnedFd::from_raw_fd(s[1]))};
        let mut f=[-1;2];os(unsafe{pipe2(f.as_mut_ptr(),0o2000000)}).unwrap();
        let (life,w)=unsafe{(OwnedFd::from_raw_fd(f[0]),OwnedFd::from_raw_fd(f[1]))};
        let mut last=Some(w.try_clone().unwrap());drop(w);let mut turns=0;
        let result=until_peer_drained(ctl.as_raw_fd(),||Err::<(),_>(io::Error::from_raw_os_error(12)),||{
            turns+=1;assert!(other.exclusive().is_err());
            assert!(unsafe{zxs_smb_ctl_live(peer.as_raw_fd())}<0); // master revoked before retry
            if turns==1 {return -1;} // even the small poll error isn't proof of last-close
            if turns==2{
                let seen=unsafe{zxs_smb_ctl_life(life.as_raw_fd(),0)};
                assert_eq!(seen,0);drop(last.take());return seen;
            }
            assert_eq!(turns,3);unsafe{zxs_smb_ctl_life(life.as_raw_fd(),0)}
        });
        assert_eq!(turns,3);assert_eq!(result.unwrap_err().raw_os_error(),Some(12));
        drop(src);other.exclusive().unwrap();
    }
    #[test]
    #[ignore="source-only real non-root SQLite first pair, no Samba authentication"]
    fn original_pair_and_independent_installation_all_have_to_agree(){
        let tmp=PrivateTemp::under(&std::env::temp_dir());let (s,p)=web_pool(&tmp);
        let src=InitialReader::open(&s,p).unwrap();
        let e=Expected{pool:p,instance:[7;16],provision:OperationId([8;16]),config:[9;32]};
        let mut b=[0u8;128];b[..16].copy_from_slice(BIND);b[16..32].copy_from_slice(&p.0);
        b[32..48].copy_from_slice(&e.instance);b[48..64].copy_from_slice(&e.provision.0);
        b[64..96].copy_from_slice(&e.config);b[96..104].copy_from_slice(b"ZXSLOCAL");
        b[104..116].copy_from_slice(&DOMAIN);
        for (i,v) in [1u32,2,3].into_iter().enumerate(){b[116+4*i..120+4*i].copy_from_slice(&v.to_le_bytes());}
        let ack=answer(&src,&e,&b,&b).unwrap();assert_eq!(&ack[..16],READY);
        assert_eq!(&ack[144..160],&[0x56;16]);assert_eq!(&ack[160..176],&[0x44;16]);
        assert_eq!(&ack[176..184],&1i64.to_be_bytes());
        assert_eq!(ack[212],9);assert_eq!(&ack[213..222],b"files.smb");
        assert!(ack[222..].iter().all(|b|*b==0));
        // Real connected anonymous socketpair, same framing and real SQLite
        // lookup. This non-root fixture does not impersonate the privileged
        // launch/RO mount check, nor a Samba login or final session.
        unsafe extern "C"{fn socketpair(domain:i32,kind:i32,protocol:i32,ends:*mut i32)->i32;}
        let pair=||{
            let mut v=[-1;2];
            assert_eq!(unsafe{socketpair(1,5|0o2000000,0,v.as_mut_ptr())},0);
            unsafe{(OwnedFd::from_raw_fd(v[0]),OwnedFd::from_raw_fd(v[1]))}
        };
        let(x,y)=pair();let yfd=y.as_raw_fd();
        let writer=unsafe{zxs_smb_ctl_hello(yfd,b.as_ptr())};assert!(writer>=0);
        let writer=unsafe{OwnedFd::from_raw_fd(writer)};
        let inherited=writer.try_clone().unwrap(); // the same OFD retained by an auxiliary child
        let life=exchange(x.as_raw_fd(),&src,&e,&b,||Ok(())).unwrap();
        let mut actual=[0u8;277];os(unsafe{zxs_smb_ctl_read(yfd,actual.as_mut_ptr(),actual.len())}).unwrap();
        assert_eq!(actual,ack);
        os(unsafe{zxs_smb_ctl_drain(yfd)}).unwrap(); // this is not a close of all smbd writers
        assert!(unsafe{zxs_smb_ctl_live(x.as_raw_fd())}<0);
        assert_eq!(unsafe{zxs_smb_ctl_life(life.as_raw_fd(),0)},0);
        drop(y);drop(writer);assert_eq!(unsafe{zxs_smb_ctl_life(life.as_raw_fd(),0)},0);
        drop(inherited);assert_eq!(unsafe{zxs_smb_ctl_life(life.as_raw_fd(),0)},1);
        let (x,y)=pair();
        os(unsafe{zxs_smb_ctl_write(y.as_raw_fd(),b.as_ptr(),b.len())}).unwrap();
        assert!(exchange(x.as_raw_fd(),&src,&e,&b,||Ok(())).is_err()); // right bytes without the original kernel lease

        let (x,y)=pair();os(unsafe{zxs_smb_ctl_write(y.as_raw_fd(),ack.as_ptr(),ack.len())}).unwrap();
        assert!(exchange(x.as_raw_fd(),&src,&e,&b,||Ok(())).is_err()); // overlength response is not a request
        drop(x);assert!(unsafe{zxs_smb_ctl_live(y.as_raw_fd())}<0);
        for x in [0,16,32,48,64,96,104,116]{
            let mut bad=b;bad[x]^=1;assert!(answer(&src,&e,&b,&bad).is_err());
            assert!(answer(&src,&e,&bad,&bad).is_err());
        }
    }
}
