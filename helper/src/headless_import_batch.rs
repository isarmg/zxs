//! A batch is an ordered list of original, individually replayable file tickets.
use super::{TICKET_BYTES, Ticket, apply, relative_path, same_name_candidate};
use super::super::{FreshSelection, Reset, import_command::write_receipt, offline_catalog};
use crate::{headless_setup::parse_paths, maintenance::diagnostic_worker, path_rules::Kind};
use rustix::{fs::{FileType, fstat}, process::{getgid, getuid}};
use std::{
    collections::HashSet,
    ffi::{OsStr, OsString},
    io::{self, IsTerminal, Write},
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    process::ExitCode,
};

#[path = "headless_import_batch_frame.rs"]
mod frame;

const PREPARE: &str = "expected import-batch prepare SOURCE TARGET-DATA SYSTEM RECYCLE DATA [DATA ...] -- RELATIVE NAME [RELATIVE NAME ...]";
type Mapping = (OsString, OsString);

#[derive(Debug)]
pub(crate) enum ImportBatch {
    Prepare {
        source: PathBuf,
        target: PathBuf,
        roles: Vec<PathBuf>,
        mapping: Vec<Mapping>,
    },
    Apply(Vec<PathBuf>),
}

impl ImportBatch {
    pub(crate) fn parse(mut args: impl Iterator<Item = OsString>) -> Result<Self, &'static str> {
        match args.next().as_deref() {
            Some(mode) if mode == OsStr::new("prepare") => {
                let source = PathBuf::from(args.next().ok_or(PREPARE)?);
                let target = PathBuf::from(args.next().ok_or(PREPARE)?);
                for path in [&source, &target] {
                    Kind::StorageRoot.check_path(path.as_os_str().as_bytes())?;
                }
                let roles = parse_paths(args.by_ref().take_while(|arg| arg != "--"))?;
                let mut mapping = Vec::new();
                while let Some(relative) = args.next() {
                    mapping.push((relative, args.next().ok_or(PREPARE)?));
                }
                if mapping.is_empty() {
                    return Err(PREPARE);
                }
                Ok(Self::Prepare { source, target, roles, mapping })
            }
            Some(mode) if mode == OsStr::new("apply") => Ok(Self::Apply(parse_paths(args)?)),
            _ => Err("expected import-batch prepare or import-batch apply"),
        }
    }

    pub(crate) fn run(self) -> io::Result<ExitCode> {
        rustix::thread::set_no_new_privs(true)?;
        diagnostic_worker(getuid().as_raw(), getgid().as_raw())?;
        match self {
            Self::Prepare { source, target, roles, mapping } => {
                if io::stdout().is_terminal() {
                    return Err(message("redirect the binary batch plan to a new private file"));
                }
                let reset = FreshSelection::select(&roles)?.open_offline(None)?;
                let plan = Batch::prepare(&reset, &source, &target, &mapping)?;
                plan.write(&mut io::stdout().lock())?;
            }
            Self::Apply(roles) => {
                // A pipe can disappear with its producer after the first write.
                // Require a retained file; never execute a live stream of operations.
                let input = io::stdin();
                let stat = fstat(&input)?;
                if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
                    return Err(message("redirect the original saved batch plan to standard input"));
                }
                let plan = Batch::read(&mut input.lock())?;
                let reset = FreshSelection::select(&roles)?.open_offline(None)?;
                plan.apply(&reset, &mut io::stdout().lock())?;
            }
        }
        Ok(ExitCode::SUCCESS)
    }
}

fn message(text: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, text.into())
}

fn no_memory(_: std::collections::TryReserveError) -> io::Error {
    io::Error::from(io::ErrorKind::OutOfMemory)
}

struct Batch(Vec<Ticket>);

fn check_mappings<'a>(
    rows: impl Iterator<Item = (&'a [u8], &'a [u8])>,
    existing: &[String],
) -> io::Result<()> {
    let mut paths = HashSet::new();
    let mut names = HashSet::new();
    for (index, (relative, name)) in rows.enumerate() {
        paths.try_reserve(1).map_err(no_memory)?;
        names.try_reserve(1).map_err(no_memory)?;
        if !relative_path(relative) || !same_name_candidate(name) {
            return Err(message(format!("batch item {}: invalid relative source or target name", index + 1)));
        }
        if !paths.insert(relative) || !names.insert(name) {
            return Err(message(format!("batch item {}: repeated source path or exact target name", index + 1)));
        }
        if existing.binary_search_by(|entry| entry.as_bytes().cmp(name)).is_ok() {
            return Err(message(format!("batch item {}: exact target name already exists", index + 1)));
        }
    }
    Ok(())
}

impl Batch {
    fn prepare(reset: &Reset, source: &Path, target: &Path, rows: &[Mapping]) -> io::Result<Self> {
        let existing = offline_catalog::read_names(reset)?;
        check_mappings(rows.iter().map(|(path, name)| (path.as_bytes(), name.as_bytes())), &existing)?;
        let mut tickets = Vec::new();
        tickets.try_reserve_exact(rows.len()).map_err(no_memory)?;
        for (index, (path, name)) in rows.iter().enumerate() {
            let ticket = Ticket::prepare(reset, target, source, path.as_bytes(), name.as_bytes())
                .map_err(|e| io::Error::new(e.kind(), format!("batch item {}: {e}", index + 1)))?;
            tickets.push(ticket);
        }
        reset.require_original()?;
        Self::checked(tickets)
    }

    fn checked(tickets: Vec<Ticket>) -> io::Result<Self> {
        let first = tickets.first().ok_or_else(|| message("an import batch cannot be empty"))?;
        check_mappings(tickets.iter().map(|t| (t.relative.as_slice(), t.name.as_slice())), &[])?;
        let mut seeds = HashSet::new();
        seeds.try_reserve(tickets.len()).map_err(no_memory)?;
        for ticket in &tickets {
            if ticket.pool != first.pool || ticket.token != first.token
                || ticket.target != first.target || ticket.source_root != first.source_root
                || !seeds.insert(ticket.seed)
            {
                return Err(message("batch tickets have mixed selections or repeated operation IDs"));
            }
        }
        Ok(Self(tickets))
    }

    fn apply(&self, reset: &Reset, output: &mut impl Write) -> io::Result<()> {
        // checked() proved every ticket belongs to this same pool, target token,
        // target path and source root. Do not pre-open sources or recheck all
        // names here: a completed item is its own collision and may lack a source.
        self.0.first().ok_or_else(|| message("empty plan"))?.require_target(reset)?;
        self.visit(|index, ticket| {
            let outcome = apply(reset, ticket)?;
            write!(output, "{}/{} ", index + 1, self.0.len())?;
            write_receipt(output, &outcome)?;
            output.flush()
        })
    }

    fn visit(&self, mut item: impl FnMut(usize, &Ticket) -> io::Result<()>) -> io::Result<()> {
        for (index, ticket) in self.0.iter().enumerate() {
            if let Err(error) = item(index, ticket) {
                return Err(io::Error::new(error.kind(), format!(
                    "batch item {} of {} not confirmed: {error}; this item and earlier items may already be committed. Keep the sources and retry only with the same intact batch plan",
                    index + 1, self.0.len()
                )));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../tests/maintenance/import_batch.rs"]
mod tests;
