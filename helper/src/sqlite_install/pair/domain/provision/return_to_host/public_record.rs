//! Exact protected root-owned public installation record, not approval alone.
use super::{bad, layout, live::Stamp};
use rustix::fs::{Mode, OFlags, ResolveFlags, openat2};
use std::{
    ffi::CStr,
    fs::File,
    io::{self, Read},
    ops::RangeInclusive,
    os::unix::fs::MetadataExt,
};

fn path(parent: &File, name: &CStr) -> io::Result<File> {
    Ok(File::from(openat2(
        parent,
        name,
        OFlags::PATH | OFlags::CLOEXEC | OFlags::NOFOLLOW,
        Mode::empty(),
        ResolveFlags::BENEATH
            | ResolveFlags::NO_XDEV
            | ResolveFlags::NO_MAGICLINKS
            | ResolveFlags::NO_SYMLINKS,
    )?))
}
pub(super) struct Public {
    pin: File,
    stamp: Stamp,
    value: Vec<u8>,
}
impl Public {
    fn read_value(pin: &File, stamp: Stamp, len: usize) -> io::Result<Vec<u8>> {
        if pin.metadata()?.gid() != 0 {
            return Err(bad(
                "not the original root-owned public installation record",
            ));
        }
        let f = crate::pinned::public_record_reader(pin, stamp.mount, len as u64)?;
        layout::bare_acl(&f, c"system.posix_acl_access")?;
        let mut b = Vec::with_capacity(len);
        (&f).take(len as u64 + 1).read_to_end(&mut b)?;
        if b.len() != len || Stamp::on(&f)? != stamp || Stamp::on(pin)? != stamp {
            return Err(bad("changed public installation record"));
        }
        Ok(b)
    }
    pub(super) fn select(
        parent: &File,
        name: &CStr,
        lengths: RangeInclusive<usize>,
    ) -> io::Result<Self> {
        let pin = path(parent, name)?;
        let stamp = Stamp::on(&pin)?;
        let len = usize::try_from(stamp.len)
            .map_err(|_| bad("invalid protected public installation record length"))?;
        if !lengths.contains(&len) || !stamp.at(&Stamp::on(parent)?, stamp.len, 0o444, 0) {
            return Err(bad("not the exact protected public installation object"));
        }
        let value = Self::read_value(&pin, stamp, len)?;
        let v = Self { pin, stamp, value };
        v.require(parent, name)?;
        Ok(v)
    }
    pub(super) fn bytes(&self) -> &[u8] {
        &self.value
    }
    pub(super) fn require(&self, parent: &File, name: &CStr) -> io::Result<()> {
        let same = || Stamp::on(&path(parent, name)?);
        if same()? != self.stamp
            || Stamp::on(&self.pin)? != self.stamp
            || Self::read_value(&self.pin, self.stamp, self.value.len())? != self.value
            || same()? != self.stamp
        {
            return Err(bad("original protected public record changed"));
        }
        Ok(())
    }
}
