//! The exact selected SYSTEM UR1 is authority. Unselected candidates are never
//! used as progress, adopted, unlinked or overwritten by this journal.
use super::{
    native::{promote_control, require_same},
    record::{ChunkUse, MAX_RECORD, Phase, Record, chunk_use},
};
use crate::{
    container_store::{Id, hex, invalid, parse_container_id},
    fs::{HeldDir, OpaquePin, ReadAccess},
};
use std::{
    ffi::{CStr, CString},
    fs::File,
    io::{self, Read, Write},
};

pub(super) const NAMESPACE: &CStr = c"upload-intents";
pub(super) const MAX_ACTIVE: usize = 16;
const CANDIDATES: u8 = 16;
const MAX_LEAVES: usize = MAX_ACTIVE * (2 + CANDIDATES as usize);

pub(super) struct Slot {
    pub(super) record: Record,
    pin: OpaquePin,
}

pub(super) struct Next<'a> {
    directory: &'a HeldDir,
    predecessor: Option<&'a Slot>,
    name: CString,
    temporary: CString,
    source: File,
    pin: OpaquePin,
    record: Record,
}

fn slot_name(id: Id) -> io::Result<CString> {
    if id == [0; 16] {
        return Err(invalid("zero upload control id"));
    }
    CString::new(hex(id)).map_err(|_| invalid("invalid upload control name"))
}

pub(super) fn read(directory: &HeldDir, id: Id) -> io::Result<Option<Slot>> {
    let name = slot_name(id)?;
    let Some(pin) = directory.opaque_pin(&name)? else {
        directory.require_named()?;
        return Ok(None);
    };
    let mut raw = Vec::new();
    (&pin.file)
        .take(MAX_RECORD as u64 + 1)
        .read_to_end(&mut raw)?;
    pin.require(directory, &name)?;
    let record = Record::decode(&raw)?;
    if record.intent.mutation.container != id {
        return Err(invalid("upload record identifies another slot"));
    }
    Ok(Some(Slot { record, pin }))
}

pub(super) struct Ledger {
    pub(super) records: Vec<Record>,
}

impl Ledger {
    pub(super) fn read(directory: Option<&HeldDir>) -> io::Result<Self> {
        let mut records = Vec::new();
        let Some(directory) = directory else {
            return Ok(Self { records });
        };
        let mut seen = 0;
        directory.each_name(ReadAccess::Ordinary, |name| {
            seen += 1;
            if seen > MAX_LEAVES {
                return Err(capacity());
            }
            if is_temporary(name)? {
                return Ok(());
            }
            let id = parse_container_id(name)?;
            if records.len() >= MAX_ACTIVE {
                return Err(capacity());
            }
            let slot =
                read(directory, id)?.ok_or_else(|| invalid("named upload control disappeared"))?;
            records.push(slot.record);
            Ok(())
        })?;
        directory.require_named()?;
        Ok(Self { records })
    }
}

fn is_temporary(name: &[u8]) -> io::Result<bool> {
    let Some(inside) = name
        .strip_prefix(b".")
        .and_then(|name| name.strip_suffix(b".incoming"))
    else {
        return Ok(false);
    };
    let Some((id, code)) = inside.split_at_checked(32) else {
        return Err(invalid("unknown private upload candidate"));
    };
    parse_container_id(id)?;
    if !matches!(code, [b'-', b'0', b'0'..=b'9' | b'a'..=b'f']) {
        return Err(invalid("unknown private upload candidate"));
    }
    Ok(true)
}

pub(super) fn capacity() -> io::Error {
    io::ErrorKind::StorageFull.into()
}

fn allowed(prior: Option<&Slot>, next: &Record) -> bool {
    let Some(prior) = prior else {
        return next.phase == Phase::Preparing;
    };
    if prior.record.data_token != next.data_token || prior.record.intent != next.intent {
        return false;
    }
    match (prior.record.phase, next.phase) {
        (Phase::Preparing, Phase::Ready { offset: 0, .. }) => true,
        (
            Phase::Ready {
                directory: old_directory,
                payload: old_payload,
                offset: old,
            },
            Phase::Ready {
                directory,
                payload,
                offset,
            },
        ) if old_directory == directory && old_payload == payload => {
            offset.checked_sub(old).is_some_and(|length| {
                chunk_use(next.intent.body.length, old, old, length)
                    == Some(ChunkUse::AppendTo(offset))
            })
        }
        _ => false,
    }
}

