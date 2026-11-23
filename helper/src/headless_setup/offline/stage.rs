//! Bounded recovery of the exact original stage, never replacement or cleanup.
use super::{Record, SCOPED, read, refused, stamp};
use rustix::fs::{Mode, OFlags, fchmod, openat2};
use std::{
    fs::File,
    io::{self, Write},
};

pub(super) fn read_stage(dir: &File, name: &str, uid: u32) -> io::Result<Option<Record>> {
    match read(dir, name, uid) {
        Ok(record) => Ok(Some(record)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub(super) fn finish_stage(
    dir: &File,
    name: &str,
    expected: &[u8],
    uid: u32,
    prove: impl Fn() -> io::Result<()>,
) -> io::Result<Record> {
    prove()?;
    let (mut file, written) = if let Some(original) = read_stage(dir, name, uid)? {
        if !expected.starts_with(&original.full) {
            return Err(refused());
        }
        let file = File::from(openat2(
            dir,
            name,
            OFlags::WRONLY | OFlags::APPEND | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
            SCOPED,
        )?);
        if stamp(&file.metadata()?) != stamp(&original.meta) {
            return Err(refused());
        }
        (file, original.full.len())
    } else {
        let file = File::from(openat2(
            dir,
            name,
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR,
            SCOPED,
        )?);
        fchmod(&file, Mode::RUSR | Mode::WUSR)?;
        (file, 0)
    };
    prove()?;
    if stamp(&read(dir, name, uid)?.meta) != stamp(&file.metadata()?) {
        return Err(refused());
    }
    file.write_all(&expected[written..])?;
    file.sync_all()?;
    let record = read(dir, name, uid)?;
    if stamp(&record.meta) != stamp(&file.metadata()?) || record.full.as_slice() != expected {
        return Err(refused());
    }
    Ok(record)
}
