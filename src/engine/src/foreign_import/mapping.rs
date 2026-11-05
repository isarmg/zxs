use super::*;
use name_policy::PortablePath;
use std::collections::BTreeSet;

impl ForeignImportPreview {
    /// Bind every complete source history to a distinct child of one empty
    /// destination directory. This only reads the admitted namespace and creates
    /// a review digest; it creates no directory and has no execute counterpart.
    pub fn map_to_primary(
        &self,
        engine: &Engine,
        slot: &str,
        parent: Option<&PortablePath>,
        names: &BTreeMap<String, String>,
    ) -> Result<ForeignImportMapping> {
        self.revalidate_source()?;
        engine.require_write()?;
        let registered = engine
            .registered_disks()
            .find(|disk| disk.slot_id.eq_ignore_ascii_case(slot))
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::DiskNotFound,
                    "foreign reconstruction target is not registered",
                )
            })?;
        if registered.registration_mode != RegistrationMode::Enabled
            || registered.level != 1
            || registered.upstream_slot_id.is_some()
        {
            return Err(invalid(
                "history reconstruction requires an enabled primary destination",
            ));
        }
        let target = registered.identity();
        let disk = engine.disk(&target.slot_id)?;
        let held = engine.disk_resources.get(disk)?;
        let physical = held.root.directory_identity(held.root.path())?;
        if (physical.device == self.local.device && physical.inode == self.local.inode)
            || self.root.path().starts_with(&disk.root)
            || disk.root.starts_with(self.root.path())
        {
            return Err(invalid("foreign source and reconstruction target overlap"));
        }
        if let Some(namespaces) = engine.admitted_current_namespaces()? {
            namespaces.require_write_slot(&target.slot_id)?;
        }
        let repository = engine.state.repositories.values().find(|repository| {
            repository.file_manager_root && repository.primary_slot_id == target.slot_id
        });
        let (branch_id, head, entries) = match repository {
            Some(repository) => {
                let branch = engine
                    .state
                    .branches
                    .get(&repository.main_branch_id)
                    .ok_or_else(|| invalid("target root branch is missing"))?;
                if branch.state != BranchState::Active || branch.file_id.is_some() {
                    return Err(invalid("target root branch is not active"));
                }
                (
                    Some(branch.branch_id.as_str()),
                    Some(branch.head_commit_id.as_str()),
                    engine.list_tree_ref(&branch.branch_id)?,
                )
            }
            None => (None, None, &[][..]),
        };
        match parent {
            None if !entries.is_empty() => {
                return Err(Error::new(
                    ErrorCode::DirectoryNotEmpty,
                    "target root is not empty",
                ));
            }
            Some(parent) => {
                let entry = entries
                    .iter()
                    .find(|entry| entry.lookup_key() == parent.key())
                    .ok_or_else(|| {
                        Error::new(
                            ErrorCode::ParentNotDirectory,
                            "reconstruction parent does not exist",
                        )
                    })?;
                if entry.kind != NamespaceEntryKind::Directory {
                    return Err(Error::new(
                        ErrorCode::ParentNotDirectory,
                        "reconstruction parent is not a directory",
                    ));
                }
                let prefix = format!("{}/", parent.key());
                if entries
                    .iter()
                    .any(|entry| entry.lookup_key().starts_with(&prefix))
                {
                    return Err(Error::new(
                        ErrorCode::DirectoryNotEmpty,
                        "reconstruction parent is not empty",
                    ));
                }
            }
            None => {}
        }
        if names.len() != self.histories.len()
            || self
                .histories
                .iter()
                .any(|history| !names.contains_key(&history.group_id))
        {
            return Err(invalid(
                "target mapping must cover exactly the verified source histories",
            ));
        }
        let mut seen = BTreeSet::new();
        let mut histories = Vec::new();
        for history in &self.histories {
            let name = &names[&history.group_id];
            name_policy::validate_name(name)?;
            if !seen.insert(name_policy::lookup_key(name)) {
                return Err(Error::new(
                    ErrorCode::NameCollision,
                    "source histories map to colliding target names",
                ));
            }
            if engine
                .state
                .histories
                .contains_key(&history.manifest.history.history_tree_id)
            {
                return Err(Error::new(
                    ErrorCode::ObjectCollision,
                    "source history identity already exists in the target pool",
                ));
            }
            let path = PortablePath::parse(&parent.map_or_else(
                || name.clone(),
                |parent| format!("{}/{name}", parent.as_str()),
            ))?;
            histories.push(ForeignMappedHistory {
                group_id: history.group_id.clone(),
                history_tree_id: history.manifest.history.history_tree_id.clone(),
                path,
            });
        }
        let paths = histories
            .iter()
            .map(|history| (&history.group_id, history.path.as_str()))
            .collect::<BTreeMap<_, _>>();
        let digest = object_hash("foreign-target-mapping", &json_codec::json!({
            "source":self.digest,"target":target,"sequence":engine.state.sequence.to_string(),"branch":branch_id,"head":head,"paths":paths
        }))?.parse().expect("invariant: object hash is a content hash");
        Ok(ForeignImportMapping {
            source_digest: self.digest,
            digest,
            target,
            target_sequence: engine.state.sequence,
            histories,
        })
    }
}
