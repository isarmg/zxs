//! One resumable raw file on the already qualified DATA. Stable births
//! identify it across process cuts; live original handles fence each exchange.
use super::{
    native::{birth, move_fresh, open_payload_writer, require_owner, require_same},
    record::{Birth, Intent, Phase, Record, valid_raw_extent},
};
use crate::{
    container_store::{Conflict, Error, Receipt, Store, codec, hex, invalid, volume::DataVolume},
    fs::{HeldDir, OPAQUE_STAMP_BYTES, OpaquePin, ReadAccess},
};
use std::{
    ffi::{CStr, CString},
    fs::File,
    io::{self, Read},
    os::unix::fs::FileExt,
};

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
    require_owner(volume.root(), c"owner", volume.bound_owner())
}

pub(super) fn available(volume: &DataVolume) -> io::Result<u64> {
    custody(volume)?;
    // A missing feature excludes just this candidate in the upload lane.
    birth(&volume.staging().file)?;
    birth(volume.bound_owner())?;
    volume.available_bytes()
}

pub(super) fn require_unused(volume: &DataVolume, record: &Record) -> io::Result<()> {
    custody(volume)?;
    volume.staging().confirm_absence(&leaf(record)?)
}

struct DraftReceipt {
    pin: OpaquePin,
    value: Receipt,
}

impl DraftReceipt {
    fn read(directory: &HeldDir) -> io::Result<Option<Self>> {
        let Some(pin) = directory.opaque_pin(c"receipt")? else {
            return Ok(None);
        };
        let mut bytes = Vec::new();
        (&pin.file).take(1025).read_to_end(&mut bytes)?;
        pin.require(directory, c"receipt")?;
        if bytes.len() > 1024 {
            return Err(invalid("provisional upload receipt too long"));
        }
        let value = codec::decode(&bytes)?;
        if value.sequence == 0 || codec::encode(&value)? != bytes {
            return Err(invalid("unqualified provisional upload receipt"));
        }
        Ok(Some(Self { pin, value }))
    }

    fn require_intent(&self, intent: &Intent) -> io::Result<()> {
        if self.value.mutation != intent.mutation || self.value.change != super::change(intent) {
            return Err(invalid(
                "provisional receipt differs from its original upload",
            ));
        }
        Ok(())
    }
}

fn layout(directory: &HeldDir, receipt: Option<&DraftReceipt>) -> io::Result<()> {
    if let Some(receipt) = receipt {
        directory.each_name(ReadAccess::Ordinary, |name| {
            if matches!(name, b"payload" | b"receipt") {
                Ok(())
            } else {
                Err(invalid("unknown member in provisional upload"))
            }
        })?;
        receipt.pin.require(directory, c"receipt")
    } else {
        directory.only(Some(c"payload"))
    }
}

pub(super) fn source_exists(volume: &DataVolume, record: &Record) -> io::Result<bool> {
    on_volume(volume, record)?;
    let present = volume.staging().pin_record(&leaf(record)?)?.is_some();
    volume.staging().require_named()?;
    Ok(present)
}

pub(super) struct Stage<'a> {
    volume: &'a DataVolume,
    name: CString,
    directory: HeldDir,
    pin: OpaquePin,
    directory_birth: Birth,
    payload_birth: Birth,
    draft: Option<DraftReceipt>,
}

pub(super) struct HeldBytes {
    custody: Custody,
    stamp: [u8; OPAQUE_STAMP_BYTES],
}

impl HeldBytes {
    pub(super) fn require(&self, staged: &Stage<'_>) -> io::Result<()> {
        self.custody.require(staged)?;
        if self.stamp != staged.pin.stamp() {
            return Err(invalid(
                "raw upload bytes differ from the original full proof",
            ));
        }
        Ok(())
    }
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
        let stage = Self::capture(volume, name, directory, None)?;
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
        let complete = (offset == record.intent.body.length).then_some(&record.intent);
        let stage = Self::capture(volume, name, original, complete)?;
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

    fn capture(
        volume: &'a DataVolume,
        name: CString,
        directory: HeldDir,
        completed_intent: Option<&Intent>,
    ) -> io::Result<Self> {
        let draft = match completed_intent {
            Some(intent) => {
                let draft = DraftReceipt::read(&directory)?;
                if let Some(receipt) = &draft {
                    receipt.require_intent(intent)?;
                }
                draft
            }
            None => None,
        };
        layout(&directory, draft.as_ref())?;
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
            draft,
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
        layout(&self.directory, self.draft.as_ref())?;
        self.pin.require(&self.directory, c"payload")
    }

