//! Keep the actual product site EX and original SYSTEM EX throughout issuance.
use super::{Site,bad,layout,lease,mark};
use crate::{pinned::Pinned,sqlite_install::request::Id};
use std::{fs::File,io,os::unix::fs::MetadataExt};

pub(super) struct SiteLease{
    pub(super) system:Pinned,
    pub(super) system_lock:File,
    pub(super) original:Site,
    pub(super) site_ex:File,
    pub(super) subject:(Id,u32,u32),
    explicit:crate::sqlite_install::pair::ExplicitSubject,
}
impl SiteLease{
    /// Only a freshly published protected product instance may lend its
    /// real creator's directory lease. There is no lookup/recovery here.
    pub(super) fn from_product(system:Pinned,system_lock:File,original:Site,site_ex:File,
        subject:crate::sqlite_install::pair::ExplicitSubject)->io::Result<Self>{
        crate::sqlite_install::pair::require_v18()?;
        let x=Self{system,system_lock,original,site_ex,subject:subject.tuple(),explicit:subject};
        x.require()?;Ok(x)
    }
    pub(super) fn require(&self)->io::Result<()>{
        layout::verify_registered_instance(&self.original,self.subject.2)?;
        self.system.require_same_named_mount()?;
        let m=self.system.metadata()?;
        crate::sqlite_install::pair::require_v18()?;
        let subject=self.explicit.tuple();
        if subject!=self.subject||!m.is_dir()||m.uid()!=self.subject.1||m.mode()!=0o040700{
            return Err(bad("SYSTEM authorization, scope or writable mount changed"));
        }
        let s=mark(&self.system.storage_directory()?)?;
        lease(&self.system_lock,s)?;
        let site=mark(self.original.base())?;lease(&self.site_ex,site)?;
        if self.system.visible_chain().contains(&site.0)
            ||self.original.chain.iter().map(mark).collect::<io::Result<Vec<_>>>()?.iter().any(|(e,_)|*e==s.0){
            return Err(bad("the host instance and SYSTEM overlap"));
        }
        Ok(())
    }
}
