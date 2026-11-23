//! Private existing-only ZXS name frame. No storage format parser here.
use super::OfflineSession;
use crate::headless_setup::import_preflight::same_name_candidate;
use helper_linux_evidence::diagnostic_process::{
    prepare_offline_catalog_exec, require_waitable_children,
};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{self, BufReader, Read},
    process::{Command, Stdio},
};

const RECORD_BYTES: usize = 255;
const MAGIC: &[u8; 8] = b"ZXCAT1\0\0";
const END: &[u8; 8] = b"ZXEND1\0\0";

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        "could not confirm the complete offline target name catalog",
    )
}

pub(super) fn read_names(session: &OfflineSession) -> io::Result<Vec<String>> {
    session.with_installed(|executable| {
        let expected = Sha256::digest(session.original.full.as_slice()).into();
        capture(executable, &session.locked, expected)
    })
}

fn capture(executable: &File, lease: &File, expected: [u8; 32]) -> io::Result<Vec<String>> {
    require_waitable_children()?;
    let mut command = Command::new("/not-an-offline-catalog-program");
    command
        .env_clear()
        .stdin(Stdio::from(lease.try_clone()?))
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    prepare_offline_catalog_exec(&mut command, executable)?;
    let mut child = command.spawn()?;
    let result = child
        .stdout
        .take()
        .ok_or_else(invalid)
        .and_then(|output| decode_frame(&mut BufReader::new(output), expected));
    if result.is_err() {
        let _ = child.kill();
    }
    let status = child.wait()?;
    if !status.success() {
        return Err(invalid());
    }
    result
}

fn read_fixed<const N: usize>(input: &mut impl Read) -> io::Result<[u8; N]> {
    let mut bytes = [0; N];
    input.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn decode_frame(input: &mut impl Read, expected: [u8; 32]) -> io::Result<Vec<String>> {
    if read_fixed::<8>(input)? != *MAGIC || read_fixed::<32>(input)? != expected {
        return Err(invalid());
    }
    let count = u64::from_be_bytes(read_fixed(input)?);
    let mut names: Vec<String> = Vec::new();
    let mut bytes = [0; RECORD_BYTES];
    for _ in 0..count {
        let width = usize::from(u16::from_be_bytes(read_fixed(input)?));
        let raw = bytes.get_mut(..width).ok_or_else(invalid)?;
        input.read_exact(raw)?;
        if !same_name_candidate(raw) {
            return Err(invalid());
        }
        let name = std::str::from_utf8(raw).map_err(|_| invalid())?;
        if names
            .last()
            .is_some_and(|previous| previous.as_str() >= name)
        {
            return Err(invalid());
        }
        let exhausted = |_| io::Error::from(io::ErrorKind::OutOfMemory);
        let mut owned = String::new();
        owned.try_reserve_exact(name.len()).map_err(exhausted)?;
        owned.push_str(name);
        names.try_reserve(1).map_err(exhausted)?;
        names.push(owned);
    }
    if read_fixed::<8>(input)? != *END {
        return Err(invalid());
    }
    match read_fixed::<1>(input) {
        Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(names),
        Err(error) => Err(error),
        Ok(_) => Err(invalid()),
    }
}

#[cfg(test)]
#[path = "../../../tests/maintenance/offline_catalog_frame.rs"]
mod tests;
