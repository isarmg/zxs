//! One resumable raw file on the already qualified DATA. Stable births
//! identify it across process cuts; live original handles fence each exchange.
use super::{
    native::{birth, open_payload_writer, require_owner, require_same},
    record::{Birth, Phase, Record, valid_raw_extent},
};
use crate::{
    container_store::{hex, invalid, volume::DataVolume},
    fs::{HeldDir, OpaquePin},
};
use std::{ffi::CString, fs::File, io, os::unix::fs::FileExt};

fn leaf(record: &Record) -> io::Result<CString> {
    CString::new(format!("u-{}", hex(record.intent.upload_id()?)))
        .map_err(|_| invalid("invalid resumed DATA staging name"))
}

fn on_volume(volume: &DataVolume, record: &Record) -> io::Result<()> {
    if volume.token != record.data_token {
        return Err(invalid("upload control names another DATA"));
    }
    custody(volume)
}

pub(super) fn custody(volume: &DataVolume) -> io::Result<()> {
    volume.require_named()?;
    require_owner(volume.root(), c"owner", volume.upload_owner())
}

pub(super) fn available(volume: &DataVolume) -> io::Result<u64> {
    custody(volume)?;
    // A missing feature excludes just this candidate in the upload lane.
    birth(&volume.staging().file)?;
    birth(volume.upload_owner())?;
    volume.available_bytes()
}

pub(super) fn require_unused(volume: &DataVolume, record: &Record) -> io::Result<()> {
    custody(volume)?;
    volume.staging().confirm_absence(&leaf(record)?)
}

pub(super) struct Stage<'a> {
    volume: &'a DataVolume,
    name: CString,
    directory: HeldDir,
    pin: OpaquePin,
    directory_birth: Birth,
    payload_birth: Birth,
}

pub(super) struct Custody {
    directory: HeldDir,
    payload: File,
}

impl Custody {
    pub(super) fn require(&self, current: &Stage<'_>) -> io::Result<()> {
        current.require()?;
        if self.directory.entity != current.directory.entity
            || !self.directory.same_mount(&current.directory)?
        {
            return Err(invalid("upload DATA changed while reading the request"));
        }
        self.directory.require_named()?;
        require_same(&current.directory, &self.payload, &current.pin.file)
    }
}

impl<'a> Stage<'a> {
    /// The exact same already-selected Preparing record can finish only an
    /// empty qualified candidate. Nonempty or foreign members stay untouched.
    pub(super) fn prepare(
        volume: &'a DataVolume,
        record: &Record,
        authorize: impl Fn() -> io::Result<()>,
    ) -> io::Result<Self> {
        if record.phase != Phase::Preparing {
            return Err(invalid("DATA creation requires an original preparation"));
        }
        on_volume(volume, record)?;
        let name = leaf(record)?;
        let parent = volume.staging();
        let directory = if let Some(original) = parent.child_directory(&name)? {
            original
        } else {
            authorize()?;
            parent.new_child_directory(&name)?
        };
        directory.only(Some(c"payload"))?;
        if directory.opaque_pin(c"payload")?.is_none() {
            authorize()?;
            directory.create_opaque_record(c"payload")?;
        }
        let stage = Self::capture(volume, name, directory)?;
        if stage.pin.file.metadata()?.len() != 0 {
            return Err(invalid("unconfirmed DATA preparation contains bytes"));
        }
        stage.pin.file.sync_all()?;
        stage.directory.file.sync_all()?;
        parent.file.sync_all()?;
        stage.require()?;
        Ok(stage)
    }

    pub(super) fn open(volume: &'a DataVolume, record: &Record) -> io::Result<Self> {
        let Phase::Ready {
            directory,
            payload,
            offset,
        } = record.phase
        else {
            return Err(invalid("unavailable preparing upload checkpoint"));
        };
        on_volume(volume, record)?;
        let name = leaf(record)?;
        let original = volume
            .staging()
            .child_directory(&name)?
            .ok_or_else(|| invalid("original ready DATA stage is absent"))?;
        let stage = Self::capture(volume, name, original)?;
        if stage.directory_birth != directory
            || stage.payload_birth != payload
            || !valid_raw_extent(
                record.intent.body.length,
                offset,
                stage.pin.file.metadata()?.len(),
            )
        {
            return Err(invalid(
                "original DATA birth or extent differs from its checkpoint",
            ));
        }
        Ok(stage)
    }