fn require_old(directory: &HeldDir, name: &CStr, prior: Option<&Slot>) -> io::Result<()> {
    match prior {
        Some(slot) => slot.pin.require(directory, name),
        None => directory.confirm_absence(name),
    }
}

pub(super) fn prepare<'a>(
    directory: &'a HeldDir,
    prior: Option<&'a Slot>,
    next: Record,
    authorize: impl Fn() -> io::Result<()>,
) -> io::Result<Next<'a>> {
    if !allowed(prior, &next) {
        return Err(invalid("invalid upload checkpoint transition"));
    }
    let raw = next.encode()?;
    let id = next.intent.mutation.container;
    let name = slot_name(id)?;
    require_old(directory, &name, prior)?;
    let (temporary, mut source) = make_candidate(directory, id, &authorize)?;
    source.write_all(&raw)?;
    source.sync_all()?;
    let pin = directory
        .opaque_pin(&temporary)?
        .ok_or_else(|| invalid("upload candidate lost its original name"))?;
    require_same(directory, &source, &pin.file)?;
    let mut checked = Vec::new();
    (&pin.file)
        .take(MAX_RECORD as u64 + 1)
        .read_to_end(&mut checked)?;
    pin.require(directory, &temporary)?;
    if checked != raw {
        return Err(invalid(
            "upload candidate differs from its synchronized bytes",
        ));
    }
    Ok(Next {
        directory,
        predecessor: prior,
        name,
        temporary,
        source,
        pin,
        record: next,
    })
}

fn make_candidate(
    directory: &HeldDir,
    id: Id,
    authorize: impl Fn() -> io::Result<()>,
) -> io::Result<(CString, File)> {
    let id = hex(id);
    for n in 0..CANDIDATES {
        let name = CString::new(format!(".{id}-{n:02x}.incoming"))
            .map_err(|_| invalid("invalid upload candidate name"))?;
        authorize()?;
        match directory.create_opaque_record(&name) {
            Ok(file) => return Ok((name, file)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(capacity())
}

impl Next<'_> {
    /// This step alone does not authorize an acknowledgement. Its source is
    /// already synced; the proof of the selected name needs the following sync.
    pub(super) fn choose(
        &self,
        prove: impl FnOnce() -> io::Result<()>,
        authorize: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        // Application custody can take multiple native observations. Do it
        // before the last exact source/predecessor check, not between that
        // comparison and the native cut under the sole cooperative gate.
        prove()?;
        self.pin.require(self.directory, &self.temporary)?;
        require_old(self.directory, &self.name, self.predecessor)?;
        authorize()?;
        promote_control(
            self.directory,
            &self.temporary,
            &self.name,
            self.predecessor.is_some(),
        )
    }

    pub(super) fn confirm(self) -> io::Result<Slot> {
        let expected = self.record.intent.mutation.container;
        let slot = read(self.directory, expected)?
            .ok_or_else(|| invalid("chosen upload checkpoint is absent"))?;
        if slot.record != self.record {
            return Err(invalid(
                "chosen upload checkpoint differs from its original",
            ));
        }
        require_same(self.directory, &self.source, &slot.pin.file)?;
        slot.settle(self.directory)?;
        Ok(slot)
    }
}

impl Slot {
    pub(super) fn settle(&self, directory: &HeldDir) -> io::Result<()> {
        let name = slot_name(self.record.intent.mutation.container)?;
        self.pin.require(directory, &name)?;
        // A visible prior rename without a known directory sync cannot be
        // announced. Confirm this exact selected record, never a candidate.
        directory.file.sync_all()?;
        self.pin.require(directory, &name)
    }

    pub(super) fn require(&self, directory: &HeldDir) -> io::Result<()> {
        self.pin.require(
            directory,
            &slot_name(self.record.intent.mutation.container)?,
        )
    }
}
