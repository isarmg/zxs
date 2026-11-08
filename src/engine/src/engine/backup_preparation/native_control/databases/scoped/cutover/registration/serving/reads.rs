//! Resolve a published HEAD entry to that same target's actual complete entity.
//! Reads do not authorize publication, namespace mutation or source release.
use super::*;

/// The host authenticates the requested scope. Exact HEAD, entry and version
/// keep a stale listing from being interpreted as a different content request.
#[derive(Clone, Debug)]
pub struct BackupServingRead {
    pub target_slot_id: String,
    pub scope: BackupNamespaceScope,
    pub head_id: String,
    pub entry_id: String,
    pub version_id: String,
    pub offset: u64,
    pub length: usize,
}

/// Bytes read from the specified published serving image. No file descriptor,
/// mutable State or target writer capability escapes this bounded boundary.
#[derive(Clone, Debug)]
pub struct BackupServingContent {
    pub publication_hash: String,
    pub registry: DiskRegistryCommitment,
    pub target_slot_id: String,
    pub scope: BackupNamespaceScope,
    pub head_id: String,
    pub entry: Entry,
    pub version: FileVersion,
    pub relative_container: PathBuf,
    pub offset: u64,
    pub bytes: Vec<u8>,
}

impl BackupServingRead {
    fn validate(&self) -> Result<()> {
        name_policy::validate_slot(&self.target_slot_id)?;
        self.scope.validate()?;
        match &self.scope {
            BackupNamespaceScope::BranchHead(_) => validate_id(&self.head_id, "commit")?,
            BackupNamespaceScope::DirectoryHead(_) => validate_id(&self.head_id, "dtree")?,
        }
        validate_id(&self.entry_id, "entry")?;
        validate_id(&self.version_id, "ver")?;
        if self.length > crate::container_storage::backup_entities::MAX_RECOVERY_RANGE {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "published serving range exceeds 4 MiB",
            ));
        }
        Ok(())
    }

    fn entry<'a>(&self, state: &'a State) -> Result<&'a Entry> {
        let (head, entries) = self.scope.entries(state)?;
        if head != self.head_id {
            return Err(Error::new(
                ErrorCode::BaseHeadMismatch,
                "published serving HEAD differs from the request",
            ));
        }
        let entry = entries
            .iter()
            .find(|entry| entry.entry_id == self.entry_id)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::FileNotFound,
                    "entry is not in the requested published HEAD",
                )
            })?;
        if entry.kind != NamespaceEntryKind::File || entry.version_id != self.version_id {
            return Err(Error::new(
                ErrorCode::InvalidRequest,
                "entry is not the requested published file version",
            ));
        }
        if state.file_deletions.contains_key(&entry.file_id) {
            return Err(Error::new(
                ErrorCode::FileNotFound,
                "soft-deleted file is not available through a serving namespace",
            ));
        }
        match state.content_states.get(&entry.version_id) {
            None | Some(ContentState::Available) => Ok(entry),
            Some(ContentState::Purged | ContentState::Migrated | ContentState::Retired) => {
                Err(Error::new(
                    ErrorCode::ContentUnavailable,
                    "published version has no available serving content",
                ))
            }
        }
    }
}

pub(in super::super) fn read_entry(
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    expected: &BackupFirstPublicationHash,
    request: &BackupServingRead,
) -> Result<BackupServingContent> {
    request.validate()?;
    let index = prepared
        .projection
        .targets
        .iter()
        .position(|target| target.node.identity.slot_id == request.target_slot_id)
        .ok_or_else(|| {
            Error::new(
                ErrorCode::DiskNotFound,
                "serving slot is not in the original complete target set",
            )
        })?;
    let stop = AtomicBool::new(false);
    let plan = Plan::capture(prepared, decision, expected, &stop)?;
    plan.observe(prepared, decision, expected, &stop)?;
    let result = plan.with_image(prepared, index, &stop, |image, state| {
        let entry = request.entry(state)?;
        let version = state
            .versions
            .get(&entry.version_id)
            .expect("invariant: admitted published namespace covers its file version");
        if entry.file_id != version.file_id {
            return Err(invalid("serving entry crosses a file identity"));
        }
        let home = image.file_home(
            &version.history_tree_id,
            &version.file_id,
            &version.version_id,
        )?;
        // Image verification also binds the actual file/directory SQLite and
        // all historical nodes. Range bytes are withheld until the same open
        // inode has been read and hashed in full, including a zero-length read.
        image.verify(&prepared.targets[index].root, &stop)?;
        let (version, bytes) = image.read_version_range(
            &prepared.targets[index].root,
            &entry.version_id,
            request.offset,
            request.length,
            &stop,
        )?;
        Ok(BackupServingContent {
            publication_hash: plan
                .registry
                .configuration()
                .publication
                .publication
                .publication_hash
                .clone(),
            registry: plan.registry.configuration().registry.clone(),
            target_slot_id: request.target_slot_id.clone(),
            scope: request.scope.clone(),
            head_id: request.head_id.clone(),
            entry: entry.clone(),
            version,
            relative_container: home.to_owned(),
            offset: request.offset,
            bytes,
        })
    })?;
    plan.observe(prepared, decision, expected, &stop)?;
    Ok(result)
}
