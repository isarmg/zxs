//! Abort promotion retains only the effective original roots and locks.
use super::*;
use crate::engine::membership_transition::archive::persistent::PendingControlArchive;
use crate::persist::layout;

impl Engine {
    pub(in crate::engine::membership_transition) fn install_certified_abort(
        &mut self,
        portable: PortableAuthority,
        source: &PendingControlArchive,
    ) -> Result<()> {
        let intent = portable.intent()?;
        let transition = object_hash(INTENT_KIND, &intent)?;
        let (base, history) = source.generation_base(&intent)?;
        let generation = Arc::new(Generation {
            backup_continuation: None,
            hash: portable.hash()?,
            workspace_roots: portable.workspace_root_aliases()?,
            original: portable.genesis_config()?,
            epoch: portable.epoch()?,
            portable,
            intent,
            history,
            base: Arc::new(base),
        });
        generation.next_state()?;
        if generation.registry().is_some() {
            generation.portable.prepare_namespace_activation(
                &generation.base,
                self.current_namespaces.as_deref(),
            )?;
        }
        PreparedMetadataObject::new(
            "membership-generation-bundle",
            &generation.portable,
            MAX_METADATA,
        )?;
        let effective = generation.portable.effective_config()?;
        let roots = effective
            .disks
            .iter()
            .map(|disk| {
                self.config
                    .disks
                    .iter()
                    .find(|held| held.disk_id == disk.disk_id)
                    .expect("invariant: Abort retains original configured members")
                    .root
                    .clone()
            })
            .collect();
        let config = effective
            .with_disk_roots(roots)?
            .with_workspace_limits(Some(self.config.effective_workspace_limits()))?
            .with_generation(Arc::clone(&generation))?;
        let config = generation.portable.restrict_io(config)?;
        let fence = self
            .membership_fence
            .as_ref()
            .expect("invariant: certification retains original fence")
            .clone();
        for disk in config.metadata_members() {
            let Some(handle) = self.disk_resources.available(disk) else {
                continue;
            };
            admission::verify_named_root(&handle.root)?;
            self._locks.verify_disk(disk, &handle.root)?;
            let bytes = handle
                .root
                .read_bytes(&disk.root.join(layout::DISK_IDENTITY.as_path()), MAX_METADATA)?;
            let expected = identity_at(&config, disk, &handle.root.directory(handle.root.path())?)?;
            predecessor_seal_matches(&config, disk, &handle.root, &bytes, &expected)?;
            let path = member_directory(disk, &transition).join(CONTROL_ARCHIVE_DIRECTORY);
            generation
                .portable
                .verify_abort_member(&handle.root, disk)?;
            source.verify_installed(handle.root.private_child(&path)?)?;
            source.install_generation_objects(&handle.root, disk, &fence)?;
            publication::install_marker(&handle.root, disk, &generation)?;
            #[cfg(feature = "fault-injection")]
            self.failpoint(&format!("after_membership_abort_marker_{}", disk.disk_id))?;
        }
        self.resume_abort_generation(config)
    }

    /// Explicit recovery may complete missing independent archive copies using
    /// one fully verified old copy. Ordinary discovery never writes this closure.
    pub(super) fn restore_abort_archives(&self, generation: &Generation) -> Result<()> {
        let transition = object_hash(INTENT_KIND, &generation.intent)?;
        let archive_hash = generation.portable.archive_hash()?;
        let mut source = None;
        let mut failure = None;
        for disk in self.config.metadata_members() {
            let root = &self.disk_resources.get(disk)?.root;
            let admitted = root
                .private_child(&member_directory(disk, &transition).join(CONTROL_ARCHIVE_DIRECTORY))
                .and_then(|directory| {
                    PendingControlArchive::from_installed(directory, &archive_hash, &transition)
                });
            match admitted {
                Ok(archive) => {
                    source = Some(archive);
                    break;
                }
                Err(error) => {
                    failure.get_or_insert(error);
                }
            }
        }
        let source = source.ok_or_else(|| {
            failure.unwrap_or_else(|| {
                Error::new(
                    ErrorCode::RecoveryRequired,
                    "Abort promotion has no complete original archive",
                )
            })
        })?;
        for disk in self.config.metadata_members() {
            let root = &self.disk_resources.get(disk)?.root;
            self._locks.verify_disk(disk, root)?;
            generation.portable.verify_abort_member(root, disk)?;
            let path = member_directory(disk, &transition).join(CONTROL_ARCHIVE_DIRECTORY);
            root.private_dir(&path)?;
            source.install(root.private_child(&path)?)?;
        }
        Ok(())
    }

    pub(super) fn resume_abort_generation(&mut self, config: ValidatedConfig) -> Result<()> {
        let generation = config
            .generation()
            .expect("invariant: certified Abort config");
        let fence = self.membership_fence.as_ref().ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "Abort adoption requires the original admitted fence",
            )
        })?;
        if !generation.portable.is_abort()
            || object_hash(INTENT_KIND, &generation.intent)? != fence.hash
            || self.config.generation_hash() != generation.intent.origin.predecessor()
            || !authority_config_matches(&self.config, generation.intent.origin.config())
        {
            return Err(Error::new(
                ErrorCode::LogFork,
                "Abort authority differs from held fence",
            ));
        }
        let handles = self.disk_resources.handles().map(Arc::clone).collect();
        let resources = DiskResources::generation_handles(&config, handles)?;
        let base = Arc::clone(&generation.base);
        let snapshot = generation.snapshot();
        let sequence = generation.first_sequence()?;
        self.config = config;
        self.named_namespaces.install_current(&mut self.current_namespaces, None);
        self.disk_resources = Arc::new(resources);
        // No original journal is modified. Only the new G gets a writer later.
        self.journals.clear();
        self.membership_fence = None;
        self.pending_membership = None;
        self.named_namespaces.install_state(&mut self.state, base);
        self.snapshot = snapshot;
        self.next_slot = sequence;
        self.blocked = true;
        Ok(())
    }
}
