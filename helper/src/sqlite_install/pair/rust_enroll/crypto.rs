//! Exact foreground-approved offline Rust vault providers.
//! No stored Helper approval, caller-named library or runtime dependency.
use crate::{fs_tools::RuntimeFile,pinned::helper_elf::ForegroundElf,sqlite_install::pair::{bad,smb_policy}};
use crate::sqlite_install::pair::fresh_issuer::{CryptoPins,compiled_v18,require_v18};
use helper_linux_evidence::first_smb::{Enrollment,Libraries};
use rustix::process::Pid;
use std::io;

const GCRYPT:&[&str]=&["usr","lib","local-ops-helper","disk-tools-v1","lib","libgcrypt.so.20"];
const GPGERR:&[&str]=&["usr","lib","local-ops-helper","disk-tools-v1","lib","libgpg-error.so.0"];
fn no()->io::Error{bad("no approved Rust SMB enrollment provider for this original site")}
/// One independently approved SHA on a continuously owned Helper image.
/// The same proof can cross later file/currentness checks within this
/// process. Separate worker processes still qualify their own expected ELF.
pub(in crate::sqlite_install::pair) struct CredentialHelper{image:ForegroundElf,digest:[u8;32]}
impl CredentialHelper{
    fn bind(image:ForegroundElf,digest:[u8;32])->io::Result<Self>{
        image.require_independent_digest(&digest)?;
        Ok(Self{image,digest})
    }
    pub(in crate::sqlite_install::pair) fn from_worker(image:ForegroundElf,expected:[u8;32],parent:Pid)->io::Result<Self>{
        // Preserve the fresh file/mapping and same-parent check that follows
        // decoding the worker's independent sealed expected bytes.
        image.requalify_host_image()?;
        let checked=Self::bind(image,expected)?;
        checked.image.parent_is_same_image(parent)?;
        Ok(checked)
    }
    pub(in crate::sqlite_install::pair) fn require(&self)->io::Result<()>{self.image.require()}
}

pub(in crate::sqlite_install::pair) struct Cryptography{
    helper:CredentialHelper,gcrypt:RuntimeFile,gpgerr:RuntimeFile,
}
impl Cryptography{
    pub(in crate::sqlite_install::pair) fn fresh(pins:CryptoPins)->io::Result<Self>{
        require_v18()?;
        Self::provider_policy(pins)?;
        let helper=CredentialHelper::bind(ForegroundElf::host()?,pins.helper)?;
        Self::selected_providers(pins,helper)
    }
    pub(in crate::sqlite_install::pair) fn from_worker(pins:CryptoPins,helper:CredentialHelper)->io::Result<Self>{
        Self::provider_policy(pins)?;
        if helper.digest!=pins.helper{return Err(no());}
        // Keep the old separate provider-boundary file profile and actual
        // current mapping proof. Only the full read and SHA are redundant.
        helper.image.requalify_host_image()?;
        Self::selected_providers(pins,helper)
    }
    fn provider_policy(pins:CryptoPins)->io::Result<()>{
        compiled_v18()?;pins.check()?;smb_policy::system_policy()?;
        if !smb_policy::clean_environment(){return Err(no());}
        Ok(())
    }
    fn selected_providers(pins:CryptoPins,helper:CredentialHelper)->io::Result<Self>{
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
