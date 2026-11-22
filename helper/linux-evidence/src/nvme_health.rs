//! One controller-wide SMART / Health Information log, on a held block FD.
use linux_raw_sys::ioctl::{BLKGETDISKSEQ, NVME_IOCTL_ADMIN_CMD};
use rustix::{fs::{OFlags, fcntl_getfl}, io::{FdFlags, fcntl_getfd}};
use std::{fs::File, io, mem::{align_of, offset_of, size_of}, os::{fd::AsRawFd, unix::fs::{FileTypeExt, MetadataExt}}};

pub const LOG_BYTES: usize = 512;
const GET_LOG_PAGE: u8 = 0x02;
const SMART_INFORMATION: u32 = 0x02;
const RETAIN_EVENT: u32 = 1 << 15;
const CDW10: u32 = (((LOG_BYTES as u32 / 4) - 1) << 16) | RETAIN_EVENT | SMART_INFORMATION;

// Linux UAPI linux/nvme_ioctl.h, struct nvme_passthru_cmd (not the 64-bit result form).
#[repr(C)]
struct LogCommand {
    opcode: u8, flags: u8, reserved: u16, namespace: u32,
    cdw2: u32, cdw3: u32, metadata: u64, address: u64,
    metadata_len: u32, data_len: u32, cdw10: u32, cdw11: u32,
    cdw12: u32, cdw13: u32, cdw14: u32, cdw15: u32,
    timeout_ms: u32, result: u32,
}
const _: () = assert!(size_of::<LogCommand>() == 72 && align_of::<LogCommand>() == 8
    && offset_of!(LogCommand, address) == 24 && offset_of!(LogCommand, data_len) == 36
    && offset_of!(LogCommand, cdw10) == 40 && offset_of!(LogCommand, timeout_ms) == 64
    && NVME_IOCTL_ADMIN_CMD == 0xc048_4e41 && BLKGETDISKSEQ == 0x8008_1280);

#[derive(Debug)]
pub enum Failure { Io(io::Error), Changed, Unsupported }
impl From<io::Error> for Failure {
    fn from(value:io::Error)->Self{Self::Io(value)}
}

/// Never opens a device, requests a controller state change, clears event
/// notices, or accepts a command from its caller. The caller proves which
/// NVMe subsystem owns this held namespace, before and after this observation.
///
/// # Errors
/// Refuses a different device or generation, a non-read-only FD, any control
/// result other than completion, or a system error. Has no fallback or retry.
#[allow(unsafe_code)]
pub fn read(original:&File, expected_device:u64, expected_generation:u64)->Result<[u8;LOG_BYTES],Failure>{
    let info=original.metadata()?;
    let flags=fcntl_getfl(original).map_err(io::Error::from)?;
    if !info.file_type().is_block_device()||!flags.contains(OFlags::NONBLOCK)||flags.contains(OFlags::PATH)
        ||flags&OFlags::ACCMODE!=OFlags::RDONLY
        ||!fcntl_getfd(original).map_err(io::Error::from)?.contains(FdFlags::CLOEXEC){return Err(Failure::Unsupported);}
    if info.rdev()!=expected_device||expected_generation==0{return Err(Failure::Changed);}
    let require_generation=||{
        let mut current=0u64;
        // SAFETY: BLKGETDISKSEQ writes one complete __u64 to this live output,
        // on the already type/flag-checked owned original block descriptor.
        let result=unsafe{libc::ioctl(original.as_raw_fd(),libc::c_ulong::from(BLKGETDISKSEQ),&raw mut current)};
        match result{
            0 if current==expected_generation=>Ok(()),
            0=>Err(Failure::Changed),
            -1=>Err(Failure::Io(io::Error::last_os_error())),
            _=>Err(Failure::Unsupported),
        }
    };
    require_generation()?;
    let mut log=[0u8;LOG_BYTES];
    let mut command=LogCommand{
        opcode:GET_LOG_PAGE,flags:0,reserved:0,namespace:u32::MAX,cdw2:0,cdw3:0,metadata:0,
        address:log.as_mut_ptr() as u64,metadata_len:0,data_len:LOG_BYTES as u32,cdw10:CDW10,
        cdw11:0,cdw12:0,cdw13:0,cdw14:0,cdw15:0,timeout_ms:0,result:0,
    };
    // SAFETY: synchronous fixed Linux LP64 UAPI struct and its one live
    // bounded data-out buffer. Kernel/driver write the log into this buffer;
    // it is never supplied as command data to the controller. All reserved,
    // vendor and offset fields are zero. There is no caller-supplied opcode.
    let done=unsafe{libc::ioctl(original.as_raw_fd(),libc::c_ulong::from(NVME_IOCTL_ADMIN_CMD),&raw mut command)};
    match done{
        0=>(),
        -1=>return Err(Failure::Io(io::Error::last_os_error())),
        _=>return Err(Failure::Unsupported),
    }
    require_generation()?;
    Ok(log)
}
