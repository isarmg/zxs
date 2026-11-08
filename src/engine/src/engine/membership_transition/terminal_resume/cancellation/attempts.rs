//! The original resume.json is never replaced. Its cancellation certificate
//! selects the only next attempt name by hash, without a mutable or guessed head.
use super::*;
use std::path::PathBuf;

pub(in crate::engine) const CANCEL_FILE: &str = "resume-cancel.json";
const ATTEMPTS: &str = "resume-attempts";
const PROPOSAL: &str = "resume.json";

pub(in crate::engine) struct ResumeAttempts {
    pub(in crate::engine) path: PathBuf,
    pub(in crate::engine::membership_transition) current: Option<ResumeProposal>,
    pub(in crate::engine) cancelled: Vec<ResumeCancellation>,
}

pub(in crate::engine) fn operation_directory(
    system: &DiskRoot,
    terminal: &TerminalAuthority,
) -> PathBuf {
    system
        .path()
        .join("terminal-management")
        .join(terminal.operation_id().to_string())
}

pub(in crate::engine) fn load(
    system: &DiskRoot,
    verified: &VerifiedTerminalAuthority,
) -> Result<ResumeAttempts> {
    let operation = operation_directory(system, &verified.authority);
    let namespace = operation.join(ATTEMPTS);
    let mut path = operation.clone();
    let mut cancelled = Vec::new();
    let mut visited = BTreeSet::new();
    let authority_hash = verified.authority.hash()?;
    let mut previous_hash = None;
    let mut operations = BTreeSet::from([verified.authority.operation_id()]);
    // A valid writer preflights the previous complete core and the next
    // cancellation independently. Bound their combined persisted closure too.
    let mut cancellation_bytes = 0usize;
    let current = loop {
        if path != operation
            && (system.optional_metadata(&namespace)?.is_none()
                || system.optional_metadata(&path)?.is_none())
        {
            break None;
        }
        let root = system.private_child(&path)?;
        inspect_inventory(&root, path == operation)?;
        let Some(file) = root.open_optional_read_regular(&path.join(PROPOSAL))? else {
            for name in [
                CANCEL_FILE,
                "resume-cancel-intent.json",
                "resume-generation.json",
                "resume-bootstrap.json",
                "resume-complete.json",
            ] {
                if root.open_optional_read_regular(&path.join(name))?.is_some() {
                    return Err(Error::new(
                        ErrorCode::PublicationInvalid,
                        "resume attempt evidence has no exact protected request",
                    ));
                }
            }
            break None;
        };
        let proposal: ResumeProposal = decode_strict(&read_file_bytes(file, MAX_METADATA)?)?;
        proposal.verify()?;
        proposal.verify_source(&verified.state)?;
        if proposal.terminal.hash()? != authority_hash
            || proposal.previous_cancellation_hash != previous_hash
            || !operations.insert(proposal.intent.operation_id)
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "terminal resume attempt does not extend its exact cancelled predecessor",
            ));
        }
        let cancellation_intent =
            root.open_optional_read_regular(&path.join("resume-cancel-intent.json"))?;
        let cancellation_started = cancellation_intent.is_some();
        if let Some(file) = cancellation_intent {
            let intent: ResumeCancellationIntent =
                decode_strict(&read_file_bytes(file, MAX_METADATA)?)?;
            intent.verify(&proposal)?;
            for name in [
                "resume-generation.json",
                "resume-bootstrap.json",
                "resume-complete.json",
            ] {
                if root.open_optional_read_regular(&path.join(name))?.is_some() {
                    return Err(Error::new(
                        ErrorCode::LogFork,
                        "cancelling resume also selected generation evidence",
                    ));
                }
            }
        }
        let Some(file) = root.open_optional_read_regular(&path.join(CANCEL_FILE))? else {
            break Some(proposal);
        };
        if !cancellation_started {
            return Err(Error::new(
                ErrorCode::PublicationInvalid,
                "cancelled resume lost its exact cancellation intent",
            ));
        }
        let bytes = read_file_bytes(file, MAX_METADATA)?;
        if bytes.len() > 2 * MAX_METADATA - cancellation_bytes {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "terminal resume cancellation history exceeds its bounded control closure",
            ));
        }
        cancellation_bytes += bytes.len();
        let cancellation: ResumeCancellation = decode_strict(&bytes)?;
        cancellation.verify()?;
        if cancellation.proposal.hash()? != proposal.hash()? {
            return Err(Error::new(
                ErrorCode::LogFork,
                "cancellation does not name the immutable selected resume request",
            ));
        }
        for name in [
            "resume-generation.json",
            "resume-bootstrap.json",
            "resume-complete.json",
        ] {
            if root.open_optional_read_regular(&path.join(name))?.is_some() {
                return Err(Error::new(
                    ErrorCode::LogFork,
                    "cancelled resume also selects active generation evidence",
                ));
            }
        }
        let hash = cancellation.hash()?;
        if !visited.insert(hash.clone()) {
            return Err(Error::new(
                ErrorCode::LogFork,
                "cyclic resume cancellation attempts",
            ));
        }
        cancelled.push(cancellation);
        path = namespace.join(&hash);
        previous_hash = Some(hash);
    };
    if system.optional_metadata(&namespace)?.is_some() {
        let root = system.private_child(&namespace)?;
        for entry in root.entries(&namespace)? {
            let entry = entry.map_err(fs_error)?;
            let name = entry.as_os_str().to_str().ok_or_else(|| {
                Error::new(ErrorCode::RecoveryRequired, "invalid resume attempt name")
            })?;
            if !visited.contains(name) {
                return Err(Error::new(
                    ErrorCode::RecoveryRequired,
                    "unselected or unknown resume attempt evidence",
                ));
            }
            root.private_directory(&namespace.join(entry.as_path()))?;
        }
    }
    Ok(ResumeAttempts {
        path,
        current,
        cancelled,
    })
}

