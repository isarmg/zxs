//! Private fresh-only issuance; no server/start, separate domain or generic credentials.
#[cfg(test)]
use super::Id;
pub(super) use crate::embedded_image::arguments::{sha, value};
#[cfg(test)]
use crate::sqlite_install::id;
use std::ffi::{OsStr, OsString};

#[cfg(test)]
#[derive(Debug)]
pub(crate) struct Setup;
#[cfg(test)]
pub(super) fn key(it: &mut impl Iterator<Item = OsString>, flag: &str) -> Result<Id, &'static str> {
    id(&value(it, flag)?).ok_or("setup identifiers must be 32 lowercase nonzero hex digits")
}
#[cfg(test)]
pub(super) fn name(
    it: &mut impl Iterator<Item = OsString>,
    flag: &str,
) -> Result<Vec<u8>, &'static str> {
    let s = value(it, flag)?;
    let b = s.as_encoded_bytes();
    if !super::login(b) {
        return Err("expected a bounded lowercase ASCII login name");
    }
    Ok(b.to_vec())
}

#[cfg(test)]
impl Setup {
    pub(crate) fn parse(mut it: impl Iterator<Item = OsString>) -> Result<Self, &'static str> {
        if !cfg!(feature = "private-fresh-install") {
            return Err("the private fresh setup must be enabled explicitly");
        }
        let first = it.next();
        let web = first.as_deref() == Some(OsStr::new("web"));
        let action = if web { it.next() } else { first };
        if action.as_deref() != Some(OsStr::new("issue-fresh")) {
            return Err("choose issue-fresh or web issue-fresh; no service command");
        }
        super::fresh_issuer::Selection::parse(it)?;
        Ok(Self)
    }
}
