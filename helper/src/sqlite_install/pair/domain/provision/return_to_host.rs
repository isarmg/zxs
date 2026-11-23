//! Actual root-owned installation hierarchy and exact original directory locks.
mod deployed;
mod layout;
mod public_record;
mod rust_site;
#[cfg(test)]
mod rust_vault_io;
mod start;
use super::{bad, child, contents, live, mark};
pub(in crate::sqlite_install) use crate::product_layout::protected::lease;
use crate::{
    product_layout::{
        STATE_COMPONENTS as PRODUCT,
        protected::{product_pin, product_top, safe_dir, state_pins},
    },
    sqlite_install::{approval, request::Id},
};
pub(in crate::sqlite_install::pair) use deployed::deployed_worker_payload;
pub(in crate::sqlite_install::pair) use rust_site::{
    VaultStamp, checked_vault_stamp, verify_fresh_worker,
};
pub(in crate::sqlite_install::pair) use start::child as request_live_approval;
use std::{
    ffi::{CStr, CString},
    fs::File,
    io,
};
struct Site {
    host: File,
    chain: Vec<File>,
    leaf: CString,
}
impl Site {
    fn product(host: File, instance: &Id, missing_final: bool) -> io::Result<Self> {
        crate::sqlite_install::pair::compiled_schema()?;
        let leaf = CString::new(crate::fs_tools::lower_hex(instance)).expect("lower hex");
        let mut chain = Vec::from(state_pins(&host)?);
        match product_pin(&chain[1], &leaf) {
            Ok(original) => chain.push(original),
            Err(e) if missing_final && e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        let this = Self { host, chain, leaf };
        this.check()?;
        Ok(this)
    }
    fn component(parent: &File, name: &CStr, level: usize) -> io::Result<File> {
        if level == 0 && name == PRODUCT[0] {
            product_top(parent)
        } else {
            product_pin(parent, name)
        }
    }
    fn base(&self) -> &File {
        self.chain.last().expect("base retained")
    }
    fn check(&self) -> io::Result<()> {
        self.require_from(approval::host_admin()?)
    }
    fn require_from(&self, host: File) -> io::Result<()> {
        safe_dir(&self.host)?;
        safe_dir(&host)?;
        if mark(&self.host)? != mark(&host)? {
            return Err(bad("different current host root"));
        }
        let mut p = host;
        for (level, (original, n)) in self
            .chain
            .iter()
            .zip(
                PRODUCT
                    .iter()
                    .copied()
                    .chain(std::iter::once(self.leaf.as_c_str())),
            )
            .enumerate()
        {
            let next = Self::component(&p, n, level)?;
            safe_dir(original)?;
            if mark(original)? != mark(&next)? {
                return Err(bad("protected installation path changed"));
            }
            p = next;
        }
        Ok(())
    }
}
