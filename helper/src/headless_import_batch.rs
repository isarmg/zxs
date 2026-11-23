//! A batch is an ordered list of original, individually replayable file tickets.
use super::super::{FreshSelection, Reset, import_command::write_receipt, offline_catalog};
use super::{SourceTree, Stamp, TICKET_BYTES, Ticket, apply, relative_path, same_name_candidate};
use crate::{headless_setup::parse_paths, maintenance::diagnostic_worker, path_rules::Kind};
use rustix::{
    fs::{FileType, fstat},
    process::{getgid, getuid},
};
use std::{
    collections::HashSet,
    ffi::{OsStr, OsString},
    fmt,
    io::{self, IsTerminal, Write},
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    process::ExitCode,
};

#[path = "headless_import_batch_frame.rs"]
mod frame;
#[path = "headless_import_batch_mapping.rs"]
mod mapping_input;
#[path = "headless_import_batch_tree.rs"]
mod tree;

const PREPARE: &str = "expected SOURCE TARGET-DATA SYSTEM RECYCLE DATA [DATA ...], followed by -- RELATIVE NAME pairs or --map0; prepare-tree also allows no mappings";
type Mapping = (OsString, OsString);

#[derive(Debug)]
pub(crate) enum ImportBatch {
    Prepare {
        source: PathBuf,
        target: PathBuf,
        roles: Vec<PathBuf>,
        mapping: Vec<Mapping>,
        whole_tree: bool,
        from_stdin: bool,
    },
    Apply(Vec<PathBuf>),
    Show,
}

impl ImportBatch {
    pub(crate) fn parse(mut args: impl Iterator<Item = OsString>) -> Result<Self, &'static str> {
        match args.next().as_deref() {
            Some(mode) if mode == OsStr::new("prepare") || mode == OsStr::new("prepare-tree") => {
                let whole_tree = mode == OsStr::new("prepare-tree");
                let source = PathBuf::from(args.next().ok_or(PREPARE)?);
                let target = PathBuf::from(args.next().ok_or(PREPARE)?);
                for path in [&source, &target] {
                    Kind::StorageRoot.check_path(path.as_os_str().as_bytes())?;
                }
                let mut from_stdin = false;
                let roles = parse_paths(args.by_ref().take_while(|arg| {
                    from_stdin = arg == "--map0";
                    !from_stdin && arg != "--"
                }))?;
                if from_stdin && args.next().is_some() {
                    return Err(
                        "--map0 takes all mapping pairs from standard input, not arguments",
                    );
                }
                let mut mapping = Vec::new();
                while let Some(relative) = args.next() {
                    mapping.push((relative, args.next().ok_or(PREPARE)?));
                }
                if !whole_tree && !from_stdin && mapping.is_empty() {
                    return Err(PREPARE);
                }
                Ok(Self::Prepare {
                    source,
                    target,
                    roles,
                    mapping,
                    whole_tree,
                    from_stdin,
                })
            }
            Some(mode) if mode == OsStr::new("apply") => Ok(Self::Apply(parse_paths(args)?)),
            Some(mode) if mode == OsStr::new("show") && args.next().is_none() => Ok(Self::Show),
            _ => Err("expected import-batch prepare, prepare-tree, show or apply"),
        }
    }

    pub(crate) fn run(self) -> io::Result<ExitCode> {
        rustix::thread::set_no_new_privs(true)?;
        match self {
            Self::Show => {
                // Display only: no source FD, role directory, catalog or worker required.
                Batch::read(&mut io::stdin().lock())?.show(&mut io::stdout().lock())?;
            }
            Self::Prepare {
                source,
                target,
                roles,
                mapping,
                whole_tree,
                from_stdin,
            } => {
                diagnostic_worker(getuid().as_raw(), getgid().as_raw())?;
                if io::stdout().is_terminal() {
                    return Err(message(
                        "redirect the binary batch plan to a new private file",
                    ));
                }
                let mapping = if from_stdin {
                    mapping_input::read(&mut io::stdin().lock())?
                } else {
                    mapping
                };
                if !whole_tree && mapping.is_empty() {
                    return Err(message("an explicit batch must select at least one file"));
                }
                let reset = FreshSelection::select(&roles)?.open_offline(None)?;
                let plan = if whole_tree {
                    Batch::prepare_tree(&reset, &source, &target, &mapping)?
                } else {
                    Batch::prepare(&reset, &source, &target, &mapping)?
                };
                plan.write(&mut io::stdout().lock())?;
            }
            Self::Apply(roles) => {
                diagnostic_worker(getuid().as_raw(), getgid().as_raw())?;
                // A pipe can disappear with its producer after the first write.
                // Require a retained file; never execute a live stream of operations.
                let input = io::stdin();
                let stat = fstat(&input)?;
                if FileType::from_raw_mode(stat.st_mode) != FileType::RegularFile {
                    return Err(message(
                        "redirect the original saved batch plan to standard input",
                    ));
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

/// Printable valid Unicode stays readable. Invalid bytes, controls and literal
/// backslashes are escaped so one path cannot inject new table rows or columns.
struct Escaped<'a>(&'a [u8]);

impl fmt::Display for Escaped<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for chunk in self.0.utf8_chunks() {
            write!(f, "{}", chunk.valid().escape_debug())?;
            for byte in chunk.invalid() {
                write!(f, "\\x{byte:02x}")?;
            }
        }
        Ok(())
    }
}

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
            return Err(message(format!(
                "batch item {}: invalid relative source or target name",
                index + 1
            )));
        }
        if !paths.insert(relative) || !names.insert(name) {
            return Err(message(format!(
                "batch item {}: repeated source path or exact target name",
                index + 1
            )));
        }
        if existing
            .binary_search_by(|entry| entry.as_bytes().cmp(name))
            .is_ok()
        {
            return Err(message(format!(
                "batch item {}: exact target name already exists: {}",
                index + 1,
                Escaped(name)
            )));
        }
    }
    Ok(())
}

