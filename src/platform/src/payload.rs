//! Stream a complete immutable payload into a new inode and publish it durably.
//!
//! This module deliberately never uses hard links, reflinks, sparse copy APIs,
//! or deduplication. It writes all bytes from offset zero. Disk placement,
//! independent physical devices, and cross-disk decision records belong to the
//! storage engine. A returned receipt attests this file publication only.

use crate::fs_safety::{EntryName, NoClobberPublish, PrivateDirectory, RelativePath};
use error_definition::Error;
use rustix::fs::{AtFlags, Mode, OFlags, openat, unlinkat};
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom, Write},
};

/// Sealed upload expectation. A byte limit is mandatory even without a digest.
#[derive(Clone, Copy, Debug)]
pub struct Expectation {
    pub bytes: u64,
    pub blake3: Option<blake3::Hash>,
}

/// Evidence for one complete, synced, single-linked file, never a transaction proof.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Receipt {
    pub bytes: u64,
    pub blake3: blake3::Hash,
    pub identity: crate::fs_safety::linux::FileIdentity,
}

#[derive(Debug, Error)]
pub enum Error {
    #[error("{operation}; staging cleanup also failed: {cleanup}")]
    Cleanup {
        #[source]
        operation: Box<Error>,
        cleanup: io::Error,
    },
    #[error("payload length does not match sealed length {expected}: observed {actual}")]
    Length { expected: u64, actual: u64 },
    #[error("payload BLAKE3 does not match sealed digest")]
    Digest,
    #[error("secure random staging name could not be generated")]
    Randomness(#[source] system_entropy::Error),
    #[error(transparent)]
    Filesystem(#[from] crate::fs_safety::Error),
    #[error(transparent)]
    Io(#[from] io::Error),
}

struct StagingCleanup {
    directory: File,
    name: String,
    armed: bool,
}

impl StagingCleanup {
    fn cleanup(&mut self) -> io::Result<()> {
        if !self.armed {
            return Ok(());
        }
        self.armed = false;
        match unlinkat(&self.directory, self.name.as_str(), AtFlags::empty()) {
            Ok(()) | Err(rustix::io::Errno::NOENT) => Ok(()),
            Err(error) => Err(error.into()),
        }
    }
}
impl Drop for StagingCleanup {
    fn drop(&mut self) {
        if let Err(error) = self.cleanup() {
            tracing::error!(%error, "payload staging cleanup during unwind failed");
        }
    }
}

/// Write exactly the sealed byte count using a fixed 64 KiB buffer.
///
/// The destination must not exist. A destination that already exists, including
/// a symlink, is never overwritten. On length/digest/I/O failures before rename,
/// the exclusive staging file is removed. If final directory synchronization
/// fails after rename, [`crate::fs_safety::Error::PublishedDurabilityUnknown`]
/// is preserved so the engine can reconcile rather than falsely report failure.
pub fn write_full(
    directory: &PrivateDirectory,
    destination: &EntryName,
    reader: &mut impl Read,
    expectation: Expectation,
) -> Result<Receipt, Error> {
    let mut nonce = [0u8; 16];
    system_entropy::fill(&mut nonce).map_err(Error::Randomness)?;
    let mut name = String::from(".zxs-payload-");
    use std::fmt::Write as _;
    for byte in nonce {
        write!(&mut name, "{byte:02x}").expect("String formatting is infallible");
    }
    name.push_str(".tmp");
    // Acquire the cleanup handle before creating anything so a descriptor
    // exhaustion error cannot strand a staging file.
    let cleanup_directory = directory.directory_handle().try_clone()?;
    let staging = File::from(
        openat(
            directory.directory_handle(),
            name.as_str(),
            OFlags::WRONLY | OFlags::CREATE | OFlags::EXCL | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::from_raw_mode(0o600),
        )
        .map_err(io::Error::from)?,
    );
    let mut cleanup = StagingCleanup {
        directory: cleanup_directory,
        name,
        armed: true,
    };
    let mut staging = staging;
    let result = (|| {
        let identity = crate::fs_safety::linux::SingleLinkRequirement::check(&staging)?;
        let mut hasher = blake3::Hasher::new();
        let mut bytes = 0u64;
        let mut buffer = [0u8; 64 * 1024];
        loop {
            let read = match reader.read(&mut buffer) {
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                result => result?,
            };
            if read == 0 {
                break;
            }
            bytes = bytes.checked_add(read as u64).ok_or(Error::Length {
                expected: expectation.bytes,
                actual: u64::MAX,
            })?;
            if bytes > expectation.bytes {
                return Err(Error::Length {
                    expected: expectation.bytes,
                    actual: bytes,
                });
            }
            staging.write_all(&buffer[..read])?;
            hasher.update(&buffer[..read]);
        }
        if bytes != expectation.bytes {
            return Err(Error::Length {
                expected: expectation.bytes,
                actual: bytes,
            });
        }
        let digest = hasher.finalize();
        if expectation
            .blake3
            .is_some_and(|expected| expected != digest)
        {
            return Err(Error::Digest);
        }
        staging.sync_all()?;
        let published = NoClobberPublish::publish(
            directory,
            &RelativePath::new(&cleanup.name)?,
            &destination.as_relative(),
        );
        if matches!(
            &published,
            Ok(())
                | Err(crate::fs_safety::Error::PublishedDurabilityUnknown(_)
                    | crate::fs_safety::Error::IdentityChanged(_))
        ) {
            cleanup.armed = false;
        }
        published?;
        Ok(Receipt {
            bytes,
            blake3: digest,
            identity,
        })
    })();
    match result {
        Ok(value) => Ok(value),
        Err(operation) => match cleanup.cleanup() {
            Ok(()) => Err(operation),
            Err(cleanup) => Err(Error::Cleanup {
                operation: Box::new(operation),
                cleanup,
            }),
        },
    }
}

/// Verify the current single-linked file content with bounded memory.
/// Reading starts at offset zero. Callers must keep the immutable payload
/// read-only and prevent concurrent writers through core locks.
pub fn verify_full(file: &mut File, expectation: Expectation) -> Result<Receipt, Error> {
    let identity = crate::fs_safety::linux::SingleLinkRequirement::check(&*file)?;
    file.seek(SeekFrom::Start(0))?;
    let mut hasher = blake3::Hasher::new();
    let mut bytes = 0u64;
    let mut buffer = [0u8; 64 * 1024];
    loop {
        let read = match file.read(&mut buffer) {
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if read == 0 {
            break;
        }
        bytes = bytes.checked_add(read as u64).ok_or(Error::Length {
            expected: expectation.bytes,
            actual: u64::MAX,
        })?;
        if bytes > expectation.bytes {
            return Err(Error::Length {
                expected: expectation.bytes,
                actual: bytes,
            });
        }
        hasher.update(&buffer[..read]);
    }
    if bytes != expectation.bytes {
        return Err(Error::Length {
            expected: expectation.bytes,
            actual: bytes,
        });
    }
    let digest = hasher.finalize();
    if expectation
        .blake3
        .is_some_and(|expected| expected != digest)
    {
        return Err(Error::Digest);
    }
    Ok(Receipt {
        bytes,
        blake3: digest,
        identity,
    })
}

#[cfg(test)]
#[path = "../../../test/platform/unit/payload/tests.rs"]
mod tests;
