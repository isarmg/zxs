//! Synchronous, fixed read-only Linux SMART commands on an already held device.
//!
//! These calls never create a process or submit asynchronous work. The caller
//! must retain its device/admission owner until the call returns, even after a
//! caller deadline or cancellation. Driver timeouts are not a completion proof.
//! Only NVMe Identify/Get Log and ATA CHECK POWER MODE/IDENTIFY/SMART reads are
//! exposed; no general pass-through, settings, enablement or self-test API exists.
use std::{io, os::fd::BorrowedFd, time::Duration};

mod nvme;
mod sat;

pub use nvme::{NvmeController, NvmeHealth, nvme_controller, nvme_health, nvme_namespace};
pub use sat::{AtaData, AtaHealth, AtaPowerMode, ata_data, ata_health, ata_power_mode};

#[derive(Debug, error_definition::Error)]
pub enum Error {
    #[error("{operation}: {source}")]
    Io {
        operation: &'static str,
        #[source]
        source: io::Error,
    },
    #[error("{operation}: NVMe completion status {status:#x}")]
    NvmeStatus {
        operation: &'static str,
        status: i32,
    },
    #[error("{operation}: SCSI status {status:#x}, host {host:#x}, driver {driver:#x}")]
    ScsiStatus {
        operation: &'static str,
        status: u8,
        host: u16,
        driver: u16,
    },
    #[error("{operation}: SCSI sense key {key:#x}, ASC {asc:#x}, ASCQ {ascq:#x}")]
    ScsiSense {
        operation: &'static str,
        key: u8,
        asc: u8,
        ascq: u8,
    },
    #[error("{operation}: ATA status {status:#x}, error {error:#x}")]
    AtaStatus {
        operation: &'static str,
        status: u8,
        error: u8,
    },
    #[error("{operation}: {detail}")]
    InvalidResponse {
        operation: &'static str,
        detail: &'static str,
    },
    #[error("SMART query deadline elapsed")]
    TimedOut,
}

impl Error {
    fn io(operation: &'static str) -> Self {
        Self::Io {
            operation,
            source: io::Error::last_os_error(),
        }
    }

    fn invalid(operation: &'static str, detail: &'static str) -> Self {
        Self::InvalidResponse { operation, detail }
    }
}

/// Only Linux block descriptors use the synchronous block-device SG_IO path.
/// Character-device /dev/sg may retain an interrupted request, so a generic FD
/// must not cross this boundary even when it accepts the same ioctl number.
#[derive(Clone, Copy, Debug)]
pub struct BlockDevice<'fd>(BorrowedFd<'fd>);

impl<'fd> BlockDevice<'fd> {
    pub fn bind(fd: BorrowedFd<'fd>) -> Result<Self, Error> {
        let metadata = rustix::fs::fstat(fd).map_err(|source| Error::Io {
            operation: "inspect SMART block descriptor",
            source: source.into(),
        })?;
        if rustix::fs::FileType::from_raw_mode(metadata.st_mode)
            != rustix::fs::FileType::BlockDevice
        {
            return Err(Error::invalid(
                "inspect SMART block descriptor",
                "not a block device",
            ));
        }
        Ok(Self(fd))
    }
}

/// A nonzero, bounded kernel timeout. Zero means "driver default" in both APIs,
/// so an expired deadline must never become a zero timeout on a device command.
#[derive(Clone, Copy, Debug)]
pub struct CommandTimeout(u32);

impl CommandTimeout {
    pub fn remaining(duration: Duration) -> Result<Self, Error> {
        if duration.is_zero() {
            return Err(Error::TimedOut);
        }
        let milliseconds =
            duration.as_millis() + u128::from(duration.subsec_nanos() % 1_000_000 != 0);
        let milliseconds = u32::try_from(milliseconds).map_err(|_| {
            Error::invalid("set SMART command deadline", "timeout exceeds Linux ABI")
        })?;
        // u32::MAX is SG_IO's no-timeout sentinel.
        if milliseconds == u32::MAX {
            return Err(Error::invalid(
                "set SMART command deadline",
                "unbounded timeout",
            ));
        }
        Ok(Self(milliseconds))
    }
}

#[cfg(test)]
#[path = "../../../../test/platform/unit/disk_smart/mod.rs"]
mod tests;
