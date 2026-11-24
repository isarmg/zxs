//! No canonical early journal is visible until its two unchanged records are complete.
use super::super::{Scope, digest, hex, native_name};
use super::{
    HeldDir, I, OpaquePin, PROOF, Route, Selected, Source, absent, birth, intend, invalid,
    no_replace, pin, publish, read_raw, require_before_intent, require_pre_ready_files, same_file,
};
use crate::fs::ReadAccess;
use std::{
    ffi::{CStr, CString},
    io,
};

pub(super) const PROOF_SPOOL: &CStr = c".source-proof.incoming";
pub(super) const INTENT_SPOOL: &CStr = c".INTENT.incoming";

pub(super) fn journal_name(scope: Scope) -> io::Result<CString> {
    publish::scoped_leaf(scope, c"journal", true)
}

pub(super) fn binding_name(intent: &[u8]) -> io::Result<CString> {
    native_name(&format!(".intent-{}.request", hex(digest(intent)?)))
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum FileAt {
    Missing,
    Spool,
    Complete,
}

fn shape(directory: &HeldDir, binding: &CStr) -> io::Result<(FileAt, FileAt, bool)> {
    use FileAt::{Complete, Missing, Spool};
    let (mut proof, mut intent) = (Missing, Missing);
    let mut bound = false;
    let bad = || invalid("unknown early DATA move journal shape is retained");
    directory.each_name(ReadAccess::NoAtime, |name| {
        if name == binding.to_bytes() && !bound {
            bound = true;
            return Ok(());
        }
        let (position, phase) = match name {
            b".source-proof.incoming" => (&mut proof, Spool),
            b"source-proof" => (&mut proof, Complete),
            b".INTENT.incoming" => (&mut intent, Spool),
            b"INTENT" => (&mut intent, Complete),
            _ => return Err(bad()),
        };
        if *position != Missing {
            return Err(bad());
        }
        *position = phase;
        Ok(())
    })?;
    match (proof, intent, bound) {
        (Complete, Complete, _)
        | (Missing, Missing, _)
        | (Complete, Missing | Spool, true)
        | (Spool, Missing, true) => Ok((proof, intent, bound)),
        _ => Err(bad()),
    }
}

/// Exclusive creation of an exact immutable name is itself atomic. The mark
/// has no payload which could be mistaken for a different request after a cut.
fn bind(
    directory: &HeldDir,
    name: &CStr,
    existed: bool,
    require: &impl Fn() -> io::Result<()>,
) -> io::Result<OpaquePin> {
    require()?;
    let mark = if existed {
        pin(directory, name)?
    } else {
        let original = directory.create_opaque_record(name)?;
        let selected = pin(directory, name)?;
        same_file(directory, &original, &selected.file)?;
        selected
    };
    if mark.file.metadata()?.len() != 0 {
        return Err(invalid(
            "a nonempty early DATA move request mark is retained",
        ));
    }
    mark.file.sync_all()?;
    directory.file.sync_all()?;
    require()?;
    mark.require(directory, name)?;
    Ok(mark)
}

fn record(
    directory: &HeldDir,
    spool: &CStr,
    name: &CStr,
    position: FileAt,
    expected: &[u8],
    require: &impl Fn() -> io::Result<()>,
) -> io::Result<OpaquePin> {
    let selected = if position == FileAt::Complete {
        let selected = pin(directory, name)?;
        if read_raw(&selected, expected.len())? != expected {
            return Err(invalid("unknown early DATA move record is retained"));
        }
        selected.file.sync_all()?;
        selected
    } else {
        publish::publish((directory, spool), (directory, name), expected, require)?
    };
    selected.require(directory, name)?;
    Ok(selected)
}

pub(super) fn prepare(
    selected: &Selected<'_>,
    parent: &HeldDir,
    work: &CStr,
    route: &Route,
    source: &Source,
) -> io::Result<(HeldDir, OpaquePin, OpaquePin)> {
    let canonical = &selected.names.operation;
    let state = selected.roots.state;
    let vacant = || {
        absent(state, &selected.names.active)?;
        absent(state, &selected.names.inactive)?;
        absent(state, &selected.names.guard_spool)?;
        absent(parent, canonical)
    };
    require_before_intent(selected, route, source)?;
    vacant()?;
    let directory = match parent.child_directory(work)? {
        Some(held) => held,
        None => parent.new_child_directory_when(work, || {
            (selected.outer)()?;
            vacant()
        })?,
    };
    let candidate = intend(selected, route, source, &directory)?;
    let raw = candidate.encode()?;
    let binding = binding_name(&raw)?;
    let (proof_at, intent_at, bound) = shape(&directory, &binding)?;
    let require = || {
        selected.require()?;
        route.require(selected)?;
        source.require_named()?;
        selected.vacancies(false)?;
        vacant()?;
        directory.require_named()
    };
    // The whole exact request, including the DATA operands and tokens,
    // must already be witnessed before even a target-independent proof.
    let mark = if !bound && (proof_at, intent_at) == (FileAt::Complete, FileAt::Complete) {
        None // A prior attempt removed its own mark after the exact pair.
    } else {
        Some(bind(&directory, &binding, bound, &require)?)
    };
    let writing = || {
        require()?;
        if let Some(mark) = &mark {
            mark.require(&directory, &binding)?;
        }
        Ok(())
    };
    let proof = record(
        &directory,
        PROOF_SPOOL,
        PROOF,
        proof_at,
        &source.bytes,
        &writing,
    )?;
    // Source proof can be large. Requalify the same original metadata and
    // the actual durable absences again before creating the exact intent.
    source.require_same(&selected.observe_source()?)?;
    require_before_intent(selected, route, source)?;
    proof.require(&directory, PROOF)?;
    let intent = record(&directory, INTENT_SPOOL, I, intent_at, &raw, &writing)?;
    directory.file.sync_all()?;
    parent.file.sync_all()?;
    selected.roots.system.file.sync_all()?;
    writing()?;
    if shape(&directory, &binding)? != (FileAt::Complete, FileAt::Complete, mark.is_some()) {
        return Err(invalid("the exact early DATA move pair is not complete"));
    }
    proof.require(&directory, PROOF)?;
    intent.require(&directory, I)?;
    if let Some(mark) = &mark {
        directory.unlink_opaque_original(&binding, mark)?;
    }
    require()?;
    require_pre_ready_files(&directory, false)?;
    proof.require(&directory, PROOF)?;
    intent.require(&directory, I)?;
    no_replace(parent, work, canonical)?;
    let installed = super::super::child(parent, canonical)?;
    same_file(parent, &directory.file, &installed.file)?;
    if birth(&installed.file)? != candidate.journal {
        return Err(invalid(
            "published DATA move journal differs from the original draft",
        ));
    }
    proof.require(&installed, PROOF)?;
    intent.require(&installed, I)?;
    absent(parent, work)?;
    // BoundJournal::settle repeats the precise sync and proof on the new name
    // before any guard. No DATA path or original CR1 has changed here.
    Ok((installed, intent, proof))
}
