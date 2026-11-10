//! Offline consumer for the opt-in independent disk format.
pub(crate) mod check;
use crate::{Result, print, private_io};
use command_line::{Args, Subcommand};
use data_serialization::de::DeserializeOwned;
use json_codec::json;
use std::path::{Path, PathBuf};
use storage_engine::{
    OperationId,
    disk_authority::{
        self, Digest, DiskSelection, FileWrite, Identity, IndependentDisk, ReviewedPublication,
        Token,
    },
};

#[derive(Subcommand)]
pub(crate) enum Command {
    /// Create only an empty directory in independent-disk-journal format, or finish its exact initialization.
    Initialize {
        disk: PathBuf,
        identity: PathBuf,
        #[arg(long)]
        accept_new_format: bool,
    },
    /// Read the exact selected identity and its complete local publication chain.
    Inspect { selection: PathBuf },
    /// Check retained local FULL copies independently; content issues return nonzero. Stop all writers first.
    Check { selection: PathBuf },
    /// Read-only review; prints the digest required by publish-file.
    ReviewFile {
        #[command(flatten)]
        write: FileArguments,
    },
    /// Rebuild and compare the exact review, then publish one primary file version.
    PublishFile {
        #[command(flatten)]
        write: FileArguments,
        #[arg(long)]
        request_digest: Digest,
        #[arg(long)]
        confirm_write: bool,
    },
    /// Read-only review of explicitly selected source checkpoints.
    ReviewBackup {
        #[command(flatten)]
        backup: BackupArguments,
    },
    /// Copy verified source closure and commit only this backup disk; never change the source.
    PublishBackup {
        #[command(flatten)]
        backup: BackupArguments,
        #[arg(long)]
        request_digest: Digest,
        #[arg(long)]
        confirm_write: bool,
    },
    /// Query durable intent/prepared/publication; it does not perform recovery writes.
    Status {
        selection: PathBuf,
        operation: OperationId,
    },
    /// Resume the exact already prepared operation or verify its existing decision.
    Resume {
        selection: PathBuf,
        operation: OperationId,
        #[arg(long)]
        request_digest: Digest,
        #[arg(long)]
        confirm_write: bool,
    },
    /// Write a disposable projection outside data disks, under an existing private parent.
    RebuildProjection { cache: PathBuf, selections: PathBuf },
    /// Export one fully verified published body; output must not exist.
    Export {
        selection: PathBuf,
        commit: Digest,
        output: PathBuf,
    },
}

#[derive(Args)]
pub(crate) struct FileArguments {
    selection: PathBuf,
    operation: OperationId,
    /// Exact reviewed checkpoint, or "empty" for an unmodified new disk.
    #[arg(long)]
    base: String,
    #[arg(long)]
    history: String,
    #[arg(long)]
    file: String,
    input: PathBuf,
}
#[derive(Args)]
pub(crate) struct BackupArguments {
    selection: PathBuf,
    operation: OperationId,
    #[arg(long)]
    base: String,
    /// JSON array of exact source selections, including original identities.
    sources: PathBuf,
}
fn read<T: DeserializeOwned>(path: &Path) -> Result<T> {
    Ok(json_codec::from_slice(&private_io::read_owned(
        path,
        1 << 20,
    )?)?)
}
fn open(path: &Path) -> Result<IndependentDisk> {
    Ok(IndependentDisk::open(&read::<DiskSelection>(path)?)?)
}
fn base(text: &str) -> Result<Option<Digest>> {
    if text == "empty" {
        Ok(None)
    } else {
        Ok(Some(text.parse()?))
    }
}
fn review_file(args: FileArguments) -> Result<(IndependentDisk, ReviewedPublication)> {
    let disk = open(&args.selection)?;
    let bytes = private_io::read_owned(&args.input, 16 << 20)?;
    let review = disk.review_file(FileWrite {
        operation: args.operation,
        expected_base: base(&args.base)?,
        history: &Token::new(args.history)?,
        file: &Token::new(args.file)?,
        bytes: &bytes,
    })?;
    Ok((disk, review))
}
fn review_backup(args: BackupArguments) -> Result<(IndependentDisk, ReviewedPublication)> {
    let disk = open(&args.selection)?;
    let sources: Vec<DiskSelection> = read(&args.sources)?;
    let review = disk.review_backup(args.operation, base(&args.base)?, &sources)?;
    Ok((disk, review))
}
fn print_review(review: &ReviewedPublication) -> Result<()> {
    print(
        json!({"format":"independent-disk-journal", "stage":"review", "operation_id": review.operation_id(), "base":review.base(), "request_digest": review.request_digest(), "published":false}),
    )
}
fn require_confirm(confirmed: bool) -> Result<()> {
    if confirmed {
        Ok(())
    } else {
        Err("review the exact request and pass --confirm-write; no write was attempted".into())
    }
}
fn publish(disk: IndependentDisk, review: ReviewedPublication, expected: Digest) -> Result<()> {
    if review.request_digest() != expected {
        return Err("review changed; no write was attempted".into());
    }
    print(disk.execute(&review)?)
}

pub(crate) fn run(command: Command) -> Result<()> {
    match command {
        Command::Initialize {
            disk,
            identity,
            accept_new_format,
        } => {
            if !accept_new_format {
                return Err("explicitly select --accept-new-format; existing online pools cannot be converted".into());
            }
            print(IndependentDisk::initialize(&disk, read::<Identity>(&identity)?)?.report()?)
        }
        Command::Inspect { selection } => print(open(&selection)?.report()?),
        Command::Check { selection } => check::run(&selection),
        Command::ReviewFile { write } => print_review(&review_file(write)?.1),
        Command::PublishFile {
            write,
            request_digest,
            confirm_write,
        } => {
            require_confirm(confirm_write)?;
            let (disk, review) = review_file(write)?;
            publish(disk, review, request_digest)
        }
        Command::ReviewBackup { backup } => print_review(&review_backup(backup)?.1),
        Command::PublishBackup {
            backup,
            request_digest,
            confirm_write,
        } => {
            require_confirm(confirm_write)?;
            let (disk, review) = review_backup(backup)?;
            publish(disk, review, request_digest)
        }
        Command::Status {
            selection,
            operation,
        } => print(open(&selection)?.operation_status(operation)?),
        Command::Resume {
            selection,
            operation,
            request_digest,
            confirm_write,
        } => {
            require_confirm(confirm_write)?;
            print(open(&selection)?.resume(operation, request_digest)?)
        }
        Command::RebuildProjection { cache, selections } => print(
            disk_authority::rebuild_projection(&cache, &read::<Vec<DiskSelection>>(&selections)?)?,
        ),
        Command::Export {
            selection,
            commit,
            output,
        } => {
            let body = open(&selection)?.read_content(commit)?;
            private_io::write_new(&output, &body)?;
            print(
                json!({"commit":commit,"bytes":body.len(),"exported":output,"mode":"independent-disk"}),
            )
        }
    }
}
