use super::{BlockDevice, CommandTimeout, Error};
use std::{ffi::c_void, os::fd::AsRawFd};

const SG_IO: u32 = 0x2285;
const SG_DXFER_NONE: i32 = -1;
const SG_DXFER_FROM_DEV: i32 = -3;

// Linux <scsi/sg.h> sg_io_hdr, native amd64 ABI. No scatter/gather, direct I/O,
// async completion, outbound data or arbitrary SCSI command is exposed.
#[repr(C)]
struct SgIo {
    interface_id: i32,
    direction: i32,
    command_len: u8,
    sense_capacity: u8,
    iovec_count: u16,
    data_len: u32,
    data: *mut c_void,
    command: *mut u8,
    sense: *mut u8,
    timeout_ms: u32,
    flags: u32,
    pack_id: i32,
    user: *mut c_void,
    status: u8,
    masked_status: u8,
    message_status: u8,
    sense_len: u8,
    host_status: u16,
    driver_status: u16,
    residual: i32,
    duration: u32,
    info: u32,
}

const _: () = assert!(std::mem::size_of::<SgIo>() == 88);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AtaPowerMode {
    Ready,
    LowPower,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AtaHealth {
    Passed,
    Failed,
    /// The command completed without return registers. smartctl may derive
    /// status from the captured attributes; malformed signatures are errors.
    RegistersUnavailable,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AtaData {
    Identify,
    Attributes,
    Thresholds,
}

#[derive(Clone, Copy)]
enum Command {
    PowerMode,
    Health,
    Data(AtaData),
}

impl Command {
    fn operation(self) -> &'static str {
        match self {
            Self::PowerMode => "check ATA power mode",
            Self::Health => "read ATA SMART return status",
            Self::Data(AtaData::Identify) => "identify ATA device",
            Self::Data(AtaData::Attributes) => "read ATA SMART attributes",
            Self::Data(AtaData::Thresholds) => "read ATA SMART thresholds",
        }
    }

    fn cdb(self) -> [u8; 16] {
        let mut cdb = [0; 16];
        cdb[0] = 0x85; // SAT ATA PASS-THROUGH (16), matching smartctl -d sat.
        cdb[1] = if matches!(self, Self::Data(_)) {
            4 << 1
        } else {
            3 << 1
        };
        cdb[2] = if matches!(self, Self::Data(_)) {
            0x0e
        } else {
            0x2c
        };
        match self {
            Self::PowerMode => cdb[14] = 0xe5,
            Self::Data(AtaData::Identify) => {
                cdb[6] = 1;
                cdb[14] = 0xec;
            }
            Self::Health | Self::Data(AtaData::Attributes | AtaData::Thresholds) => {
                cdb[10] = 0x4f;
                cdb[12] = 0xc2;
                cdb[14] = 0xb0;
                cdb[4] = match self {
                    Self::Health => 0xda,
                    Self::Data(AtaData::Attributes) => 0xd0,
                    Self::Data(AtaData::Thresholds) => 0xd1,
                    _ => unreachable!("invariant: matched SMART command"),
                };
                if matches!(self, Self::Data(_)) {
                    cdb[6] = 1;
                }
                if matches!(self, Self::Data(AtaData::Thresholds)) {
                    cdb[8] = 1;
                }
            }
        }
        cdb
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Registers {
    error: u8,
    status: u8,
    count: u8,
    lba_mid: u8,
    lba_high: u8,
}

pub fn ata_power_mode(
    device: BlockDevice<'_>,
    timeout: CommandTimeout,
) -> Result<AtaPowerMode, Error> {
    power_mode(execute(device, Command::PowerMode, None, timeout)?)
}

fn power_mode(registers: Option<Registers>) -> Result<AtaPowerMode, Error> {
    let operation = Command::PowerMode.operation();
    let registers =
        registers.ok_or_else(|| Error::invalid(operation, "power mode return register missing"))?;
    match registers.count {
        0x00 | 0x01 | 0x40 => Ok(AtaPowerMode::LowPower),
        0x41 | 0x80..=0x83 | 0xff => Ok(AtaPowerMode::Ready),
        _ => Err(Error::invalid(
            operation,
            "unknown power mode; query skipped",
        )),
    }
}

pub fn ata_health(device: BlockDevice<'_>, timeout: CommandTimeout) -> Result<AtaHealth, Error> {
    health_status(execute(device, Command::Health, None, timeout)?)
}

fn health_status(registers: Option<Registers>) -> Result<AtaHealth, Error> {
    match registers.map(|value| (value.lba_mid, value.lba_high)) {
        Some((0x4f, 0xc2)) => Ok(AtaHealth::Passed),
        Some((0xf4, 0x2c)) => Ok(AtaHealth::Failed),
        None => Ok(AtaHealth::RegistersUnavailable),
        Some(_) => Err(Error::invalid(
            Command::Health.operation(),
            "invalid SMART status signature",
        )),
    }
}

pub fn ata_data(
    device: BlockDevice<'_>,
    kind: AtaData,
    timeout: CommandTimeout,
) -> Result<[u8; 512], Error> {
    let mut data = [0; 512];
    execute(device, Command::Data(kind), Some(&mut data), timeout)?;
    Ok(data)
}

#[allow(unsafe_code)]
fn execute(
    device: BlockDevice<'_>,
    command: Command,
    data: Option<&mut [u8; 512]>,
    timeout: CommandTimeout,
) -> Result<Option<Registers>, Error> {
    let mut cdb = command.cdb();
    let mut sense = [0; 64];
    let (pointer, size) = match data {
        Some(data) => (data.as_mut_ptr().cast(), 512),
        None => (std::ptr::null_mut(), 0),
    };
    let mut header = SgIo {
        interface_id: i32::from(b'S'),
        direction: if size == 0 {
            SG_DXFER_NONE
        } else {
            SG_DXFER_FROM_DEV
        },
        command_len: 16,
        sense_capacity: 64,
        iovec_count: 0,
        data_len: size,
        data: pointer,
        command: cdb.as_mut_ptr(),
        sense: sense.as_mut_ptr(),
        timeout_ms: timeout.0,
        flags: 0,
        pack_id: 0,
        user: std::ptr::null_mut(),
        status: 0,
        masked_status: 0,
        message_status: 0,
        sense_len: 0,
        host_status: 0,
        driver_status: 0,
        residual: 0,
        duration: 0,
        info: 0,
    };
    // SAFETY: repr(C) matches Linux amd64 sg_io_hdr (size asserted). All pointer
    // targets are initialized, correctly sized, exclusively borrowed and live
    // until this synchronous block-device SG_IO returns. Null data is used only
    // with length zero. Fixed private CDBs can only read and never submit async
    // work; the kernel completes/unmaps the request before returning.
    let result = unsafe { libc::ioctl(device.0.as_raw_fd(), SG_IO as _, &mut header) };
    if result == -1 {
        return Err(Error::io(command.operation()));
    }
    completion(&header, &sense, command.operation())
}

fn completion(
    header: &SgIo,
    sense: &[u8; 64],
    operation: &'static str,
) -> Result<Option<Registers>, Error> {
    if header.host_status != 0
        || !matches!(header.driver_status, 0 | 8)
        || !matches!(header.status, 0 | 2)
    {
        return Err(Error::ScsiStatus {
            operation,
            status: header.status,
            host: header.host_status,
            driver: header.driver_status,
        });
    }
    if header.residual != 0 {
        return Err(Error::invalid(operation, "incomplete data transfer"));
    }
    let sense = sense
        .get(..usize::from(header.sense_len))
        .ok_or_else(|| Error::invalid(operation, "sense length exceeds buffer"))?;
    if sense.is_empty() {
        return if header.status == 0 {
            Ok(None)
        } else {
            Err(Error::invalid(operation, "check condition without sense"))
        };
    }
    let registers = parse_sense(sense, operation)?;
    if let Some(registers) = registers
        && registers.status & 0xa9 != 0
    {
        return Err(Error::AtaStatus {
            operation,
            status: registers.status,
            error: registers.error,
        });
    }
    Ok(registers)
}

fn parse_sense(sense: &[u8], operation: &'static str) -> Result<Option<Registers>, Error> {
    if sense.len() < 8 || usize::from(sense[7]) + 8 > sense.len() {
        return Err(Error::invalid(operation, "truncated sense response"));
    }
    let sense = &sense[..usize::from(sense[7]) + 8];
    match sense[0] & 0x7f {
        0x72 => {
            if !matches!(sense[1] & 0x0f, 0 | 1) || sense[2] != 0 || !matches!(sense[3], 0 | 0x1d) {
                return Err(Error::ScsiSense {
                    operation,
                    key: sense[1] & 0x0f,
                    asc: sense[2],
                    ascq: sense[3],
                });
            }
            let mut descriptors = &sense[8..];
            let mut registers = None;
            while !descriptors.is_empty() {
                if descriptors.len() < 2 || usize::from(descriptors[1]) + 2 > descriptors.len() {
                    return Err(Error::invalid(operation, "truncated sense descriptor"));
                }
                let (descriptor, rest) = descriptors.split_at(usize::from(descriptors[1]) + 2);
                if descriptor[0] == 9 {
                    if descriptor.len() != 14 || registers.is_some() {
                        return Err(Error::invalid(operation, "invalid ATA return descriptor"));
                    }
                    registers = Some(Registers {
                        error: descriptor[3],
                        status: descriptor[13],
                        count: descriptor[5],
                        lba_mid: descriptor[9],
                        lba_high: descriptor[11],
                    });
                }
                descriptors = rest;
            }
            Ok(registers)
        }
        0x70 => {
            if sense.len() < 14 {
                return Err(Error::invalid(operation, "truncated fixed sense"));
            }
            if !matches!(sense[2] & 0x0f, 0 | 1) || sense[12] != 0 || !matches!(sense[13], 0 | 0x1d)
            {
                return Err(Error::ScsiSense {
                    operation,
                    key: sense[2] & 0x0f,
                    asc: sense[12],
                    ascq: sense[13],
                });
            }
            if sense[13] != 0x1d {
                return Ok(None);
            }
            Ok(Some(Registers {
                error: sense[3],
                status: sense[4],
                count: sense[6],
                lba_mid: sense[10],
                lba_high: sense[11],
            }))
        }
        _ => Err(Error::invalid(
            operation,
            "not a current SCSI sense response",
        )),
    }
}

#[cfg(test)]
#[path = "../../../../test/platform/unit/disk_smart/sat.rs"]
mod tests;
