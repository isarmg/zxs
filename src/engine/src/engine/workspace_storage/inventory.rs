//! Read-only physical manifest diagnostics, independent of engine quota admission.
use super::*;

/// One physical manifest, not a logical workspace or an authorization decision.
#[derive(Debug, data_serialization::Serialize)]
pub struct WorkspaceInventoryRecord {
    pub path: PathBuf,
    pub workspace_id: String,
    pub principal: String,
    pub state: WorkspaceState,
    #[serialization(with = "storage_contract::u64_string")]
    pub canonical_metadata_bytes: u64,
}

/// Streaming offline diagnostics independent of engine quota admission/replay.
/// It retains writer locks, configuration roots and one directory cursor, but no
/// per-ID index or decoded history. Mirrors are deliberately reported separately.
pub struct WorkspaceInventory {
    locations: Vec<WorkspaceLocation>,
    current: Option<(usize, platform_io::fs_safety::linux::DirectoryEntries)>,
    next_location: usize,
    _locks: Vec<fs::File>,
}

impl WorkspaceInventory {
    /// Inspect an existing system root without replay, repair or quota overrides.
    ///
    /// # Errors
    /// Returns configuration, disk identity, lock or filesystem errors. Like
    /// other offline tools, exclusive writer-lock files may be opened/created.
    /// Individual records remain provisional: no journal or mirror agreement is
    /// inferred. Stop the daemon first and exhaust the iterator to finish a scan.
    pub fn open(root: &Path) -> Result<Self> {
        let root = existing_private_dir(root)?.path().to_owned();
        let config = ValidatedConfig::try_from(read_json::<Config>(&root.join("config.json"))?)?;
        let disks = DiskResources::open(&config, true)?;
        let locks = crate::engine::config::acquire_locks(&root, &disks)?;
        Ok(Self {
            locations: workspace_locations(&root, &config, &disks)?,
            current: None,
            next_location: 0,
            _locks: locks,
        })
    }

    fn next_record(&mut self) -> Result<Option<WorkspaceInventoryRecord>> {
        loop {
            let Some((index, entries)) = &mut self.current else {
                let Some(location) = self.locations.get(self.next_location) else {
                    return Ok(None);
                };
                let index = self.next_location;
                self.next_location += 1;
                match location.root().entries(&location.directory()) {
                    Ok(entries) => self.current = Some((index, entries)),
                    Err(error) if workspace_absent(&error) => {}
                    Err(error) => return Err(error),
                }
                continue;
            };
            let Some(entry) = entries.next() else {
                self.current = None;
                continue;
            };
            let entry = entry?;
            let location = &self.locations[*index];
            let directory = location.directory().join(entry.as_path());
            if !workspace_directory(location.root(), &directory)? {
                continue;
            }
            let path = directory.join("workspace.json");
            let file = match location.source(path.clone()).open() {
                Ok(file) => file,
                Err(error) if workspace_absent(&error) => continue,
                Err(error) => return Err(error),
            };
            let record = (|| {
                let id = entry
                    .as_os_str()
                    .to_str()
                    .ok_or_else(|| recovery_required("invalid workspace directory identity"))?;
                validate_id(id, "ws")?;
                let workspace: Workspace = decode_strict(&read_file_bytes(file, MAX_METADATA)?)?;
                if workspace.workspace_id != id || workspace.fencing_token == 0 {
                    return Err(recovery_required(
                        "workspace manifest identity or fence is invalid",
                    ));
                }
                let canonical_metadata_bytes = canonical_json(&workspace)?.len() as u64;
                Ok(WorkspaceInventoryRecord {
                    path: path.clone(),
                    workspace_id: workspace.workspace_id,
                    principal: workspace.principal,
                    state: workspace.state,
                    canonical_metadata_bytes,
                })
            })();
            return record.map(Some).map_err(|error: Error| {
                error.context(
                    ErrorCode::RecoveryRequired,
                    format!("workspace inventory: {}", path.display()),
                )
            });
        }
    }
}

impl Iterator for WorkspaceInventory {
    type Item = Result<WorkspaceInventoryRecord>;

    fn next(&mut self) -> Option<Self::Item> {
        self.next_record().transpose()
    }
}