impl Batch {
    fn prepare(reset: &Reset, source: &Path, target: &Path, rows: &[Mapping]) -> io::Result<Self> {
        Self::prepare_rows(
            reset,
            source,
            target,
            rows.iter()
                .map(|(path, name)| (path.as_os_str(), name.as_os_str(), None)),
        )
    }

    fn prepare_rows<'a>(
        reset: &Reset,
        source: &Path,
        target: &Path,
        rows: impl ExactSizeIterator<Item = (&'a OsStr, &'a OsStr, Option<(&'a SourceTree, &'a Stamp)>)>
        + Clone,
    ) -> io::Result<Self> {
        let existing = offline_catalog::read_names(reset)?;
        check_mappings(
            rows.clone()
                .map(|(path, name, _)| (path.as_bytes(), name.as_bytes())),
            &existing,
        )?;
        let mut tickets = Vec::new();
        tickets.try_reserve_exact(rows.len()).map_err(no_memory)?;
        for (index, (path, name, observation)) in rows.enumerate() {
            let ticket = Ticket::prepare_observed(
                reset,
                target,
                source,
                path.as_bytes(),
                name.as_bytes(),
                observation,
            )
            .map_err(|e| io::Error::new(e.kind(), format!("batch item {}: {e}", index + 1)))?;
            tickets.push(ticket);
        }
        reset.require_original()?;
        Self::checked(tickets)
    }

    fn checked(tickets: Vec<Ticket>) -> io::Result<Self> {
        let first = tickets
            .first()
            .ok_or_else(|| message("an import batch cannot be empty"))?;
        check_mappings(
            tickets
                .iter()
                .map(|t| (t.relative.as_slice(), t.name.as_slice())),
            &[],
        )?;
        let mut seeds = HashSet::new();
        seeds.try_reserve(tickets.len()).map_err(no_memory)?;
        for ticket in &tickets {
            if ticket.pool != first.pool
                || ticket.token != first.token
                || ticket.target != first.target
                || ticket.source_root != first.source_root
                || !seeds.insert(ticket.seed)
            {
                return Err(message(
                    "batch tickets have mixed selections or repeated operation IDs",
                ));
            }
        }
        Ok(Self(tickets))
    }

    fn show(&self, output: &mut impl Write) -> io::Result<()> {
        let first = self.0.first().ok_or_else(|| message("empty plan"))?;
        writeln!(
            output,
            "{} files; source: {}; target DATA: {}",
            self.0.len(),
            Escaped(&first.source_root),
            Escaped(&first.target)
        )?;
        writeln!(output, "item\tsource relative path\ttarget name\tbytes")?;
        for (index, ticket) in self.0.iter().enumerate() {
            writeln!(
                output,
                "{}\t{}\t{}\t{}",
                index + 1,
                Escaped(&ticket.relative),
                Escaped(&ticket.name),
                ticket.source.length
            )?;
        }
        output.flush()
    }

    fn apply(&self, reset: &Reset, output: &mut impl Write) -> io::Result<()> {
        // checked() proved every ticket belongs to this same pool, target token,
        // target path and source root. Do not pre-open sources or recheck all
        // names here: a completed item is its own collision and may lack a source.
        self.0
            .first()
            .ok_or_else(|| message("empty plan"))?
            .require_target(reset)?;
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
                return Err(io::Error::new(
                    error.kind(),
                    format!(
                        "batch item {} of {} not confirmed: {error}; this item and earlier items may already be committed. Keep the sources and retry only with the same intact batch plan",
                        index + 1,
                        self.0.len()
                    ),
                ));
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../tests/maintenance/import_batch.rs"]
mod tests;