impl ResumeAttempts {
    pub(in crate::engine) fn has_current(&self) -> bool {
        self.current.is_some()
    }
    pub(in crate::engine::membership_transition) fn previous_hash(&self) -> Result<Option<String>> {
        self.cancelled
            .last()
            .map(ResumeCancellation::hash)
            .transpose()
    }

    pub(in crate::engine::membership_transition) fn persist_request(
        &self,
        system: &DiskRoot,
        proposal: &ResumeProposal,
    ) -> Result<()> {
        if let Some(current) = &self.current
            && current.hash()? != proposal.hash()?
        {
            return Err(Error::new(
                ErrorCode::IdempotencyKeyReused,
                "terminal authority reserves another exact resume attempt",
            ));
        }
        verify_history(&proposal.terminal, &self.cancelled, Some(proposal))?;
        system.private_dir(&self.path)?;
        system.write_json(&self.path.join(PROPOSAL), proposal, false)
    }
}

pub(in crate::engine) fn selected_path(
    system: &DiskRoot,
    verified: &VerifiedTerminalAuthority,
) -> Result<PathBuf> {
    let attempts = load(system, verified)?;
    if attempts.current.is_none() {
        return Err(Error::new(
            ErrorCode::RecoveryRequired,
            "terminal has no uncancelled resume request",
        ));
    }
    Ok(attempts.path)
}

pub(in crate::engine) fn synchronize_history(
    system: &DiskRoot,
    verified: &VerifiedTerminalAuthority,
) -> Result<()> {
    let attempts = load(system, verified)?;
    let mut path = operation_directory(system, &verified.authority);
    for cancellation in &attempts.cancelled {
        for name in [PROPOSAL, "resume-cancel-intent.json", CANCEL_FILE] {
            system.open_regular(&path.join(name))?.sync_all()?;
        }
        system.sync_dir(&path)?;
        path = operation_directory(system, &verified.authority)
            .join(ATTEMPTS)
            .join(cancellation.hash()?);
    }
    if attempts.current.is_some() {
        system
            .open_regular(&attempts.path.join(PROPOSAL))?
            .sync_all()?;
        system.sync_dir(&attempts.path)?;
    }
    let namespace = operation_directory(system, &verified.authority).join(ATTEMPTS);
    if system.optional_metadata(&namespace)?.is_some() {
        system.sync_dir(&namespace)?;
        system.sync_dir(&operation_directory(system, &verified.authority))?;
    }
    Ok(())
}

fn inspect_inventory(root: &DiskRoot, initial: bool) -> Result<()> {
    for entry in root.entries(root.path())? {
        let entry = entry.map_err(fs_error)?;
        let name = entry.as_os_str().to_str().ok_or_else(|| {
            Error::new(ErrorCode::RecoveryRequired, "invalid resume attempt entry")
        })?;
        if matches!(
            name,
            PROPOSAL
                | CANCEL_FILE
                | "resume-cancel-intent.json"
                | "resume-generation.json"
                | "resume-bootstrap.json"
                | "resume-complete.json"
        ) {
            continue;
        }
        if initial
            && (matches!(
                name,
                "authority.json" | "complete.json" | "bootstrap-stop.json" | ATTEMPTS
            ) || crate::engine::disk_lifecycle::terminal::retirement::owns_initial_entry(name))
        {
            continue;
        }
        return Err(Error::new(
            ErrorCode::RecoveryRequired,
            "unknown or unfinished resume attempt write",
        ));
    }
    Ok(())
}
