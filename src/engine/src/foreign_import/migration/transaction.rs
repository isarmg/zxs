use super::*;

#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serialization(deny_unknown_fields)]
struct StageBinding {
    identity: PhysicalBinding,
    intent_digest: String,
}

impl ForeignMigrationPlan {
    /// Reserve a new capsule name beneath an existing private parent. The
    /// durable parent intent precedes staging creation, so even that first mkdir
    /// window can be resumed without deleting or adopting unrelated contents.
    pub fn begin(self, destination: &Path) -> Result<ForeignMigrationTransaction> {
        let (parent, name) = open_parent(destination)?;
        let lock = AdvisoryLock::directory(&parent.directory(parent.path())?)?;
        reject_overlap(&self.record, &parent, destination)?;
        let stage = staging_path(&parent, &self.record.operation_id);
        let reservation = reservation_path(&parent, &self.record.operation_id);
        require_absent(&parent, &parent.path().join(&name))?;
        require_absent(&parent, &stage)?;
        require_absent(&parent, &reservation)?;
        let intent = Intent {
            plan: self.record,
            destination: name,
            parent_path: parent.path().to_owned(),
            parent: PhysicalBinding::of(&parent)?,
        };
        parent.write_json(&reservation, &intent, false)?;
        open_reserved(parent, intent, self.digest, lock)
    }
}

impl ForeignMigrationTransaction {
    /// Recover only the caller's exact reviewed operation and digest. A final
    /// capsule is verified before retry success; a conflicting final or partial
    /// capsule is preserved and rejected. No source is needed after sealing.
    pub fn resume(
        destination: &Path,
        operation: &OperationId,
        expected: &ForeignEvidenceDigest,
    ) -> Result<Self> {
        let (parent, name) = open_parent(destination)?;
        let lock = AdvisoryLock::directory(&parent.directory(parent.path())?)?;
        let intent: Intent =
            decode_strict(&parent.read_bytes(&reservation_path(&parent, operation), MAX_INTENT)?)?;
        intent.plan.validate()?;
        if intent.destination != name
            || intent.parent_path != parent.path()
            || intent.plan.operation_id != *operation
            || digest(&intent.plan)? != *expected
            || intent.parent != PhysicalBinding::of(&parent)?
        {
            return Err(changed(
                "migration reservation does not match this reviewed operation and parent",
            ));
        }
        parent.sync_dir(parent.path())?;
        open_reserved(parent, intent, *expected, lock)
    }

    /// Copy independently into the reserved tree. Existing exact bytes are
    /// verified and synced; conflicting bytes are never replaced or removed.
    pub fn copy_source(&mut self, source: &ForeignImportPreview) -> Result<()> {
        if self.published {
            return Err(invalid("published capsule is immutable"));
        }
        self.check_names()?;
        if source.root.path() != self.intent.plan.source_path
            || PhysicalBinding::of(&source.root)? != self.intent.plan.source_local
        {
            return Err(changed(
                "copy source is not the physical directory selected by the plan",
            ));
        }
        self.intent.plan.check_replica(source)?;
        source.revalidate_source()?;
        let path = self.capsule.path().join("source");
        self.capsule.private_dir(&path)?;
        let replica = self.capsule.private_child(&path)?;
        self.intent.plan.inventory.quarantine_residue(
            &replica,
            &self.capsule,
            "source",
            &self.intent.plan.budget,
        )?;
        self.intent.plan.inventory.copy(&source.root, &replica)?;
        self.capsule.sync_dir(self.capsule.path())
    }

    /// Verify the complete retained evidence, graph, provenance and availability
    /// on the copy, synchronize it, then create an immutable completion marker.
    pub fn seal(&mut self) -> Result<()> {
        if self.published {
            return Err(invalid("published capsule is immutable"));
        }
        self.check_names()?;
        let replica = self.verify_replica()?;
        self.intent.plan.inventory.sync(&replica.root)?;
        projection::Projection::from_replica(&self.intent.plan, &replica)?
            .install(&self.capsule, &self.intent.plan.budget)?;
        ensure_exact(
            &self.capsule,
            &self.capsule.path().join("intent.json"),
            &canonical_json(&self.intent)?,
        )?;
        let marker = self.marker()?;
        ensure_exact(
            &self.capsule,
            &self.capsule.path().join("sealed.json"),
            &marker,
        )?;
        self.capsule.sync_dir(self.capsule.path())
    }