    pub(super) fn held_bytes(&self) -> io::Result<HeldBytes> {
        self.require()?;
        let retained = HeldBytes {
            custody: Custody {
                directory: HeldDir {
                    file: self.directory.file.try_clone()?,
                    path: self.directory.path.clone(),
                    entity: self.directory.entity,
                },
                payload: self.pin.file.try_clone()?,
            },
            stamp: self.pin.stamp(),
        };
        retained.require(self)?;
        Ok(retained)
    }

    pub(super) fn read_held_at(&self, target: &mut [u8], offset: u64) -> io::Result<usize> {
        self.pin.file.read_at(target, offset)
    }

    #[cfg(test)]
    pub(super) fn verify_whole(
        &self,
        body: &crate::container_store::Body,
        authorize: impl Fn() -> io::Result<()>,
    ) -> Result<(), Error> {
        authorize()?;
        self.require()?;
        body.copy_exact(&mut &self.pin.file, &mut io::sink(), &authorize)?;
        self.require()?;
        authorize()?;
        Ok(())
    }

    pub(super) fn prepare_receipt(
        &mut self,
        store: &Store,
        expected: &Receipt,
        prove: &impl Fn() -> io::Result<()>,
        authorize: &impl Fn() -> io::Result<()>,
    ) -> Result<(), Error> {
        prove()?;
        self.require()?;
        authorize()?;
        self.pin.file.sync_all()?;
        if let Some(draft) = &self.draft {
            if draft.value == *expected {
                draft.pin.file.sync_all()?;
                self.directory.file.sync_all()?;
                self.require()?;
                return Ok(());
            }
            if draft.value.mutation == expected.mutation && draft.value.change == expected.change {
                return Err(Conflict::SequenceChanged(expected.sequence).into());
            }
            return Err(invalid("unrecognized provisional receipt cannot be replaced").into());
        }
        self.install_receipt(store, expected, prove, authorize)?;
        self.require()?;
        Ok(())
    }

    fn install_receipt(
        &mut self,
        store: &Store,
        expected: &Receipt,
        prove: &impl Fn() -> io::Result<()>,
        authorize: &impl Fn() -> io::Result<()>,
    ) -> Result<(), Error> {
        prove()?;
        self.require()?;
        authorize()?;
        let mut transient = store.stage(self.volume)?;
        transient.write(c"receipt", &codec::encode(expected)?)?;
        let original = transient.held_receipt()?;
        move_fresh(
            &transient.directory,
            c"receipt",
            &self.directory,
            c"receipt",
            || {
                prove()?;
                self.require()?;
                original.require(&transient.directory, c"receipt")
            },
            authorize,
        )?;
        let now = DraftReceipt::read(&self.directory)?
            .ok_or_else(|| invalid("provisional upload receipt disappeared"))?;
        require_same(&self.directory, &original.file, &now.pin.file)?;
        if now.value != *expected {
            return Err(invalid("installed provisional receipt differs from its original").into());
        }
        self.draft = Some(now);
        self.directory.file.sync_all()?;
        transient.directory.file.sync_all()?;
        Ok(())
    }

    pub(super) fn require_prepared(&self) -> io::Result<()> {
        if self.draft.is_none() {
            return Err(invalid("upload requires its exact provisional receipt"));
        }
        self.require()
    }

    pub(super) fn choose(
        &self,
        target: &HeldDir,
        name: &CStr,
        prove: &impl Fn() -> io::Result<()>,
        authorize: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        if self.draft.is_none() {
            return Err(invalid("upload requires an exact provisional receipt"));
        }
        move_fresh(
            self.volume.staging(),
            &self.name,
            target,
            name,
            || {
                prove()?;
                self.require()
            },
            authorize,
        )
    }

    pub(super) fn confirm_moved(&self, target: &HeldDir, name: &CStr) -> io::Result<()> {
        let draft = self
            .draft
            .as_ref()
            .ok_or_else(|| invalid("published upload receipt is not held"))?;
        custody(self.volume)?;
        let selected = target
            .child_directory(name)?
            .ok_or_else(|| invalid("published upload entry lost its new name"))?;
        if selected.entity != self.directory.entity
            || !selected.same_mount(&self.directory)?
            || birth(&selected.file)? != self.directory_birth
        {
            return Err(invalid(
                "published upload differs from its original directory",
            ));
        }
        layout(&selected, Some(draft))?;
        self.pin.require(&selected, c"payload")?;
        self.volume.staging().confirm_absence(&self.name)?;
        target.require_named()?;
        selected.require_named()
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
