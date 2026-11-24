use std::ffi::{OsStr, OsString};
pub(crate) fn value(
    it: &mut impl Iterator<Item = OsString>,
    flag: &str,
) -> Result<OsString, &'static str> {
    if it.next().as_deref() != Some(OsStr::new(flag)) {
        return Err("expected named setup fields in the displayed order");
    }
    it.next().ok_or("missing setup field")
}
pub(crate) fn sha(
    it: &mut impl Iterator<Item = OsString>,
    flag: &str,
) -> Result<[u8; 32], &'static str> {
    value(it, flag)?
        .to_str()
        .filter(|s| {
            s.bytes()
                .all(|b| b.is_ascii_digit() || matches!(b, b'a'..=b'f'))
        })
        .and_then(crate::asset_digest::hex)
        .filter(|x| *x != [0; 32])
        .ok_or("expected the independent original receipt SHA-256 (64 lowercase hex digits)")
}
