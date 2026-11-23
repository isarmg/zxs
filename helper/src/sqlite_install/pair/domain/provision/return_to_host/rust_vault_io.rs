//! Retained directory-inventory fixture for the retired vault parent. No actual credentials.
use super::{bad, contents};
use crate::sqlite_install::pair::{Id, native_sql::rust_vault::VAULT_NAME as FIXED};
use std::{
    ffi::{CStr, CString},
    fs::File,
    io,
};

fn wrong() -> io::Error {
    bad("wrong exact offline Rust SMB credential state; preserve or query the original operation")
}
fn stage_name(op: &Id) -> io::Result<CString> {
    use std::fmt::Write;
    if op == &[0; 16] {
        return Err(wrong());
    }
    let mut n = String::from("pending-");
    for b in op {
        write!(&mut n, "{b:02x}").map_err(|_| wrong())?;
    }
    CString::new(n).map_err(|_| wrong())
}
fn inventory(dir: &File, active: bool, stage: Option<&CStr>) -> io::Result<()> {
    let mut names = [&b""[..]; 2];
    let mut count = 0;
    if active {
        names[0] = FIXED.to_bytes();
        count = 1;
    }
    if let Some(p) = stage {
        names[count] = p.to_bytes();
        count += 1;
    }
    contents(dir, &names[..count]).map_err(|_| wrong())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn shared_reset_new_cannot_clear_or_adopt_an_unknown_stage_name() {
        // Exercise the real plain directory inventory only. These empty
        // placeholder files are neither a credential nor any actual vault.
        let shelf = tempfile::tempdir().unwrap();
        std::fs::File::create(shelf.path().join("credential.v2")).unwrap();
        let dir = std::fs::File::open(shelf.path()).unwrap();
        inventory(&dir, true, None).unwrap();
        let name = stage_name(&[50; 16]).unwrap();
        let name = name.to_str().unwrap();
        std::fs::File::create(shelf.path().join(name)).unwrap();
        assert!(inventory(&dir, true, None).is_err());
        assert!(shelf.path().join(name).is_file() && shelf.path().join("credential.v2").is_file());
    }
}