    /// Atomically publish a sealed capsule. Engine preconditions are checked at
    /// this boundary for rebuild/secondary plans. A post-rename error retains an
    /// uncertain outcome: resume the same operation, never start a new one.
    pub fn publish(self, engine: Option<&Engine>) -> Result<ForeignMigrationReceipt> {
        self.check_names()?;
        let marker = self
            .capsule
            .read_bytes(&self.capsule.path().join("sealed.json"), MAX_METADATA)?;
        if marker != self.marker()? {
            return Err(invalid("capsule completion marker differs"));
        }
        let replica = self.verify_replica()?;
        if self
            .capsule
            .read_bytes(&self.capsule.path().join("intent.json"), MAX_INTENT)?
            != canonical_json(&self.intent)?
        {
            return Err(changed("capsule portable intent differs"));
        }
        self.intent.plan.inventory.sync(&replica.root)?;
        projection::Projection::from_replica(&self.intent.plan, &replica)?
            .verify_synced(&self.capsule, &self.intent.plan.budget)?;
        self.capsule
            .open_regular(&self.capsule.path().join("binding.json"))?
            .sync_all()?;
        self.capsule
            .open_regular(&self.capsule.path().join("sealed.json"))?
            .sync_all()?;
        self.capsule.sync_dir(self.capsule.path())?;
        if !self.published {
            self.intent.plan.check_target(&replica, engine)?;
            // This separate marker is written only after target admission has
            // passed. Merely renaming a sealed staging tree cannot manufacture
            // a historic publication receipt that bypasses target validation.
            ensure_exact(
                &self.capsule,
                &self.capsule.path().join("ready.json"),
                &self.marker()?,
            )?;
        } else {
            self.capsule
                .open_regular(&self.capsule.path().join("ready.json"))?
                .sync_all()?;
        }
        let destination = self.parent.path().join(&self.intent.destination);
        if !self.published {
            self.check_names()?;
            self.parent.rename_noreplace_synced(self.capsule.path(), &destination).map_err(|error| {
                error.context(ErrorCode::RecoveryRequired, "capsule publication may have changed its name; resume this operation to determine and synchronize the outcome")
            })?;
        }
        let final_root = self.parent.private_child(&destination)?;
        if PhysicalBinding::of(&final_root)? != PhysicalBinding::of(&self.capsule)? {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "published capsule name no longer identifies the verified directory",
            ));
        }
        DiskRoot::from_directory(
            self.parent.path().into(),
            self.parent.directory(self.parent.path())?,
        )?;
        // Also repeat this barrier after a successful rename whose original
        // response or directory synchronization was lost.
        self.parent.sync_dir(self.parent.path())?;
        Ok(ForeignMigrationReceipt {
            path: destination,
            digest: self.digest,
            origin_count: self.intent.plan.groups.len(),
        })
    }

    fn marker(&self) -> Result<Vec<u8>> {
        Ok(canonical_json(&StageBinding {
            identity: PhysicalBinding::of(&self.capsule)?,
            intent_digest: object_hash("foreign-migration-intent", &self.intent)?,
        })?)
    }

    fn verify_replica(&self) -> Result<ForeignImportPreview> {
        verify_replica(&self.capsule, &self.intent.plan)
    }

    fn check_names(&self) -> Result<()> {
        DiskRoot::from_directory(
            self.parent.path().into(),
            self.parent.directory(self.parent.path())?,
        )?;
        if self.parent.path() != self.intent.parent_path
            || PhysicalBinding::of(&self.parent)? != self.intent.parent
        {
            return Err(changed("capsule parent changed"));
        }
        let held = PhysicalBinding::of(&self.capsule)?;
        let named = self.parent.private_child(self.capsule.path())?;
        if held != PhysicalBinding::of(&named)? {
            return Err(changed("capsule staging directory was replaced"));
        }
        let binding: StageBinding = decode_strict(
            &self
                .capsule
                .read_bytes(&self.capsule.path().join("binding.json"), MAX_METADATA)?,
        )?;
        if binding.identity != held
            || binding.intent_digest != object_hash("foreign-migration-intent", &self.intent)?
        {
            return Err(changed(
                "capsule physical identity or intent binding changed",
            ));
        }
        let reservation = self.parent.read_bytes(
            &reservation_path(&self.parent, &self.intent.plan.operation_id),
            MAX_INTENT,
        )?;
        if reservation != canonical_json(&self.intent)? {
            return Err(changed("migration reservation changed"));
        }
        check_top_level(&self.capsule)?;
        if self.published {
            for name in ["sealed.json", "ready.json"] {
                if self
                    .capsule
                    .read_bytes(&self.capsule.path().join(name), MAX_METADATA)?
                    != self.marker()?
                {
                    return Err(invalid(
                        "final capsule lacks the exact publication-ready evidence",
                    ));
                }
            }
        }
        Ok(())
    }
}

