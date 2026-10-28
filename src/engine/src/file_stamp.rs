//! Metadata snapshot for held-file preparation evidence, not a content proof.
use crate::Result;
use std::{fs::File, os::unix::fs::MetadataExt};

#[derive(Debug, PartialEq, Eq)]
pub(crate) struct FileStamp {
    device: u64,
    inode: u64,
    pub(crate) size: u64,
    links: u64,
    mode: u32,
    modified: (i64, i64),
    changed: (i64, i64),
}

impl FileStamp {
    pub(crate) fn of(file: &File) -> Result<Self> {
        let metadata = file.metadata()?;
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            size: metadata.len(),
            links: metadata.nlink(),
            mode: metadata.mode(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        })
    }
}

#[cfg(test)]
#[path = "../../../test/engine/unit/file_stamp.rs"]
mod tests;
