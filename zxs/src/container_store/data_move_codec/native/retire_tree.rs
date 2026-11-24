//! The only source unlink plan. Preflight every surviving name before removing one.
use super::super::super::super::super::{
    native_name,
    proof::Row,
    source::{self, Member},
};
use super::{HeldDir, OpaquePin, Proof, Selected, absent, invalid, io, pin, read_raw};
use crate::container_store::{
    Id, Receipt, codec, entry_leaf, generation_name, held_entry, parse_container_id,
};
use crate::fs::ReadAccess;

const ACCESS: ReadAccess = ReadAccess::NoAtime;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Part {
    Gone,
    Empty,
    Receipt,
    Whole,
}

struct Step {
    from: Row,
    target: Row,
    part: Part,
}

struct Tail {
    at: HeldDir,
    receipt: Option<OpaquePin>,
    payload: Option<OpaquePin>,
}

impl Tail {
    fn part(&self, expected: Row) -> Part {
        if self.receipt.is_none() {
            Part::Empty
        } else if expected.payload.is_some() && self.payload.is_none() {
            Part::Receipt
        } else {
            Part::Whole
        }
    }
}

pub(super) struct SourcePlan {
    original: Option<HeldDir>,
    rows: Vec<Step>,
}

impl SourcePlan {
    pub(super) fn prove(
        selected: &Selected<'_>,
        target: &HeldDir,
        (source_proof, target_proof): (&Proof<'_>, &Proof<'_>),
        source_identity: (u64, Option<&HeldDir>),
        expected: (u64, [u8; 32]),
    ) -> io::Result<Self> {
        let original = open_source(selected, source_identity)?;
        let mut rows = Vec::new();
        let mut partial = false;
        let mut suffix = false;
        let evidence = source::walk(target, selected.scope, selected.pulse, |receipt| {
            let from = source_proof.for_receipt(receipt)?;
            let aim = target_proof.for_receipt(receipt)?;
            if Member::open(target, receipt)?.row != aim {
                return Err(invalid("retirement target row changed"));
            }
            let part = match &original {
                Some(at) => {
                    inspect_tail(at, receipt, from)?.map_or(Part::Gone, |held| held.part(from))
                }
                None => Part::Gone,
            };
            if source_identity.1.is_some() && part != Part::Whole {
                return Err(invalid("the live source changed before first retirement"));
            }
            match part {
                Part::Gone if !partial && !suffix => {}
                Part::Empty | Part::Receipt if !partial && !suffix => partial = true,
                Part::Whole => suffix = true,
                _ => return Err(invalid("retirement source is not a proved cleanup prefix")),
            }
            rows.push(Step {
                from,
                target: aim,
                part,
            });
            Ok(aim)
        })?;
        if (evidence.count, evidence.ledger) != expected {
            return Err(invalid(
                "retirement target is not the exact original logical history",
            ));
        }
        if let Some(at) = &original {
            require_inventory(at, &rows)?;
        }
        Ok(Self { original, rows })
    }

    pub(super) fn remove(
        self,
        selected: &Selected<'_>,
        target: &HeldDir,
        live: &dyn Fn() -> io::Result<()>,
    ) -> io::Result<()> {
        let parent = &selected.original_data;
        let name = &selected.names.cid;
        let Some(original) = self.original else {
            live()?;
            return parent.confirm_absence(name);
        };
        let missing = self
            .rows
            .iter()
            .take_while(|row| row.part == Part::Gone)
            .count();
        confirm_prefix(&original, &self.rows[..missing], live)?;
        for row in &self.rows[missing..] {
            remove_step(&original, target, selected.scope.container, row, live)?;
        }
        live()?;
        parent.unlink_empty_opaque_original(name, &original)
    }
}

fn open_source(
    selected: &Selected<'_>,
    (inode, live): (u64, Option<&HeldDir>),
) -> io::Result<Option<HeldDir>> {
    let parent = &selected.original_data;
    parent.require_named()?;
    let at = parent.child_directory(&selected.names.cid)?;
    if let Some(candidate) = &at {
        if candidate.entity.inode != inode || !candidate.same_mount(parent)? {
            return Err(invalid("retirement source has a different native identity"));
        }
        if let Some(original) = live {
            original.require_named()?;
            if original.entity != candidate.entity || !original.same_mount(candidate)? {
                return Err(invalid("held source is not the original live directory"));
            }
        }
    } else if live.is_some() {
        return Err(invalid(
            "original live source disappeared before retirement",
        ));
    }
    parent.require_named()?;
    Ok(at)
}