fn open_parent(destination: &Path) -> Result<(DiskRoot, String)> {
    let absolute = absolute_path(destination)?;
    let parent_path = absolute
        .parent()
        .ok_or_else(|| invalid("capsule destination needs a parent"))?;
    let name = absolute
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| invalid("capsule destination needs a UTF-8 name"))?;
    EntryName::new(name)?;
    if name.starts_with(".zxs-foreign-") {
        return Err(invalid("capsule name uses the reserved transaction prefix"));
    }
    Ok((DiskRoot::open(parent_path)?, name.into()))
}

fn staging_path(parent: &DiskRoot, operation: &OperationId) -> PathBuf {
    parent.path().join(format!(".zxs-foreign-{operation}"))
}
fn reservation_path(parent: &DiskRoot, operation: &OperationId) -> PathBuf {
    parent.path().join(format!(".zxs-foreign-{operation}.json"))
}
fn require_absent(root: &DiskRoot, path: &Path) -> Result<()> {
    if root.optional_metadata(path)?.is_some() {
        return Err(Error::new(
            ErrorCode::DestinationExists,
            format!("migration destination already exists: {path:?}"),
        ));
    }
    Ok(())
}

fn open_reserved(
    parent: DiskRoot,
    intent: Intent,
    digest: ForeignEvidenceDigest,
    lock: AdvisoryLock,
) -> Result<ForeignMigrationTransaction> {
    let stage = staging_path(&parent, &intent.plan.operation_id);
    let destination = parent.path().join(&intent.destination);
    let published = parent.optional_metadata(&destination)?.is_some();
    let stage_exists = parent.optional_metadata(&stage)?.is_some();
    if published && stage_exists {
        return Err(changed("both migration staging and destination exist"));
    }
    let path = if published { &destination } else { &stage };
    if !published && !stage_exists {
        let name = EntryName::new(
            stage
                .file_name()
                .expect("invariant: generated staging name"),
        )?;
        rustix::fs::mkdirat(
            parent.directory(parent.path())?,
            name.as_os_str(),
            rustix::fs::Mode::from_raw_mode(0o700),
        )
        .map_err(std::io::Error::from)?;
    }
    let capsule = parent.private_child(path)?;
    capsule.sync_dir(capsule.path())?;
    parent.sync_dir(parent.path())?;
    let binding_path = path.join("binding.json");
    if capsule.optional_metadata(&binding_path)?.is_none() {
        if published {
            return Err(invalid("destination is not a complete migration capsule"));
        }
        // The durable parent reservation owns this name. Before the first
        // binding, only atomic writer residue from that reservation is allowed.
        for entry in capsule.entries(capsule.path())? {
            let entry = entry?;
            if !platform_io::fs_safety::AtomicFile::is_temporary_name(&entry) {
                return Err(changed(
                    "unbound reserved stage contains unrelated material",
                ));
            }
            capsule.open_read_regular(&path.join(entry.as_path()))?;
        }
        let binding = StageBinding {
            identity: PhysicalBinding::of(&capsule)?,
            intent_digest: object_hash("foreign-migration-intent", &intent)?,
        };
        capsule.write_json(&binding_path, &binding, false)?;
    }
    let transaction = ForeignMigrationTransaction {
        parent,
        capsule,
        intent,
        digest,
        published,
        _lock: lock,
    };
    transaction.check_names()?;
    Ok(transaction)
}

