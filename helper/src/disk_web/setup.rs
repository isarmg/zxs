//! Fixed public packaging assets only. No chain from this observation to an
//! offline caller, pool selection, approval, TTY prompt or service manager.
use super::http::Reply;
use crate::fs_tools::{RuntimeFile,hash_upto};
use sha2::{Digest,Sha256};
use std::{io::{self,Read},os::unix::fs::MetadataExt};

type Parts=&'static [&'static str];
struct Asset {path:Parts, bytes:&'static [u8]}
const DOMAIN:&[Asset]=&[
    Asset{path:&["usr","share","local-ops-helper","initial-live.conf"],
        bytes:include_bytes!("../../resources/samba-domain/initial-live.conf")},
];
const SMB:&[Asset]=&[
    Asset{path:&["usr","share","local-ops-helper","smb-host","check.py"],
        bytes:include_bytes!("../../resources/smb-host/check.py")},
    Asset{path:&["usr","share","local-ops-helper","smb-host","zxs-smb-preflight.service.in"],
        bytes:include_bytes!("../../resources/smb-host/zxs-smb-preflight.service.in")},
];
const HTTPS:&[Asset]=&[
    Asset{path:&["usr","share","local-ops-helper","web-management","nginx.conf"],
        bytes:include_bytes!("../../resources/web-management/nginx.conf")},
    Asset{path:&["usr","share","local-ops-helper","web-management","profile.py"],
        bytes:include_bytes!("../../resources/web-management/profile.py")},
    Asset{path:&["usr","share","local-ops-helper","web-management","zxs-management.socket.in"],
        bytes:include_bytes!("../../resources/web-management/zxs-management.socket.in")},
    Asset{path:&["usr","share","local-ops-helper","web-management","zxs-management.service.in"],
        bytes:include_bytes!("../../resources/web-management/zxs-management.service.in")},
    Asset{path:&["usr","share","local-ops-helper","web-management","zxs-management-https.service.in"],
        bytes:include_bytes!("../../resources/web-management/zxs-management-https.service.in")},
];
const LIMITED:u64=32<<10;

#[derive(Clone,Copy,Debug,PartialEq,Eq)]
enum Observation { Equal, Different, Unconfirmed }
impl Observation{
    fn tag(self)->&'static str{match self{
        Self::Equal=>"equal",Self::Different=>"different",Self::Unconfirmed=>"unconfirmed"
    }}
}
fn equal_bytes(file:&mut impl Read,length:u64,expected:&[u8])->io::Result<bool>{
    if length!=expected.len() as u64{return Ok(false);}
    // Use the original length and this guide's small cap, not a later stat
    // through the generic tool digest. A growing file stops at one tail byte.
    Ok(hash_upto(file,length,LIMITED)?==<[u8;32]>::from(Sha256::digest(expected)))
}
fn assets(set:&[Asset])->Observation{
    let checked=(||->io::Result<_>{
        let mut held=Vec::with_capacity(set.len());let mut same=true;
        for item in set{
            let mut f=RuntimeFile::open(item.path,false,LIMITED)?;
            let m=f.file.metadata()?;
            if m.mode()!=0o100444||m.uid()!=0||m.gid()!=0||m.nlink()!=1{
                return Err(io::Error::other("not the fixed public packaged template"));
            }
            if !equal_bytes(&mut f.file,m.len(),item.bytes)?{same=false;}
            held.push(f);
        }
        for f in &held{f.recheck()?;}
        Ok(if same{Observation::Equal}else{Observation::Different})
    })();
    checked.unwrap_or(Observation::Unconfirmed)
}
pub(super) fn observe()->Reply{
    let states=[DOMAIN,SMB,HTTPS].map(assets);
    // Fixed type tags, no filename, digest, system error or output from a child.
    Reply::plain(200,wire(states))
}
fn wire([sam,smb,https]:[Observation;3])->String{
    format!("helper-install-guide-1\n{}\n{}\n{}",sam.tag(),smb.tag(),https.tag())
}

#[cfg(test)]
mod tests{
    use super::*;
    #[test]fn a_growing_template_reads_at_most_the_original_length_and_one_byte(){
        let want=b"fixed template";
        let mut original=io::Cursor::new(want.as_slice());
        assert!(equal_bytes(&mut original,want.len() as u64,want).unwrap());
        let mut growing=io::Cursor::new([want.as_slice(),&[b'a';4096]].concat());
        assert!(equal_bytes(&mut growing,want.len() as u64,want).is_err());
        assert_eq!(growing.position(),want.len() as u64+1);
        let mut unexpected=io::Cursor::new(want.as_slice());
        assert!(!equal_bytes(&mut unexpected,want.len() as u64-1,want).unwrap());
        assert_eq!(unexpected.position(),0);
    }
    #[test]fn only_the_selected_passive_release_assets_can_reach_this_page(){
        assert_eq!([DOMAIN.len(),SMB.len(),HTTPS.len()],[1,2,5]);
        for a in [DOMAIN,SMB,HTTPS].into_iter().flatten(){
            assert_eq!(&a.path[..3],["usr","share","local-ops-helper"]);
            assert!(a.bytes.len() as u64<=LIMITED&&!a.bytes.is_empty());
        }
        assert_eq!(wire([Observation::Equal,Observation::Different,Observation::Unconfirmed]),
            "helper-install-guide-1\nequal\ndifferent\nunconfirmed");
    }
}
