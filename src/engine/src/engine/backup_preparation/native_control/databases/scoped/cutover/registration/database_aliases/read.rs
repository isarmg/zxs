//! Same-descriptor complete verification precedes release of any byte range.
//! Stored alias JSON and a caller-supplied path never select a file to open.
use super::*;
use std::io::Read;

const MAX_RANGE: usize = 4 * 1024 * 1024;
#[derive(Clone, Debug)]
pub struct BackupDatabaseRead {
    pub scope: BackupDatabaseScope,
    pub file: BackupDatabaseFile,
    pub offset: u64,
    pub length: usize,
}
#[derive(Clone, Debug)]
pub struct BackupDatabaseRange {
    pub aliases_hash: String,
    pub publication_hash: String,
    pub database: BackupDatabaseAlias,
    pub offset: u64,
    pub bytes: Vec<u8>,
}

pub(super) fn verify_range(
    root: &DiskRoot,
    relative: &str,
    maximum: u64,
    expected: &str,
    range: Option<(u64, usize)>,
    stop: &AtomicBool,
) -> Result<(u64, Vec<u8>)> {
    let mut file = root.open_read_regular(&root.path().join(relative))?;
    let size = file.metadata()?.len();
    if size > maximum {
        return Err(Error::new(
            ErrorCode::ResourceLimit,
            "Backup database exceeds its scope budget",
        ));
    }
    let (offset, length) = range.unwrap_or((0, 0));
    let end = offset
        .checked_add(length as u64)
        .filter(|end| *end <= size)
        .ok_or_else(|| {
            Error::new(
                ErrorCode::InvalidRequest,
                "Backup database range exceeds its actual file",
            )
        })?;
    if length > MAX_RANGE {
        return Err(Error::new(
            ErrorCode::ResourceLimit,
            "Backup database range exceeds 4 MiB",
        ));
    }
    let mut bytes = Vec::with_capacity(length);
    let mut buffer = [0u8; 64 * 1024];
    let mut hasher = blake3::Hasher::new();
    let mut position = 0u64;
    loop {
        preparation_checkpoint(stop)?;
        file.check_running()?;
        let count = match file.read(&mut buffer) {
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            result => result?,
        };
        if count == 0 {
            break;
        }
        let next = position
            .checked_add(count as u64)
            .filter(|next| *next <= size)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::ChecksumMismatch,
                    "Backup database grew while reading",
                )
            })?;
        hasher.update(&buffer[..count]);
        let begin = position.max(offset);
        let finish = next.min(end);
        if begin < finish {
            bytes.extend_from_slice(
                &buffer[(begin - position) as usize..(finish - position) as usize],
            );
        }
        position = next;
    }
    if position != size
        || file.metadata()?.len() != size
        || hasher.finalize().to_hex().as_str() != expected
        || bytes.len() != length
    {
        return Err(Error::new(
            ErrorCode::ChecksumMismatch,
            "Backup database content differs from its exact published scope",
        ));
    }
    file.check_running()?;
    Ok((size, bytes))
}

pub(in super::super) fn read(
    context: &common_databases::Context<'_>,
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    expected: &BackupFirstPublicationHash,
    request: &BackupDatabaseRead,
    stop: &AtomicBool,
) -> Result<BackupDatabaseRange> {
    if request.length > MAX_RANGE {
        return Err(Error::new(
            ErrorCode::ResourceLimit,
            "Backup database range exceeds 4 MiB",
        ));
    }
    let plan = Plan::capture(context, prepared, decision, expected, stop)?;
    plan.require_complete(context, prepared, stop)?;
    let alias = plan
        .report
        .aliases
        .iter()
        .find(|alias| alias.scope == request.scope && alias.file == request.file)
        .ok_or_else(|| {
            Error::new(
                ErrorCode::NotFound,
                "Backup database scope is outside the exact original set",
            )
        })?;
    let mut result = None;
    visit_files(
        context,
        prepared,
        &plan.report.current,
        stop,
        |root, scope, file, relative, digest| {
            if scope != request.scope || file != request.file {
                return Ok(());
            }
            if result.is_some() || alias.relative_file != relative || alias.digest != digest {
                return Err(invalid(
                    "Backup alias is ambiguous or differs from its exact scope",
                ));
            }
            require_named(root, &alias.root)?;
            let (size, bytes) = verify_range(
                root,
                relative,
                file.maximum(),
                digest,
                Some((request.offset, request.length)),
                stop,
            )?;
            if size != alias.bytes {
                return Err(invalid("Backup database alias length changed"));
            }
            require_named(root, &alias.root)?;
            result = Some(bytes);
            Ok(())
        },
    )?;
    let after = Plan::capture(context, prepared, decision, expected, stop)?;
    plan.require_same(&after)?;
    after.require_complete(context, prepared, stop)?;
    Ok(BackupDatabaseRange {
        aliases_hash: plan.report.aliases_hash.clone(),
        publication_hash: expected.as_str().into(),
        database: alias.clone(),
        offset: request.offset,
        bytes: result.expect("invariant: exact alias was captured from these same complete roots"),
    })
}