pub(super) fn ensure_exact(root: &DiskRoot, path: &Path, bytes: &[u8]) -> Result<()> {
    match root.open_optional_read_regular(path)? {
        Some(file) => {
            if read_file_bytes(file.try_clone()?, bytes.len())? != bytes {
                return Err(changed("existing immutable capsule marker differs"));
            }
            file.sync_all()?;
            root.sync_dir(path.parent().expect("invariant: marker has parent"))
        }
        None => root.write_atomic(path, bytes, false),
    }
}

fn check_top_level(root: &DiskRoot) -> Result<()> {
    let mut count = 0;
    for entry in root.entries(root.path())? {
        count += 1;
        if count > MAX_ENTRIES {
            return Err(invalid("capsule directory entry limit exceeded"));
        }
        let entry = entry?;
        match entry.as_path().to_str() {
            Some("source" | "origins" | "residue") => {
                root.directory_identity(&root.path().join(entry.as_path()))?;
            }
            Some("binding.json" | "sealed.json" | "ready.json" | "intent.json") => {
                root.open_read_regular(&root.path().join(entry.as_path()))?;
            }
            _ if platform_io::fs_safety::AtomicFile::is_temporary_name(&entry) => {
                root.open_read_regular(&root.path().join(entry.as_path()))?;
            }
            _ => return Err(changed("capsule contains unexpected top-level material")),
        }
    }
    Ok(())
}

fn reject_overlap(plan: &PlanRecord, parent: &DiskRoot, destination: &Path) -> Result<()> {
    let destination = absolute_path(destination)?;
    if destination.starts_with(&plan.source_path) || plan.source_path.starts_with(&destination) {
        return Err(invalid("foreign source and capsule destination overlap"));
    }
    let local = PhysicalBinding::of(parent)?;
    let source = DiskRoot::open(&plan.source_path)?;
    if PhysicalBinding::of(&source)? != plan.source_local {
        return Err(changed("original source root changed before staging"));
    }
    if local == plan.source_local {
        return Err(invalid("capsule parent is the source"));
    }
    for path in &plan.inventory.directories {
        let identity = source.directory_identity(&source.path().join(path))?;
        if local.device == identity.device && local.inode == identity.inode {
            return Err(invalid("capsule parent aliases a source descendant"));
        }
    }
    Ok(())
}

fn verify_replica(capsule: &DiskRoot, plan: &PlanRecord) -> Result<ForeignImportPreview> {
    let path = capsule.path().join("source");
    let root = capsule.private_child(&path)?;
    plan.inventory.verify(&root, &plan.budget)?;
    let replica =
        ForeignImportPreview::inspect(root.directory(root.path())?, path, plan.budget.limits()?)?;
    plan.check_replica(&replica)?;
    Ok(replica)
}

impl ForeignMigrationReceipt {
    /// Read-only verification after copying this whole capsule to another
    /// machine/directory. Original physical bindings remain evidence, not an
    /// admission for the new location. Requires the independently retained plan
    /// digest; no original source, parent reservation, Engine or SQLite is used.
    pub fn verify_capsule(path: &Path, expected: &ForeignEvidenceDigest) -> Result<Self> {
        let capsule = DiskRoot::open(path)?;
        let path = capsule.path();
        check_top_level(&capsule)?;
        let intent: Intent =
            decode_strict(&capsule.read_bytes(&path.join("intent.json"), MAX_INTENT)?)?;
        intent.plan.validate()?;
        if digest(&intent.plan)? != *expected {
            return Err(changed(
                "portable capsule plan differs from reviewed digest",
            ));
        }
        let binding: StageBinding =
            decode_strict(&capsule.read_bytes(&path.join("binding.json"), MAX_METADATA)?)?;
        if binding.intent_digest != object_hash("foreign-migration-intent", &intent)? {
            return Err(invalid("portable capsule intent binding differs"));
        }
        let marker = canonical_json(&binding)?;
        for name in ["sealed.json", "ready.json"] {
            if capsule.read_bytes(&path.join(name), MAX_METADATA)? != marker {
                return Err(invalid("portable capsule is not publication-ready"));
            }
        }
        let replica = verify_replica(&capsule, &intent.plan)?;
        projection::Projection::from_replica(&intent.plan, &replica)?
            .verify(&capsule, &intent.plan.budget)?;
        Ok(Self {
            path: capsule.path().into(),
            digest: *expected,
            origin_count: intent.plan.groups.len(),
        })
    }
}
