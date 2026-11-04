use super::{BlockDevice, CommandTimeout, Error};
use std::os::fd::AsRawFd;

// Linux UAPI <linux/nvme_ioctl.h>, NVME_IOCTL_ID and NVME_IOCTL_ADMIN_CMD.
const NVME_IOCTL_ID: u32 = 0x4e40;
const NVME_IOCTL_ADMIN_CMD: u32 = 0xc048_4e41;

#[repr(C)]
#[derive(Default)]
struct AdminCommand {
    opcode: u8,
    flags: u8,
    reserved: u16,
    namespace: u32,
    cdw2: u32,
    cdw3: u32,
    metadata: u64,
    address: u64,
    metadata_len: u32,
    data_len: u32,
    cdw10: u32,
    cdw11: u32,
    cdw12: u32,
    cdw13: u32,
    cdw14: u32,
    cdw15: u32,
    timeout_ms: u32,
    result: u32,
}

const _: () = assert!(std::mem::size_of::<AdminCommand>() == 72);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NvmeController {
    per_namespace_health: bool,
}

impl NvmeController {
    pub fn health_namespace(self, namespace: u32) -> u32 {
        if self.per_namespace_health {
            namespace
        } else {
            u32::MAX
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NvmeHealth {
    pub critical_warning: u8,
    pub temperature_celsius: Option<i32>,
    pub power_on_hours: u64,
}

/// Read the namespace identifier from the held namespace block descriptor.
#[allow(unsafe_code)]
pub fn nvme_namespace(device: BlockDevice<'_>) -> Result<u32, Error> {
    // SAFETY: this ioctl has no pointer argument and borrows a live descriptor.
    // It reads the kernel's namespace identity without submitting device I/O.
    let result = unsafe { libc::ioctl(device.0.as_raw_fd(), NVME_IOCTL_ID as _) };
    if result == -1 {
        return Err(Error::io("read NVMe namespace identifier"));
    }
    let namespace = result as u32;
    if namespace == 0 || namespace == u32::MAX {
        return Err(Error::invalid(
            "read NVMe namespace identifier",
            "not an individual namespace",
        ));
    }
    Ok(namespace)
}

pub fn nvme_controller(
    device: BlockDevice<'_>,
    timeout: CommandTimeout,
) -> Result<NvmeController, Error> {
    let mut identify = [0; 4096];
    let command = AdminCommand {
        opcode: 0x06,
        cdw10: 1,
        ..AdminCommand::default()
    };
    execute(
        device,
        command,
        &mut identify,
        timeout,
        "identify NVMe controller",
    )?;
    parse_controller(&identify)
}

pub fn nvme_health(
    device: BlockDevice<'_>,
    namespace: u32,
    timeout: CommandTimeout,
) -> Result<NvmeHealth, Error> {
    let mut log = [0; 512];
    // Get Log Page 02h, 128 dwords (NUMD=127), no setting or power changes.
    let command = AdminCommand {
        opcode: 0x02,
        namespace,
        cdw10: 0x007f_0002,
        ..AdminCommand::default()
    };
    execute(
        device,
        command,
        &mut log,
        timeout,
        "read NVMe SMART health log",
    )?;
    parse_health(&log)
}

#[allow(unsafe_code)]
fn execute<const N: usize>(
    device: BlockDevice<'_>,
    mut command: AdminCommand,
    data: &mut [u8; N],
    timeout: CommandTimeout,
    operation: &'static str,
) -> Result<(), Error> {
    command.address = data.as_mut_ptr() as u64;
    command.data_len = u32::try_from(N).expect("invariant: fixed NVMe buffers fit the ABI");
    command.timeout_ms = timeout.0;
    // SAFETY: repr(C) matches the Linux amd64 UAPI and the size is asserted.
    // The only callers construct fixed read-only opcodes with no metadata. The
    // initialized mutable data and command buffers remain exclusively borrowed
    // for the synchronous ioctl; the kernel completes/unmaps before returning.
    let result = unsafe {
        libc::ioctl(
            device.0.as_raw_fd(),
            NVME_IOCTL_ADMIN_CMD as _,
            &mut command,
        )
    };
    match result {
        -1 => Err(Error::io(operation)),
        0 => Ok(()),
        status => Err(Error::NvmeStatus { operation, status }),
    }
}

fn parse_controller(data: &[u8; 4096]) -> Result<NvmeController, Error> {
    if data.iter().all(|byte| *byte == 0) || data.iter().all(|byte| *byte == 0xff) {
        return Err(Error::invalid(
            "identify NVMe controller",
            "empty or erased identification",
        ));
    }
    // NVMe Identify Controller byte 261: Log Page Attributes bit 0.
    Ok(NvmeController {
        per_namespace_health: data[261] & 1 != 0,
    })
}

fn parse_health(data: &[u8; 512]) -> Result<NvmeHealth, Error> {
    let kelvin = u16::from_le_bytes([data[1], data[2]]);
    let hours = u128::from_le_bytes(
        data[128..144]
            .try_into()
            .expect("invariant: fixed NVMe field"),
    );
    let power_on_hours = u64::try_from(hours).map_err(|_| {
        Error::invalid(
            "read NVMe SMART health log",
            "power-on hours exceed the observation contract",
        )
    })?;
    Ok(NvmeHealth {
        critical_warning: data[0],
        temperature_celsius: (kelvin != 0).then_some(i32::from(kelvin) - 273),
        power_on_hours,
    })
}

#[cfg(test)]
#[path = "../../../../test/platform/unit/disk_smart/nvme.rs"]
mod tests;
