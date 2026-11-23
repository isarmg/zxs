//! Exact protected parent for the optional standalone image publisher.
//! An occupied name or a loss of the original never authorizes publication.
use super::super::{Site, approval, bad, child, lease};
#[cfg(test)]
use super::publish;
use super::{Times, host_mount, mark, protected_control, state_spelling};
use crate::sqlite_install::pair::fresh_issuer::require_private_install;
use rustix::fs::{FlockOperation, Mode, OFlags, RawDir, flock};
use std::{
    ffi::{CStr, CString},
    fs::File,
    io,
    mem::MaybeUninit,
};

#[cfg(test)]
const ENTRY: &CStr = c"deployment.v1";
#[cfg(test)]
const TICKET: &CStr = c".deployment.v1.new";
fn no() -> io::Error {
    bad("not a new protected standalone ZXS deployment; never reuse an earlier layout")
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
pub(in crate::sqlite_install::pair) struct FreshParent {
    parent: Site,
    ex: File,
    instance: [u8; 16],
}
impl FreshParent {
    pub(in crate::sqlite_install::pair) fn select(instance: [u8; 16]) -> io::Result<Self> {
        require_private_install()?;
        approval::no_inherited_descriptors()?;
        approval::same_terminal(&io::stdin(), &io::stderr())?;
        if instance == [0; 16] {
            return Err(no());
        }
        let parent = Site::product(approval::host_admin()?, &instance, true)?;
        if parent.chain.len() != parent.prefix().len() {
            return Err(no());
        }
        let ex = child(parent.base(), c".")?;
        if mark(&ex)? != mark(parent.base())? {
            return Err(no());
        }
        protected_control(&ex)?;
        flock(&ex, FlockOperation::NonBlockingLockExclusive)?;
        let leaf = parent.leaf.to_str().map_err(|_| no())?;
        CString::new(format!(".{leaf}.zxs-new")).map_err(|_| no())?;
        let p = Self {
            parent,
            ex,
            instance,
        };
        p.before_original_sql(&instance)?;
        Ok(p)
    }
    pub(in crate::sqlite_install::pair) fn before_original_sql(
        &self,
        instance: &[u8; 16],
    ) -> io::Result<()> {
        if instance != &self.instance {
            return Err(no());
        }
        self.parent.check()?;
        host_mount(&self.parent)?;
        state_spelling(
            &self.parent.host,
            &self.parent.chain[0],
            &self.parent.chain[1],
        )?;
        lease(&self.ex, mark(self.parent.base())?)?;
        protected_control(&self.ex)?;
        inventory(&self.ex, &[])
    }
    #[cfg(any(test, feature = "embedded-zxs"))]
    pub(in crate::sqlite_install::pair) fn protect_image_destination(
        &self,
        target: crate::pinned::Entity,
    ) -> io::Result<crate::pinned::Entity> {
        self.before_original_sql(&self.instance)?;
        for original in std::iter::once(&self.parent.host).chain(&self.parent.chain) {
            if mark(original)?.0 == target {
                return Err(no());
            }
        }
        Ok(mark(self.parent.base())?.0)
    }
    pub(in crate::sqlite_install::pair) fn product_root(
        &self,
    ) -> io::Result<crate::pinned::Entity> {
        self.before_original_sql(&self.instance)?;
        Ok(mark(&self.parent.chain[0])?.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fresh_v18_exact_dedicated_inventory_and_noreplace_use_only_disposable_names() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = File::open(tmp.path()).unwrap();
        inventory(&dir, &[]).unwrap();
        std::fs::write(tmp.path().join(".deployment.v1.new"), b"fresh-candidate").unwrap();
        assert!(inventory(&dir, &[]).is_err());
        inventory(&dir, &[TICKET.to_bytes()]).unwrap();
        std::fs::write(tmp.path().join("deployment.v1"), b"original-must-stay").unwrap();
        assert!(publish(&dir, TICKET, ENTRY).is_err());
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
