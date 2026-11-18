use crate::pinned::{Entity, Kind, LocalDataFile, Pinned, invalid};
use std::fmt::Write as _;
use std::fs::{File, Metadata};
use std::io::{self, Read};
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

pub(crate) const MAX_DIRS: usize = 16;
const MAX_REPORT_BYTES: usize = 32 * 1024;
const MAX_COMPARE: u64 = 256 << 20;

pub(crate) fn directories(paths: &[PathBuf]) -> io::Result<String> {
    if !(1..=MAX_DIRS).contains(&paths.len()) { return Err(invalid("expected 1–16 directories")); }
    // One held leaf per operand, not a retained FD for every component of every path.
    let held: Vec<Pinned> = paths.iter().map(|p| Pinned::open(p, Kind::Directory)).collect::<io::Result<_>>()?;
    let mut report = format!("directories={}", held.len());
    for (i, dir) in held.iter().enumerate() {
        let id = dir.entity();
        write!(report, "\ndir[{i}] dev={} ino={}", id.dev, id.ino).expect("writing to String");
    }
    // At most 120 unordered pairs. Separate direction flags retain the actual observations
    // even when two different bind-mount spellings give unusual ancestry.
    for (j, right) in held.iter().enumerate() {
        for (i, left) in held[..j].iter().enumerate() {
            let a = left.entity();
            let b = right.entity();
            let ab = left.entity_is_visible_ancestor_of(right);
            let ba = right.entity_is_visible_ancestor_of(left);
            write!(report, "\npair[{i},{j}] same_entity={} same_st_dev={} first_visible_ancestor={} second_visible_ancestor={}",
                   a == b, a.dev == b.dev, ab, ba).expect("writing to String");
            if report.len() >= MAX_REPORT_BYTES { return Err(invalid("directory report exceeds output budget")); }
        }
    }
    for dir in &held { dir.require_same_path()?; }
    Ok(report)
}

#[derive(PartialEq, Eq)]
struct Stamp { entity: Entity, size: u64, mode: u32, links: u64, modified: (i64,i64), changed: (i64,i64) }
impl Stamp {
    fn of(meta: Metadata) -> io::Result<Self> {
        if !meta.is_file() || meta.nlink() != 1 || meta.len() > MAX_COMPARE { return Err(invalid("expected a single-linked regular file of at most 256 MiB")); }
        Ok(Self { entity: Entity::of(&meta)?, size: meta.len(), mode: meta.mode(), links: meta.nlink(),
            modified: (meta.mtime(), meta.mtime_nsec()), changed: (meta.ctime(), meta.ctime_nsec()) })
    }
}

struct Input { pinned: LocalDataFile, stamp: Stamp }
impl Input {
    fn select(path: &Path) -> io::Result<Self> {
        let pinned = LocalDataFile::select(path)?;
        let stamp = Stamp::of(pinned.metadata()?)?;
        Ok(Self { pinned, stamp })
    }
    fn reader(&self) -> io::Result<File> {
        let file = self.pinned.reader()?;
        if Stamp::of(file.metadata()?)? != self.stamp { return Err(invalid("file changed before the comparison")); }
        Ok(file)
    }
    fn recheck(&self, file: &File) -> io::Result<()> {
        self.pinned.require_same_path()?;
        if Stamp::of(file.metadata()?)? != self.stamp {
            return Err(invalid("file changed during comparison"));
        }
        Ok(())
    }
}

pub(crate) struct Comparison { pub left: u64, pub right: u64, pub equal: bool }
pub(crate) fn compare(left: &Path, right: &Path) -> io::Result<Comparison> {
    let a = Input::select(left)?;
    let b = Input::select(right)?;
    if a.stamp.entity == b.stamp.entity { return Err(invalid("reference and candidate are the same file")); }
    // Validate the filesystem and identity of BOTH operands without a read open or probe.
    let mut af = a.reader()?;
    let mut bf = b.reader()?;
    let (na, nb) = (a.stamp.size, b.stamp.size);
    let equal = exact_bytes(&mut af, na, &mut bf, nb)?;
    a.recheck(&af)?;
    b.recheck(&bf)?;
    Ok(Comparison { left: na, right: nb, equal })
}

fn exact_bytes(a: &mut impl Read, mut remaining_a: u64, b: &mut impl Read, mut remaining_b: u64) -> io::Result<bool> {
    let mut left = vec![0_u8; 64 * 1024];
    let mut right = vec![0_u8; left.len()];
    let mut same = remaining_a == remaining_b;
    // Validate both complete streams even when their lengths or an early byte differ.
    while remaining_a > 0 || remaining_b > 0 {
        let na = remaining_a.min(left.len() as u64) as usize;
        let nb = remaining_b.min(right.len() as u64) as usize;
        a.read_exact(&mut left[..na])?;
        b.read_exact(&mut right[..nb])?;
        same &= left[..na] == right[..nb];
        remaining_a -= na as u64;
        remaining_b -= nb as u64;
    }
    fn at_end(r: &mut impl Read) -> io::Result<bool> {
        let mut byte = [0_u8];
        loop {
            match r.read(&mut byte) {
                Ok(n) => return Ok(n == 0),
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) => return Err(e),
            }
        }
    }
    if !at_end(a)? || !at_end(b)? { return Err(invalid("actual bytes exceed the observed file size")); }
    Ok(same)
}

#[cfg(test)]
mod tests {
    use super::exact_bytes;
    use std::{fs, io::{self, Read}};
    #[test]
    fn source_size_change_and_replaced_name_never_become_a_completed_result() {
        let temp = crate::test_area::data_directory();
        let path = temp.path().join("input");
        fs::write(&path, b"before").unwrap();
        let input = super::Input::select(&path).unwrap();
        let reader = input.reader().unwrap();
        fs::write(&path, b"longer contents").unwrap();
        assert!(input.recheck(&reader).is_err());
        let other = super::Input::select(&path).unwrap();
        let reader = other.reader().unwrap();
        fs::rename(&path, temp.path().join("held-original")).unwrap();
        fs::write(&path, b"longer contents").unwrap();
        assert!(other.recheck(&reader).is_err());
    }
    struct Short<'a>(&'a [u8], usize);
    impl Read for Short<'_> {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let n = buf.len().min(self.1);
            self.0.read(&mut buf[..n])
        }
    }
    #[test]
    fn comparison_accepts_unequal_read_sizes_not_unequal_content_or_false_lengths() {
        let data = vec![42_u8; 70_003];
        let len = data.len() as u64;
        assert!(exact_bytes(&mut Short(&data, 7), len, &mut Short(&data, 19), len).unwrap());
        assert!(!exact_bytes(&mut &b"aa"[..], 2, &mut &b"a"[..], 1).unwrap());
        assert!(!exact_bytes(&mut &b"ab"[..], 2, &mut &b"ac"[..], 2).unwrap());
        assert!(exact_bytes(&mut &b"short"[..], 7, &mut &b"longer!"[..], 7).is_err());
        assert!(exact_bytes(&mut &b"bad"[..], 0, &mut &b"ok"[..], 2).is_err());
        assert!(exact_bytes(&mut &b"ab"[..], 2, &mut &b"cx!"[..], 2).is_err());
    }
}
