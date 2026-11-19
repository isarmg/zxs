//! The two fixed ordinary feedback files. No user-supplied name, path or bytes
//! from unknown media are ever published through this helper.
use super::*;

#[derive(Copy, Clone)]
pub(super) enum Output { Floor, Marker }

impl Output {
    fn name(self) -> &'static [u8] {
        match self { Self::Floor => FLOOR, Self::Marker => ROOT_SLOT }
    }

    pub(super) fn create(self, dir: &File) -> io::Result<File> {
        Ok(File::from(openat(dir, OsStr::from_bytes(self.name()),
            OFlags::CREATE | OFlags::EXCL | OFlags::RDWR | OFlags::NOFOLLOW | OFlags::CLOEXEC,
            Mode::RUSR | Mode::WUSR)?))
    }

    /// After the caller verified the exact raw spelling and inventory, check
    /// the just-created original again before its first write. Never repair it.
    pub(super) fn require_new(self, dir: &File, parent: &Stamp, fd: &File) -> io::Result<()> {
        let fresh=Stamp::on(fd)?;
        if !fresh.kind.is_file() || fresh.mode&0o7777!=0o600 || fresh.uid!=parent.uid
            || fresh.links!=1 || fresh.len!=0 || fresh.mount!=parent.mount
            || fresh.entity.dev!=parent.entity.dev || Stamp::on(&pin(dir,self.name())?)?!=fresh {
            return Err(bad("exclusive feedback name does not refer to our new empty private file; preserved"));
        }
        Ok(())
    }

    /// Sync the same actual file, then its original parent, before opening
    /// an independent read descriptor through its still exact name.
    pub(super) fn sync_and_readback(self, dir: &File, uid: u32, original: &File, bytes: &[u8]) -> io::Result<Record> {
        let parent=Stamp::root(dir,uid)?;
        let file=Stamp::on(original)?;
        if Stamp::on(&pin(dir,self.name())?)?!=file {
            return Err(bad("synchronization would concern a different named original; preserved"));
        }
        original.sync_all()?;
        dir.sync_all()?;
        self.readback(dir,uid,original,bytes,&parent,&file)
    }

    fn readback(self, dir: &File, uid: u32, original: &File, bytes: &[u8], parent: &Stamp, file: &Stamp)
        -> io::Result<Record> {
        if Stamp::root(dir,uid)?!=*parent || Stamp::on(original)?!=*file {
            return Err(bad("feedback or original parent changed after its synchronization cut; preserved"));
        }
        let result=Record::open(dir,parent,self.name())?;
        if result.original!=*file || result.bytes()?.as_slice()!=bytes {
            return Err(bad("feedback readback differs from the synchronized original; preserved"));
        }
        result.require_name(dir)?;
        if Stamp::root(dir,uid)?!=*parent || Stamp::on(original)?!=*file {
            return Err(bad("feedback or original parent changed during synchronized readback; preserved"));
        }
        Ok(result)
    }
}

#[cfg(test)]
#[path = "../../../../tests/portable/feedback_file.rs"]
mod tests;
