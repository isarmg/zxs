//! Startup pairing for the dedicated smbd and the non-root storage service.
//! The reply contains only nonsecret original identity. It cannot construct
//! SmbSession, publish a share, validate a password or open a FILE.
use super::{Credential,InitialReader,denied};
use crate::{OperationId,PoolId,fs::invalid};
use std::{fs::File,io,os::{fd::{AsRawFd,FromRawFd,OwnedFd},unix::fs::{FileExt,MetadataExt}},path::Path};

const BIND:&[u8;16]=b"ZXS-SAM/BOUND-1\0";
const READY:&[u8;16]=b"ZXS-SAM/READY-1\0";
const DOMAIN:[u8;12]=[1,4,0,0,0,0,0,5,21,0,0,0];

unsafe extern "C"{
    fn zxs_smb_ctl_claim(fd:i32)->i32;
    fn zxs_smb_bound_fd(fd:i32)->i32;
    #[cfg(test)] fn zxs_smb_ctl_read(fd:i32,p:*mut u8,n:usize)->i32;
    #[cfg(test)] fn zxs_smb_ctl_hello(fd:i32,p:*const u8)->i32;
    fn zxs_smb_ctl_bound(fd:i32,p:*mut u8)->i32;
    fn zxs_smb_ctl_write(fd:i32,p:*const u8,n:usize)->i32;
    fn zxs_smb_ctl_live(fd:i32)->i32;
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
fn answer(src:&InitialReader,e:&Expected,host:&[u8;128],observed:&[u8;128])->io::Result<[u8;212]>{
    if host!=observed{return Err(denied());}
    let domain=e.checked(host)?;
    let cred=src.at(|db,c|{
        if c.birth.pool!=e.pool{return Err(denied());}Credential::current(db,&c)
    })?;
    let mut user=[0;28];user[..24].copy_from_slice(&domain);user[1]=5;
    user[24..].copy_from_slice(&1000u32.to_le_bytes());
    if cred.sid!=user{return Err(denied());}
    let mut out=[0;212];out[..16].copy_from_slice(READY);out[16..144].copy_from_slice(host);
    out[144..160].copy_from_slice(&cred.account.0);out[160..176].copy_from_slice(&cred.op.0);
    out[176..184].copy_from_slice(&cred.generation.to_be_bytes());out[184..].copy_from_slice(&cred.sid);
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
/// Both descriptors come from the trusted launcher. The original BOUND fd is
/// independent of smbd's check of the live config and the native SAM. This is
/// a service-start operation: no arbitrary client connector or auth result.
pub fn serve(system:&Path,pool:PoolId,instance:&str,provision:OperationId,config:&str,
    endpoint:OwnedFd,binding:File,stopping:impl Fn()->bool)->io::Result<()>{
    let fd=endpoint.as_raw_fd();
    os(unsafe{zxs_smb_ctl_claim(fd)})?;
    let e=Expected{pool,instance:hex(instance)?,provision,config:hex(config)?};
    let (host,stamp)=original(&binding)?;e.checked(&host)?;
    let src=InitialReader::open(system,pool)?; // original S SH through close
    let result=(||{
        let life=exchange(fd,&src,&e,&host,||{
            if original(&binding)?!=(host,stamp)||stopping(){return Err(denied());}Ok(())
        })?; // never an NT secret or password response
        let mut draining=false;
        loop{
            if !draining&&(stopping()||unsafe{zxs_smb_ctl_live(fd)}!=0){
                // All smbd peers see revocation; its idle parent's tevent
                // watcher follows the normal clean exit. A half-close can
                // never stand in for release of every inherited writer.
                let _=unsafe{zxs_smb_ctl_drain(fd)};draining=true;
            }
            match unsafe{zxs_smb_ctl_life(life.as_raw_fd(),500)}{
                1=>break,0=>(),_=>return Err(io::Error::last_os_error())
            }
        }
        Ok(())
    })();
    drop(endpoint); // revoke the smbd end before releasing the original S, on errors too
    drop(src);result
}
#[cfg(test)]
mod tests{
    use super::*;
    use crate::{system::reader::web_pool,test_support::PrivateTemp};
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
        let mut actual=[0u8;212];os(unsafe{zxs_smb_ctl_read(yfd,actual.as_mut_ptr(),actual.len())}).unwrap();
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
