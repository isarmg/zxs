//! Parent-side loan of the single original verifier file after the lowered
//! ACTIVE proof. Only the strict isolated receiver reads its 240 bytes.
use super::{SiteLease,denied,held::{self,Stamp}};
use crate::smb::launch::envelope::VaultStamp;
use std::{fs::File,io,os::fd::{AsFd,BorrowedFd}};

use crate::system::shared_account::{VAULT_NAME as NAME,VAULT_LENGTH};
const MEMBERS:&[&[u8]]=&[NAME.to_bytes()];

#[must_use]
pub(in crate::smb::launch) struct Vault<'s,'p>{
    site:&'s SiteLease<'p>,
    pin:File,
    file:File,
    stamp:Stamp,
    witness:VaultStamp,
}
impl<'s,'p> Vault<'s,'p>{
    /// Only SiteLease::vault may call this, after matching the authenticated
    /// low receipt to the actual primary SH and exclusive site held here.
    pub(super) fn select(site:&'s SiteLease<'p>)->io::Result<Self>{
        site.require()?;
        let dir=&site.vault;
        held::exactly(dir,MEMBERS)?;
        let pin=held::leaf(dir,NAME)?;
        let stamp=held::regular(&pin)?;
        let parent=held::place(dir)?;
        if stamp.place.mount!=parent.mount||stamp.place.entity.device!=parent.entity.device{return Err(denied());}
        let witness=witness(stamp)?;
        let file=held::read_original(&site.task,&pin,stamp)?;
        let v=Self{site,pin,file,stamp,witness};
        v.require()?;Ok(v)
    }
    pub(in crate::smb::launch) fn require(&self)->io::Result<()>{
        self.site.require()?;
        let dir=&self.site.vault;
        held::exactly(dir,MEMBERS)?;
        held::rdonly(&self.file)?;
        let same=||->io::Result<bool>{Ok(
            held::regular(&self.pin)?==self.stamp&&held::regular(&self.file)?==self.stamp&&
                held::regular(&held::leaf(dir,NAME)?)?==self.stamp)};
        if !same()?{return Err(denied());}
        held::no_xattr(&self.file,c"system.posix_acl_access")?;
        if !same()?{return Err(denied());}
        held::exactly(dir,MEMBERS)?;
        Ok(())
    }
    /// The mount-qualified retained RO fd and public time/inode witness
    /// cannot be recombined from independently supplied file paths or bytes.
    pub(in crate::smb::launch) fn offer(&self)->io::Result<(BorrowedFd<'_>,VaultStamp)>{
        self.require()?;Ok((self.file.as_fd(),self.witness))
    }
}
fn witness(stamp:Stamp)->io::Result<VaultStamp>{
    let nsec=|n:i64|->io::Result<u32>{u32::try_from(n).ok().filter(|v|*v<1_000_000_000).ok_or_else(denied)};
    if stamp.mode!=0o100600||stamp.uid!=0||stamp.gid!=0||stamp.bytes!=VAULT_LENGTH as u64||stamp.place.entity.inode==0{
        return Err(denied());
    }
    Ok(VaultStamp{dev:stamp.place.entity.device,ino:stamp.place.entity.inode,
        ctime_s:stamp.ctime.0,ctime_ns:nsec(stamp.ctime.1)?,mtime_s:stamp.mtime.0,mtime_ns:nsec(stamp.mtime.1)?})
}

#[cfg(test)]
#[path = "../../../../tests/unit/attached/smb/launch/site/vault/tests.rs"]
mod tests;
