use crate::{inspect::Input, pinned::invalid};
use rustix::fs::{MemfdFlags, SealFlags, fcntl_add_seals, fcntl_get_seals, memfd_create};
use sha2::{Digest, Sha256};
use std::{
    fs::File,
    io::{self, Read, Seek, Write},
    os::unix::fs::FileExt,
    path::Path,
};

const MAX: u64 = 128 << 20;
fn unsupported(why: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::Unsupported, why)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct Geometry {
    pub(super) length: u64,
    pub(super) sector_bytes: u16,
    pub(super) data_clusters: u32,
}
fn geometry(b: &[u8; 512], length: u64) -> io::Result<Geometry> {
    let u16le = |at| u16::from_le_bytes([b[at], b[at + 1]]);
    let u32le = |at| u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
    let bs = u16le(11);
    let cluster = u64::from(b[13]);
    let reserved = u64::from(u16le(14));
    let copies = b[16];
    let sectors = u64::from(u32le(32));
    let fat = u64::from(u32le(36));
    let root = u32le(44);
    if b[510..] != [0x55, 0xaa]
        || !matches!(bs, 512 | 1024 | 2048 | 4096)
        || !cluster.is_power_of_two()
        || cluster > 128
        || cluster * u64::from(bs) > 32768
        || reserved == 0
        || !matches!(copies, 1 | 2)
        || u16le(17) != 0
        || u16le(19) != 0
        || u16le(22) != 0
        || u16le(42) != 0
        || fat == 0
        || sectors * u64::from(bs) != length
    {
        return Err(unsupported(
            "not a supported complete canonical FAT32 volume image",
        ));
    }
    let metadata = reserved + u64::from(copies) * fat;
    let clusters = sectors
        .checked_sub(metadata)
        .ok_or_else(|| invalid("invalid FAT allocation extents"))?
        / cluster;
    // FAT type is determined by cluster count, NOT the printable filesystem label.
    let unsupported_clusters = || unsupported("invalid or unsupported FAT32 cluster geometry");
    if !(65525..=0x0fff_ffed).contains(&clusters)
        || fat * u64::from(bs) / 4 < clusters + 2
        || root < 2
        || u64::from(root) > clusters + 1
    {
        return Err(unsupported_clusters());
    }
    let data_clusters = u32::try_from(clusters).map_err(|_| unsupported_clusters())?;
    Ok(Geometry {
        length,
        sector_bytes: bs,
        data_clusters,
    })
}

// The selected FAT allocation layout and extended boot volume ID cannot be
// replaced by another image with a coincidentally matching size. Leave the
// boot labels, state bits and backup/FSInfo locations to the genuine checker.
#[derive(Clone, Copy)]
pub(super) struct FatRepairFrame {
    pub(super) view: Geometry,
    bpb: [u8; 42],
}
pub(super) type FatRepairImage = super::leased::Image<FatRepairFrame>;
pub(super) fn fat_repair_frame(file: &File, length: u64) -> io::Result<FatRepairFrame> {
    let mut boot = [0; 512];
    file.read_exact_at(&mut boot, 0)?;
    work_frame(&boot, length)
}
fn work_frame(boot: &[u8; 512], length: u64) -> io::Result<FatRepairFrame> {
    let view = geometry(boot, length)?;
    let mut bpb = [0; 42];
    bpb[..37].copy_from_slice(&boot[11..48]);
    bpb[37..].copy_from_slice(&boot[66..71]);
    Ok(FatRepairFrame { view, bpb })
}
impl FatRepairFrame {
    pub(super) fn same_volume(self, reopened: Self) -> bool {
        self.view == reopened.view && self.bpb == reopened.bpb
    }
}

