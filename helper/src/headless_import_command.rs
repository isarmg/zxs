//! Explicit one-file conversion: prepare a ticket, then apply exactly that ticket.
use super::{
    FreshSelection,
    import_one::{ApplyOutcome, TICKET_BYTES, Ticket, apply},
};
use crate::{headless_setup::parse_paths, maintenance::diagnostic_worker, path_rules::Kind};
use rustix::process::{getgid, getuid};
use std::{
    ffi::{OsStr, OsString},
    io::{self, Read, Write},
    os::unix::ffi::OsStrExt,
    path::PathBuf,
    process::ExitCode,
};

const PREPARE: &str =
    "expected import-one prepare SOURCE RELATIVE NAME TARGET-DATA SYSTEM RECYCLE DATA [DATA ...]";
const LIMIT: usize = 2 * TICKET_BYTES + 1;

#[derive(Debug)]
pub(crate) enum ImportOne {
    Prepare {
        roles: Vec<PathBuf>,
        source: PathBuf,
        relative: OsString,
        name: OsString,
        target: PathBuf,
    },
    Apply(Vec<PathBuf>),
}

impl ImportOne {
    pub(crate) fn parse(mut args: impl Iterator<Item = OsString>) -> Result<Self, &'static str> {
        match args.next().as_deref() {
            Some(mode) if mode == OsStr::new("prepare") => {
                let source = PathBuf::from(args.next().ok_or(PREPARE)?);
                let relative = args.next().ok_or(PREPARE)?;
                let name = args.next().ok_or(PREPARE)?;
                let target = PathBuf::from(args.next().ok_or(PREPARE)?);
                for path in [&source, &target] {
                    Kind::StorageRoot.check_path(path.as_os_str().as_bytes())?;
                }
                let roles = parse_paths(args)?;
                Ok(Self::Prepare {
                    roles,
                    source,
                    relative,
                    name,
                    target,
                })
            }
            Some(mode) if mode == OsStr::new("apply") => Ok(Self::Apply(parse_paths(args)?)),
            _ => Err("expected import-one prepare or import-one apply"),
        }
    }

    pub(crate) fn run(self) -> io::Result<ExitCode> {
        rustix::thread::set_no_new_privs(true)?;
        diagnostic_worker(getuid().as_raw(), getgid().as_raw())?;
        match self {
            Self::Prepare {
                roles,
                source,
                relative,
                name,
                target,
            } => {
                let reset = FreshSelection::select(&roles)?.open_offline(None)?;
                let ticket = Ticket::prepare(
                    &reset,
                    &target,
                    &source,
                    relative.as_os_str().as_bytes(),
                    name.as_os_str().as_bytes(),
                )?;
                let mut out = hex_line(&ticket.encode()?);
                out.push(b'\n');
                io::stdout().lock().write_all(&out)?;
            }
            Self::Apply(roles) => {
                let input = io::stdin().lock();
                let ticket = read_ticket(input)?;
                let reset = FreshSelection::select(&roles)?.open_offline(None)?;
                let outcome = apply(&reset, &ticket).map_err(|_| uncertain())?;
                write_receipt(&mut io::stdout().lock(), &outcome).map_err(|_| uncertain())?;
            }
        }
        Ok(ExitCode::SUCCESS)
    }
}

pub(super) fn write_receipt(output: &mut impl Write, outcome: &ApplyOutcome) -> io::Result<()> {
    let text = |id: [u8; 16]| String::from_utf8_lossy(&hex_line(&id)).into_owned();
    let result = if outcome.prior {
        "existing exact import receipt"
    } else {
        "imported complete file"
    };
    writeln!(
        output,
        "{result}: container {} node {}",
        text(outcome.container),
        text(outcome.node)
    )
}

fn invalid() -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidInput,
        "expected one intact import ticket on standard input",
    )
}

fn uncertain() -> io::Error {
    io::Error::other(
        "could not confirm the import; keep the original source and retry only with the same unchanged ticket",
    )
}

fn read_ticket(input: impl Read) -> io::Result<Ticket> {
    let mut line = Vec::new();
    input
        .take(u64::try_from(LIMIT + 1).map_err(|_| invalid())?)
        .read_to_end(&mut line)?;
    if line.len() > LIMIT || line.pop() != Some(b'\n') || line.is_empty() {
        return Err(invalid());
    }
    let (pairs, remainder) = line.as_chunks::<2>();
    if !remainder.is_empty() {
        return Err(invalid());
    }
    let value = |byte| match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => Err(invalid()),
    };
    let raw: Vec<u8> = pairs
        .iter()
        .map(|[high, low]| Ok((value(*high)? << 4) | value(*low)?))
        .collect::<io::Result<_>>()?;
    Ticket::decode(&raw).map_err(|_| invalid())
}

fn hex_line(bytes: &[u8]) -> Vec<u8> {
    const ALPHABET: &[u8; 16] = b"0123456789abcdef";
    bytes
        .iter()
        .flat_map(|byte| {
            [
                ALPHABET[usize::from(byte >> 4)],
                ALPHABET[usize::from(byte & 15)],
            ]
        })
        .collect()
}

#[cfg(test)]
#[path = "../tests/maintenance/import_one_command.rs"]
mod tests;
