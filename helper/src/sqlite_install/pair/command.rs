//! Private fresh-only issuance; no server/start, separate domain or generic credentials.
use super::Id;
use crate::sqlite_install::id;
use std::ffi::{OsStr, OsString};

#[cfg(test)]
#[derive(Debug)]
pub(crate) struct Setup;
pub(super) fn value(
    it: &mut impl Iterator<Item = OsString>,
    flag: &str,
) -> Result<OsString, &'static str> {
    if it.next().as_deref() != Some(OsStr::new(flag)) {
        return Err("expected named setup fields in the displayed order");
    }
    it.next().ok_or("missing setup field")
}
pub(super) fn key(it: &mut impl Iterator<Item = OsString>, flag: &str) -> Result<Id, &'static str> {
    id(value(it, flag)?).ok_or("setup identifiers must be 32 lowercase nonzero hex digits")
}
pub(super) fn sha(
    it: &mut impl Iterator<Item = OsString>,
    flag: &str,
) -> Result<[u8; 32], &'static str> {
    value(it, flag)?
        .to_str()
        .filter(|s| {
            s.bytes()
                .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
        })
        .and_then(crate::fs_tools::hex)
        .filter(|x| *x != [0; 32])
        .ok_or("expected the independent original receipt SHA-256 (64 lowercase hex digits)")
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