// Only a complete capture can construct a checker operand. No inherited device,
// arbitrary File or unsealed bytes can be promoted by the launcher.
pub(super) struct Sealed {
    fd: File,
    geometry: Geometry,
}
fn seals() -> SealFlags {
    SealFlags::WRITE | SealFlags::GROW | SealFlags::SHRINK | SealFlags::SEAL
}
impl Sealed {
    pub(super) fn geometry(&self) -> Geometry {
        self.geometry
    }
    pub(super) fn file(&self) -> &File {
        &self.fd
    }
    pub(super) fn capture(path: &Path, expected: &[u8; 32]) -> io::Result<Self> {
        capture(path, expected, 512, Self::from_sealed)
    }
    pub(super) fn from_sealed(copy: File, expected: &[u8; 32]) -> io::Result<Self> {
        let (fd, boot, len) = verify_copy(copy, expected, 512)?;
        Ok(Self {
            fd,
            geometry: geometry(&boot, len)?,
        })
    }
}
pub(super) struct ExfatSealed {
    fd: File,
    frame: super::exfat::Framing,
}
impl ExfatSealed {
    pub(super) fn capture(path: &Path, expected: &[u8; 32]) -> io::Result<Self> {
        capture(path, expected, 1 << 20, Self::from_sealed)
    }
    pub(super) fn from_sealed(copy: File, expected: &[u8; 32]) -> io::Result<Self> {
        let (fd, boot, len) = verify_copy(copy, expected, 1 << 20)?;
        Ok(Self {
            fd,
            frame: super::exfat::framing(&boot, len)?,
        })
    }
    pub(super) fn file(&self) -> &File {
        &self.fd
    }
    pub(super) fn frame(&self) -> super::exfat::Framing {
        self.frame
    }
}
// Both formats share source selection, the independently sealed memfd and a full
// second hash pass. Neither public operation can supply a raw device/File here.
fn capture<T>(
    path: &Path,
    expected: &[u8; 32],
    minimum: u64,
    finish: fn(File, &[u8; 32]) -> io::Result<T>,
) -> io::Result<T> {
    let source = Input::select_in(path, minimum..=MAX)?;
    let length = source.len();
    let mut original = source.reader()?;
    let mut copy = File::from(memfd_create(
        c"helper-check-image",
        MemfdFlags::CLOEXEC | MemfdFlags::ALLOW_SEALING | MemfdFlags::NOEXEC_SEAL,
    )?);
    let mut buf = [0_u8; 32768];
    let mut remaining = length;
    while remaining != 0 {
        let n = usize::try_from(remaining)
            .unwrap_or(usize::MAX)
            .min(buf.len());
        original.read_exact(&mut buf[..n])?;
        copy.write_all(&buf[..n])?;
        remaining -= n as u64;
    }
    eof(&mut original)?;
    source.recheck(&original)?;
    fcntl_add_seals(&copy, seals())?;
    if copy.metadata()?.len() != length {
        return Err(invalid("sealed copy length mismatch"));
    }
    let result = finish(copy, expected)?;
    source.recheck(&original)?;
    Ok(result)
}

// A fixed panel worker may also receive this anonymous tmpfs file on stdin.
// Reject regular named files, devices, pipes and incomplete seals before reading.
fn verify_copy(
    mut copy: File,
    expected: &[u8; 32],
    min: u64,
) -> io::Result<(File, [u8; 512], u64)> {
    use std::os::unix::fs::MetadataExt;
    let meta = copy.metadata()?;
    let length = meta.len();
    if !meta.is_file() || meta.nlink()!=0 || meta.mode()&0o111!=0 || !(min..=MAX).contains(&length)
        || rustix::fs::fstatfs(&copy)?.f_type!=0x0102_1994 // Linux TMPFS_MAGIC (memfd)
        || !fcntl_get_seals(&copy)?.contains(seals()|SealFlags::EXEC)
    {
        return Err(invalid(
            "expected a complete, non-executable, write-sealed anonymous image",
        ));
    }
    copy.rewind()?;
    let mut buf = [0u8; 32768];
    let mut hash = Sha256::new();
    let mut left = length;
    while left != 0 {
        let n = usize::try_from(left).unwrap_or(usize::MAX).min(buf.len());
        copy.read_exact(&mut buf[..n])?;
        hash.update(&buf[..n]);
        left -= n as u64;
    }
    eof(&mut copy)?;
    let actual: [u8; 32] = hash.finalize().into();
    if actual != *expected {
        return Err(invalid("sealed image differs from the supplied SHA-256"));
    }
    let mut boot = [0u8; 512];
    copy.read_exact_at(&mut boot, 0)?;
    copy.rewind()?;
    Ok((copy, boot, length))
}
fn eof(f: &mut impl Read) -> io::Result<()> {
    let mut byte = [0; 1];
    loop {
        match f.read(&mut byte) {
            Ok(0) => return Ok(()),
            Ok(_) => return Err(invalid("image length changed")),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
}

#[cfg(test)]
#[path = "../../tests/fs_tools/fat32_image.rs"]
mod tests;

#[cfg(test)]
mod repair_tests {
    use super::*;
    #[test]
    fn matching_size_or_counts_do_not_replace_the_known_fat_volume() {
        let mut boot = [0u8; 512];
        let sectors = 80_000u32;
        let len = u64::from(sectors) * 512;
        boot[11..13].copy_from_slice(&512u16.to_le_bytes());
        boot[13] = 1;
        boot[14..16].copy_from_slice(&32u16.to_le_bytes());
        boot[16] = 2;
        boot[32..36].copy_from_slice(&sectors.to_le_bytes());
        boot[36..40].copy_from_slice(&640u32.to_le_bytes());
        boot[44] = 2;
        boot[66] = 0x29;
        boot[510..].copy_from_slice(&[0x55, 0xaa]);
        let original = work_frame(&boot, len).unwrap();
        boot[65] = 1;
        boot[71] = b'L';
        assert!(original.same_volume(work_frame(&boot, len).unwrap())); // state / label, not identity
        boot[67] = 1;
        assert!(!original.same_volume(work_frame(&boot, len).unwrap())); // volume ID
        boot[67] = 0;
        boot[44] = 3;
        assert!(!original.same_volume(work_frame(&boot, len).unwrap())); // ordinary root's layout
        assert!(work_frame(&boot, len + 512).is_err());
    }
}
