//! Bounded reads of the exact first published namespaces on one selected target.
//! No mutable CurrentNamespaces/State/Config escapes this frozen read boundary.
use super::*;

/// Identity scope, not a caller-provided path, historical commit or write plan.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BackupNamespaceScope {
    BranchHead(String),
    DirectoryHead(String),
}

/// One frozen read page. A soft-deleted entry retains its exact D node; content
/// retirement remains visible and never creates permission to read a body.
#[derive(Clone, Debug)]
pub struct BackupNamespacePage {
    pub publication_hash: String,
    pub registry: crate::DiskRegistryCommitment,
    pub target_slot_id: String,
    pub scope: BackupNamespaceScope,
    pub head_id: String,
    pub entries: Vec<Entry>,
    pub file_deletions: BTreeMap<String, FileDeletionNode>,
    pub content_states: BTreeMap<String, ContentState>,
    pub next_offset: Option<usize>,
}

impl BackupNamespaceScope {
    fn validate(&self) -> Result<()> {
        match self {
            Self::BranchHead(id) => validate_id(id, "br")?,
            Self::DirectoryHead(id) => validate_id(id, "dbr")?,
        }
        Ok(())
    }
    fn entries<'a>(&self, state: &'a State) -> Result<(&'a str, &'a [Entry])> {
        match self {
            Self::BranchHead(id) => {
                let branch = state.branches.get(id).ok_or_else(|| {
                    Error::new(
                        ErrorCode::BranchNotFound,
                        "published Backup branch is absent",
                    )
                })?;
                if branch.state == BranchState::Migrated {
                    return Err(Error::new(
                        ErrorCode::TreeMoved,
                        "published Backup branch was migrated",
                    ));
                }
                let commit = state
                    .commits
                    .get(&branch.head_commit_id)
                    .expect("invariant: admitted full snapshot covers every branch HEAD");
                Ok((&branch.head_commit_id, &commit.entries))
            }
            Self::DirectoryHead(id) => {
                let branch = state.directory_branches.get(id).ok_or_else(|| {
                    Error::new(
                        ErrorCode::DirectoryBranchNotFound,
                        "published Backup directory is absent",
                    )
                })?;
                Ok((&branch.head_tree_id, &branch.entries))
            }
        }
    }
}

pub(in super::super) fn read_page(
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    expected: &BackupFirstPublicationHash,
    slot: &str,
    scope: BackupNamespaceScope,
    offset: usize,
    limit: usize,
) -> Result<BackupNamespacePage> {
    name_policy::validate_slot(slot)?;
    scope.validate()?;
    if !(1..=256).contains(&limit) {
        return Err(Error::new(
            ErrorCode::InvalidRequest,
            "Backup namespace page limit must be 1..=256",
        ));
    }
    let stop = AtomicBool::new(false);
    let plan = Plan::capture(prepared, decision, expected, &stop)?;
    observe_with_plan(prepared, decision, &plan, &stop)?;
    let index = prepared
        .projection
        .targets
        .iter()
        .position(|target| target.node.identity.slot_id == slot)
        .ok_or_else(|| {
            Error::new(
                ErrorCode::InvalidRequest,
                "namespace target is outside the original complete selection",
            )
        })?;
    let target = &prepared.targets[index];
    // The chosen target supplies BOTH its actual graph and native snapshot.
    // Target zero or a source disk is never a missing-page fallback.
    let registry = plan.verify(target, &stop)?;
    if registry.registration(&prepared.projection.targets[index].node.identity)?
        != Some(crate::RegistrationMode::Enabled)
    {
        return Err(invalid(
            "published target is not registered in its exact graph",
        ));
    }
    let admitted = plan.state_at(target, &stop)?;
    let state = admitted.state();
    let (head, entries) = scope.entries(state)?;
    if offset > entries.len() {
        return Err(Error::new(
            ErrorCode::InvalidRequest,
            "Backup namespace offset exceeds its frozen scope",
        ));
    }
    let end = offset + limit.min(entries.len() - offset);
    let mut file_deletions = BTreeMap::new();
    let mut content_states = BTreeMap::new();
    let mut bytes = 0usize;
    for entry in &entries[offset..end] {
        bytes = bytes
            .checked_add(canonical_json(entry)?.len())
            .ok_or_else(budget)?;
        if entry.kind == NamespaceEntryKind::File {
            if let Some(deletion) = state.file_deletions.get(&entry.file_id)
                && !file_deletions.contains_key(&entry.file_id)
            {
                bytes = bytes
                    .checked_add(canonical_json(deletion)?.len())
                    .ok_or_else(budget)?;
                file_deletions.insert(entry.file_id.clone(), deletion.clone());
            }
            let content = state
                .content_states
                .get(&entry.version_id)
                .cloned()
                .unwrap_or(ContentState::Available);
            content_states.insert(entry.version_id.clone(), content);
        }
        if bytes > 4 * 1024 * 1024 {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "Backup namespace entry/deletion bytes exceed 4 MiB; request fewer entries",
            ));
        }
    }
    let result = BackupNamespacePage {
        publication_hash: plan
            .resolved
            .report
            .publication
            .publication
            .publication_hash
            .clone(),
        registry: registry.commitment().clone(),
        target_slot_id: slot.to_owned(),
        scope: scope.clone(),
        head_id: head.to_owned(),
        entries: entries[offset..end].to_vec(),
        file_deletions,
        content_states,
        next_offset: (end < entries.len()).then_some(end),
    };
    // Reobserve actual Publication/payloads and all installed registries after
    // the selected snapshot read. The host must still authorize every request.
    first_publication::observe(prepared, decision, expected, &stop)?;
    observe_with_plan(prepared, decision, &plan, &stop)?;
    Ok(result)
}