fn inspect_tail(parent: &HeldDir, receipt: &Receipt, row: Row) -> io::Result<Option<Tail>> {
    let name = native_name(&entry_leaf(row.sequence, row.operation))?;
    let Some(at) = parent.child_directory(&name)? else {
        absent(parent, &name)?;
        return Ok(None);
    };
    if at.entity.inode != row.dir_inode {
        return Err(invalid("a surviving retirement entry is not the original"));
    }
    at.each_name(ACCESS, |name| match name {
        b"receipt" => Ok(()),
        b"payload" if row.payload.is_some() => Ok(()),
        _ => Err(invalid(
            "unexpected original source entry material is retained",
        )),
    })?;
    let found = at.opaque_pin_with(c"receipt", ACCESS)?;
    let payload = at.opaque_pin_with(c"payload", ACCESS)?;
    if let Some(exact) = &found {
        let canonical = codec::encode(receipt)?;
        if source::stamp(exact)? != row.receipt || read_raw(exact, canonical.len())? != canonical {
            return Err(invalid(
                "a surviving source receipt differs from its exact proof",
            ));
        }
        exact.require(&at, c"receipt")?;
    } else {
        absent(&at, c"receipt")?;
    }
    if let Some(raw) = &payload {
        if found.is_none() || row.payload != Some(source::stamp(raw)?) {
            return Err(invalid(
                "a surviving source payload is not the proved original",
            ));
        }
        raw.require(&at, c"payload")?;
    } else {
        absent(&at, c"payload")?;
    }
    at.require_named()?;
    Ok(Some(Tail {
        at,
        receipt: found,
        payload,
    }))
}

fn require_inventory(original: &HeldDir, rows: &[Step]) -> io::Result<()> {
    let mut remaining = rows.iter().filter(|r| r.part != Part::Gone).count();
    original.each_name(ACCESS, |raw| {
        let leaf = std::str::from_utf8(raw).map_err(|_| invalid("unknown source entry"))?;
        let (sequence, operation) = generation_name(leaf)?;
        let index = usize::try_from(sequence - 1).map_err(|_| invalid("unknown source entry"))?;
        let operation = parse_container_id(operation.as_bytes())?;
        if rows
            .get(index)
            .is_none_or(|row| row.from.operation != operation || row.part == Part::Gone)
        {
            return Err(invalid("unknown source member is retained"));
        }
        remaining = remaining
            .checked_sub(1)
            .ok_or_else(|| invalid("unexpected source member"))?;
        Ok(())
    })?;
    if remaining != 0 {
        return Err(invalid("the source inventory changed before retirement"));
    }
    original.require_named()
}

fn confirm_prefix(
    original: &HeldDir,
    gone: &[Step],
    live: &dyn Fn() -> io::Result<()>,
) -> io::Result<()> {
    if !gone.is_empty() {
        let absence = || {
            for row in gone {
                let name = native_name(&entry_leaf(row.from.sequence, row.from.operation))?;
                absent(original, &name)?;
            }
            original.require_named()
        };
        live()?;
        absence()?;
        original.file.sync_all()?;
        absence()?;
        live()?;
    }
    Ok(())
}

fn exact_target(target: &HeldDir, cid: Id, row: Row) -> io::Result<(Receipt, Member)> {
    let leaf = entry_leaf(row.sequence, row.operation);
    let directory = held_entry(target, &leaf)?;
    let at = pin(&directory, c"receipt")?;
    let length = usize::try_from(row.receipt.len).map_err(|_| invalid("target receipt extent"))?;
    let raw = read_raw(&at, length)?;
    if source::stamp(&at)? != row.receipt {
        return Err(invalid(
            "the target receipt changed before source retirement",
        ));
    }
    at.require(&directory, c"receipt")?;
    let receipt = codec::decode(&raw)?;
    if (
        receipt.mutation.container,
        receipt.mutation.request,
        receipt.sequence,
    ) != (cid, row.operation, row.sequence)
        || codec::encode(&receipt)? != raw
    {
        return Err(invalid(
            "the target receipt no longer identifies the exact source entry",
        ));
    }
    let member = Member::open(target, &receipt)?;
    if member.row != row {
        return Err(invalid("the target row changed before source retirement"));
    }
    Ok((receipt, member))
}

fn remove_step(
    original: &HeldDir,
    target: &HeldDir,
    cid: Id,
    step: &Step,
    live: &dyn Fn() -> io::Result<()>,
) -> io::Result<()> {
    let (receipt, protected) = exact_target(target, cid, step.target)?;
    let tail = inspect_tail(original, &receipt, step.from)?
        .filter(|held| held.part(step.from) == step.part)
        .ok_or_else(|| invalid("a preflighted source entry changed before retirement"))?;
    let check = || {
        live()?;
        protected.require()?;
        original.require_named()?;
        tail.at.require_named()
    };
    check()?;
    if let Some(stored) = &tail.receipt {
        stored.require(&tail.at, c"receipt")?;
        if let Some(raw) = &tail.payload {
            tail.at.unlink_opaque_original(c"payload", raw)?;
        } else if step.from.payload.is_some() {
            // Newly observed absence may be visible but not durable across a crash.
            tail.at.confirm_absence(c"payload")?;
        }
        check()?;
        absent(&tail.at, c"payload")?;
        tail.at.unlink_opaque_original(c"receipt", stored)?;
    } else {
        // Both prior unlinks can be confirmed by the same original directory sync.
        tail.at.only(None)?;
        tail.at.file.sync_all()?;
        tail.at.only(None)?;
    }
    check()?;
    let name = native_name(&entry_leaf(step.from.sequence, step.from.operation))?;
    original.unlink_empty_opaque_original(&name, &tail.at)
}
