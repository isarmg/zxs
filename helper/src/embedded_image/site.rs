//! Exclusive lock on the original empty state parent for publishing one image.
//! No pool identity, SQLite schema, account or startup decision participates.
use crate::{
    foreground,
    maintenance::host_admin,
    pinned::Entity,
    product_layout::protected::{
        Times, child, host_mount, lease, mark, protected_control, safe_dir, state_pins,
        state_spelling,
    },
};
use rustix::fs::{FlockOperation, Mode, OFlags, RawDir, flock};
use std::{fs::File, io, mem::MaybeUninit};

fn no() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "not the original protected empty ZXS deployment parent",
    )
}

fn inventory(dir: &File, expected: &[&[u8]]) -> io::Result<()> {
    let before = Times::on(dir)?;
    let other = File::from(rustix::fs::openat(
        dir,
        c".",
        OFlags::RDONLY
            | OFlags::DIRECTORY
            | OFlags::NONBLOCK
            | OFlags::CLOEXEC
            | OFlags::NOFOLLOW
            | OFlags::NOATIME,
        Mode::empty(),
    )?);
    if mark(dir)? != mark(&other)? || expected.len() > 3 {
        return Err(no());
    }
    let mut buf = [MaybeUninit::uninit(); 4096];
    let mut raw = RawDir::new(&other, &mut buf);
    let mut seen = 0u8;
    while let Some(e) = raw.next() {
        let e = e?;
        let n = e.file_name().to_bytes();
        if matches!(n, b"." | b"..") {
            continue;
        }
        let i = expected.iter().position(|f| *f == n).ok_or_else(no)?;
        if seen & (1 << i) != 0 {
            return Err(no());
        }
        seen |= 1 << i;
    }
    if seen != (1 << expected.len()) - 1
        || Times::on(dir)? != before
        || Times::on(&other)? != before
    {
        return Err(no());
    }
    Ok(())
}

#[must_use]
pub(super) struct PublishSite {
    host: File,
    chain: [File; 2],
    exclusive: File,
}
impl PublishSite {
    pub(super) fn select() -> io::Result<Self> {
        foreground::no_inherited_descriptors()?;
        foreground::same_terminal(&io::stdin(), &io::stderr())?;
        let host = host_admin()?;
        let chain = state_pins(&host)?;
        let exclusive = child(&chain[1], c".")?;
        if mark(&exclusive)? != mark(&chain[1])? {
            return Err(no());
        }
        protected_control(&exclusive)?;
        flock(&exclusive, FlockOperation::NonBlockingLockExclusive)?;
        let site = Self {
            host,
            chain,
            exclusive,
        };
        site.require()?;
        Ok(site)
    }
    pub(super) fn require(&self) -> io::Result<()> {
        let host = host_admin()?;
        safe_dir(&self.host)?;
        safe_dir(&host)?;
        if mark(&host)? != mark(&self.host)? {
            return Err(no());
        }
        for (original, current) in self.chain.iter().zip(state_pins(&host)?) {
            safe_dir(original)?;
            if mark(original)? != mark(&current)? {
                return Err(no());
            }
        }
        host_mount(&self.host, &self.chain)?;
        let [top, state] = &self.chain;
        state_spelling(&self.host, top, state)?;
        lease(&self.exclusive, mark(state)?)?;
        protected_control(&self.exclusive)?;
        inventory(&self.exclusive, &[])
    }
    pub(super) fn protect_destination(&self, target: Entity) -> io::Result<Entity> {
        self.require()?;
        for original in std::iter::once(&self.host).chain(&self.chain) {
            if mark(original)?.0 == target {
                return Err(no());
            }
        }
        Ok(mark(&self.chain[1])?.0)
    }
    pub(super) fn product_root(&self) -> io::Result<Entity> {
        self.require()?;
        Ok(mark(&self.chain[0])?.0)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use rustix::fs::{RenameFlags, renameat_with};
    const ENTRY: &std::ffi::CStr = c"deployment.v1";
    const TICKET: &std::ffi::CStr = c".deployment.v1.new";

    #[test]
    fn fresh_v18_exact_dedicated_inventory_and_noreplace_use_only_disposable_names() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = File::open(tmp.path()).unwrap();
        inventory(&dir, &[]).unwrap();
        std::fs::write(tmp.path().join(".deployment.v1.new"), b"fresh-candidate").unwrap();
        assert!(inventory(&dir, &[]).is_err());
        inventory(&dir, &[TICKET.to_bytes()]).unwrap();
        std::fs::write(tmp.path().join("deployment.v1"), b"original-must-stay").unwrap();
        assert!(renameat_with(&dir, TICKET, &dir, ENTRY, RenameFlags::NOREPLACE).is_err());
        assert_eq!(
            std::fs::read(tmp.path().join("deployment.v1")).unwrap(),
            b"original-must-stay"
        );
        assert_eq!(
            std::fs::read(tmp.path().join(".deployment.v1.new")).unwrap(),
            b"fresh-candidate"
        );
        assert!(inventory(&dir, &[ENTRY.to_bytes()]).is_err());
    }
}