    fn capture(volume: &'a DataVolume, name: CString, directory: HeldDir) -> io::Result<Self> {
        directory.only(Some(c"payload"))?;
        let pin = directory
            .opaque_pin(c"payload")?
            .ok_or_else(|| invalid("original staged raw file is absent"))?;
        let directory_birth = birth(&directory.file)?;
        let payload_birth = birth(&pin.file)?;
        if directory_birth.inode == payload_birth.inode {
            return Err(invalid("DATA directory and raw file share an inode"));
        }
        let stage = Self {
            volume,
            name,
            directory,
            pin,
            directory_birth,
            payload_birth,
        };
        stage.require()?;
        Ok(stage)
    }

    pub(super) fn retain(self) -> Custody {
        Custody {
            directory: self.directory,
            payload: self.pin.file,
        }
    }

    pub(super) fn phase(&self, offset: u64) -> Phase {
        Phase::Ready {
            directory: self.directory_birth,
            payload: self.payload_birth,
            offset,
        }
    }

    pub(super) fn require(&self) -> io::Result<()> {
        let parent = self.volume.staging();
        custody(self.volume)?;
        let named = parent
            .child_directory(&self.name)?
            .ok_or_else(|| invalid("original DATA stage lost its name"))?;
        if named.entity != self.directory.entity
            || !named.same_mount(&self.directory)?
            || birth(&self.directory.file)? != self.directory_birth
            || birth(&self.pin.file)? != self.payload_birth
        {
            return Err(invalid("original DATA stage or raw file was replaced"));
        }
        self.directory.only(Some(c"payload"))?;
        self.pin.require(&self.directory, c"payload")
    }

    /// After verified bytes are accepted, pwrite only that exact next extent.
    /// A later control record, not physical length, proves uploaded progress.
    pub(super) fn write(
        &mut self,
        at: u64,
        end: u64,
        bytes: &[u8],
        before_write: impl FnOnce() -> io::Result<()>,
        authorize: impl Fn() -> io::Result<()>,
    ) -> io::Result<()> {
        let length =
            u64::try_from(bytes.len()).map_err(|_| invalid("invalid upload block width"))?;
        if end.checked_sub(at) != Some(length) || length == 0 {
            return Err(invalid("invalid upload write extent"));
        }
        self.require()?;
        let file = open_payload_writer(&self.directory, &self.pin)?;
        before_write()?;
        self.require()?;
        authorize()?;
        file.write_all_at(bytes, at)?;
        file.sync_all()?;
        let current = self
            .directory
            .opaque_pin(c"payload")?
            .ok_or_else(|| invalid("raw DATA lost its name after sync"))?;
        require_same(&self.directory, &file, &current.file)?;
        if current.file.metadata()?.len() != end {
            return Err(invalid("raw DATA ended at an unexpected extent"));
        }
        self.pin = current;
        if !self.same_bytes(at, bytes, authorize)? {
            return Err(invalid("synchronized DATA differs from the accepted block"));
        }
        Ok(())
    }

    pub(super) fn same_bytes(
        &self,
        mut at: u64,
        bytes: &[u8],
        authorize: impl Fn() -> io::Result<()>,
    ) -> io::Result<bool> {
        self.require()?;
        let mut matched = true;
        let mut buffered = [0; 4096];
        for piece in bytes.chunks(buffered.len()) {
            authorize()?;
            let read = &mut buffered[..piece.len()];
            self.pin.file.read_exact_at(read, at)?;
            matched &= read == piece;
            at = at
                .checked_add(piece.len() as u64)
                .ok_or_else(|| invalid("upload verification extent overflow"))?;
        }
        self.require()?;
        authorize()?;
        Ok(matched)
    }
}
