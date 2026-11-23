//! Additional checks for retired installation fixtures only.
use super::{PRODUCT, Site, bad, child, mark, safe_dir};
pub(super) use crate::product_layout::protected::{bare_acl, exact_with, protected_control};
use crate::product_layout::protected::{host_mount, state_spelling};
use std::{io, os::unix::fs::MetadataExt};
fn access_chain(site: &Site, gid: u32) -> io::Result<()> {
    site.check()?;
    host_mount(&site.host, &site.chain)?;
    for path in std::iter::once(&site.host).chain(site.chain.iter()) {
        crate::pinned::local_data_filesystem(path)?;
        let dir = child(path, c".")?;
        if mark(&dir)? != mark(path)? {
            return Err(bad("unattributed installation ancestor"));
        }
        safe_dir(&dir)?;
        bare_acl(&dir, c"system.posix_acl_access")?;
        let m = dir.metadata()?;
        let search = if m.gid() == gid { 0o010 } else { 0o001 };
        if m.mode() & search == 0 {
            return Err(bad(
                "storage subject cannot traverse the protected installation ancestors",
            ));
        }
    }
    state_spelling(&site.host, &site.chain[0], &site.chain[1])?;
    site.check()
}
pub(super) fn verify_registered_instance(site: &Site, gid: u32) -> io::Result<()> {
    if site.chain.len() != PRODUCT.len() + 1 {
        return Err(bad("the full original instance path is required"));
    }
    access_chain(site, gid)?;
    let base = site.base();
    let m = base.metadata()?;
    if m.gid() != gid || m.mode() & 0o7777 != 0o750 {
        return Err(bad("not the protected original instance parent"));
    }
    let parent = &site.chain[PRODUCT.len() - 1];
    exact_with(parent, &site.leaf, base, || {
        Site::component(parent, &site.leaf, PRODUCT.len())
    })?;
    site.check()
}
