//! One verified raw-content pass; a fresh copy never reads its bodies back.
use super::super::super::super::Intent;
use super::{
    Armed, CStr, Checked, File, HeldDir, OpaquePin, Place, Selected, invalid, io, pin, same_file,
};
use crate::container_store::{Body, Change, Error, Receipt, codec, entry_leaf};

use super::super::super::{
    native_name,
    proof::{Proof, Side},
    source::{self, Evidence, Member, qualified_row},
};
use std::io::Write;

/// Current held scope and its verified immutable semantic target, before or after selection.
pub(super) type View<'held, 'scope> = (&'held Selected<'scope>, &'held Checked<Intent>);

pub(super) struct Tree {
    pub(super) at: HeldDir,
    pub(super) bytes: Vec<u8>,
}

impl Tree {
    pub(super) fn copy(armed: &Armed<'_, '_>, proof: &Proof<'_>) -> io::Result<Self> {
        let plan = &armed.plan;
        let selected = plan.selected;
        let storage = &selected.target_staging;
        let source = &plan.source.original;
        if source.entity.device == storage.entity.device || source.same_mount(storage)? {
            return Err(invalid("DATA move staging is not on a distinct filesystem"));
        }
        let at = storage.new_child_directory_when(&selected.names.target_stage, || {
            armed.require_leaf(storage)
        })?;
        let live = selected.outer;
        let mut evidence = source::walk(source, selected.scope, live, |receipt| {
            let input = Member::open(source, receipt)?;
            if input.row != proof.for_receipt(receipt)? {
                return Err(invalid("DATA move source differs from its original proof"));
            }
            let name = native_name(&entry_leaf(receipt.sequence, receipt.mutation.request))?;
            let entry = at.new_child_directory_when(&name, || armed.require_leaf(&at))?;
            let body = copy_payload(armed, receipt, &input, &entry)?;
            armed.require_leaf(&entry)?;
            input.require()?;
            let record = super::super::create(&entry, c"receipt", &codec::encode(receipt)?)?;
            entry.file.sync_all()?;
            let row = qualified_row(&entry, receipt, &record, body.as_ref())?;
            input.require()?;
            Ok(row)
        })?;
        bind(&plan.journal.original, &at, &mut evidence)?;
        Ok(Self {
            at,
            bytes: evidence.bytes,
        })
    }

    pub(super) fn cold(
        view: View<'_, '_>,
        (proof, expected): (&Proof<'_>, &[u8]),
        (parent, name): (&HeldDir, &CStr),
        inode: u64,
    ) -> io::Result<Self> {
        let at = super::super::super::child(parent, name)?;
        if at.entity.inode != inode {
            return Err(invalid("the cold DATA target is not the exact READY inode"));
        }
        let bytes = observe(view, &at, Some(proof))?;
        if bytes != expected {
            return Err(invalid("the cold DATA move target differs from READY"));
        }
        Ok(Self { at, bytes })
    }

    pub(super) fn require_metadata(&self, armed: &Armed<'_, '_>) -> io::Result<()> {
        self.require_metadata_at((armed.plan.selected, &armed.plan.journal.original))
    }

    pub(super) fn require_metadata_at(&self, view: View<'_, '_>) -> io::Result<()> {
        if observe(view, &self.at, None)? != self.bytes {
            return Err(invalid("the exact prepared DATA move target changed"));
        }
        Ok(())
    }

    pub(super) fn require(&self, (parent, name): (&HeldDir, &CStr)) -> io::Result<()> {
        parent.require_named()?;
        self.at.require_named()?;
        if self.at.path != parent.checked_child_path(name)? || !parent.same_mount(&self.at)? {
            return Err(invalid(
                "DATA move target is not at its exact held position",
            ));
        }
        Ok(())
    }

    pub(super) fn settle(&self, armed: &Armed<'_, '_>, place: Place) -> io::Result<()> {
        let selected = armed.plan.selected;
        let site = place.tree_site(selected);
        self.require(site)?;
        self.at.file.sync_all()?;
        if place != Place::Private {
            selected.target_staging.require_named()?;
            selected.target_staging.file.sync_all()?;
            self.require(site)?;
        }
        site.0.file.sync_all()?;
        self.require(site)
    }
}

fn copy_payload(
    armed: &Armed<'_, '_>,
    receipt: &Receipt,
    input: &Member,
    output: &HeldDir,
) -> io::Result<Option<OpaquePin>> {
    let Change::Save { body, .. } = &receipt.change else {
        return Ok(None);
    };
    let source = input
        .payload
        .as_ref()
        .ok_or_else(|| invalid("missing DATA move source payload"))?;
    armed.require_leaf(output)?;
    input.require()?;
    let mut raw = output.create_opaque_record(c"payload")?;
    check_body(body, &source.file, &mut raw, armed.plan.selected.outer)?;
    // Pin the output of the completed write before the next lifecycle cut.
    // An equal-length change at that cut must not become the copied witness.
    let exact = pin_written(output, c"payload", &raw)?;
    armed.require_leaf(output)?;
    input.require()?;
    exact.require(output, c"payload")?;
    raw.sync_all()?;
    exact.require(output, c"payload")?;
    Ok(Some(exact))
}

fn pin_written(output: &HeldDir, name: &CStr, raw: &File) -> io::Result<OpaquePin> {
    let exact = pin(output, name)?;
    same_file(output, raw, &exact.file)?;
    exact.require(output, name)?;
    Ok(exact)
}

fn observe(
    (selected, intent): View<'_, '_>,
    at: &HeldDir,
    full: Option<&Proof<'_>>,
) -> io::Result<Vec<u8>> {
    let live = selected.outer;
    let mut evidence = source::walk(at, selected.scope, live, |receipt| {
        let member = Member::open(at, receipt)?;
        if let Some(expect) = full {
            if member.row != expect.for_receipt(receipt)? {
                return Err(invalid(
                    "cold target row differs from its bound native proof",
                ));
            }
            if let Change::Save { body, .. } = &receipt.change {
                let raw = member
                    .payload
                    .as_ref()
                    .ok_or_else(|| invalid("missing cold DATA move target payload"))?;
                check_body(body, &raw.file, &mut io::sink(), live)?;
                live()?; // The final body/EOF read can have blocked.
            }
            member.require()?;
        }
        Ok(member.row)
    })?;
    bind(intent, at, &mut evidence)?;
    Ok(evidence.bytes)
}

fn bind(intent: &Checked<Intent>, at: &HeldDir, evidence: &mut Evidence) -> io::Result<()> {
    let i = intent.value();
    if evidence.count != i.rows || evidence.ledger != i.ledger {
        return Err(invalid(
            "DATA move target does not have the exact held logical history",
        ));
    }
    evidence.bind(Side::Target, i.scope, i.target, at.entity.inode)
}

fn check_body(
    body: &Body,
    file: &File,
    output: &mut impl Write,
    live: &dyn Fn() -> io::Result<()>,
) -> io::Result<()> {
    live()?;
    let mut input = file;
    body.copy_exact(&mut input, output, live)
        .map_err(|e| match e {
            Error::Io(e) => e,
            _ => invalid("DATA move body failed complete length, EOF or SHA-256 verification"),
        })
}
