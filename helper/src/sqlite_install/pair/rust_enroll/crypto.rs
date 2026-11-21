//! Exact foreground-approved offline Rust vault providers.
//! No stored Helper approval, caller-named library or runtime dependency.
use crate::{fs_tools::RuntimeFile,pinned::helper_elf::ForegroundElf,sqlite_install::pair::{bad,smb_policy}};
use crate::sqlite_install::pair::fresh_issuer::{CryptoPins,compiled_v18,require_v18};
use helper_linux_evidence::first_smb::{Enrollment,Libraries};
use std::io;

const GCRYPT:&[&str]=&["usr","lib","local-ops-helper","disk-tools-v1","lib","libgcrypt.so.20"];
const GPGERR:&[&str]=&["usr","lib","local-ops-helper","disk-tools-v1","lib","libgpg-error.so.0"];
fn no()->io::Error{bad("no approved Rust SMB enrollment provider for this original site")}
pub(in crate::sqlite_install::pair) struct Cryptography{
    helper:ForegroundElf,gcrypt:RuntimeFile,gpgerr:RuntimeFile,
}
impl Cryptography{
    pub(in crate::sqlite_install::pair) fn fresh(pins:CryptoPins)->io::Result<Self>{
        require_v18()?;Self::for_deployed(pins)
    }
    pub(in crate::sqlite_install::pair) fn for_deployed(pins:CryptoPins)->io::Result<Self>{
        compiled_v18()?;pins.check()?;smb_policy::system_policy()?;
        if !smb_policy::clean_environment(){return Err(no());}
        let helper=ForegroundElf::host()?;
        helper.require_independent_digest(&pins.helper)?;
        let gcrypt=smb_policy::release(GCRYPT,0o644,&pins.gcrypt)?;
        let gpgerr=smb_policy::release(GPGERR,0o644,&pins.gpgerror)?;
        let x=Self{helper,gcrypt,gpgerr};x.require()?;Ok(x)
    }
    pub(in crate::sqlite_install::pair) fn require(&self)->io::Result<()>{
        self.helper.require()?;
        compiled_v18()?;
        self.gcrypt.recheck()?;self.gpgerr.recheck()?;smb_policy::system_policy()
    }
    pub(in crate::sqlite_install::pair) fn before_dropping(&self)->io::Result<Libraries>{
        self.require()?;
        let crypto=Libraries::load()?;
        smb_policy::mappings(&self.gcrypt.file,&self.gpgerr.file,&crypto)?;
        self.require()?;Ok(crypto)
    }
    pub(in crate::sqlite_install::pair) fn for_low_writer(&self,library:&Libraries)->io::Result<Enrollment>{
        // Both already mapped originals are checked again without reopening
        // the protected policy or exposing verifier bytes.
        smb_policy::mappings(&self.gcrypt.file,&self.gpgerr.file,library)?;
        library.qualify()
    }
}
