use super::*;

impl ForeignMigrationPlan {
    /// Plan a verified level 1/2/3 source as an offline primary capsule with
    /// unchanged original graphs, without admitting or changing the source role.
    pub fn primary(source: &ForeignImportPreview, operation_id: OperationId) -> Result<Self> {
        Self::build(source, operation_id, Placement::Primary, None)
    }

    /// Plan a level 2 reconstruction under one existing empty logical directory.
    /// Every original primary gets one distinct child, including all its histories.
    pub fn rebuild(
        source: &ForeignImportPreview,
        operation_id: OperationId,
        engine: &Engine,
        slot: &str,
        directory: &PortablePath,
        names: &BTreeMap<ForeignPrimaryOrigin, String>,
    ) -> Result<Self> {
        if source.source_level != 2 {
            return Err(invalid(
                "only a secondary source supports per-origin reconstruction",
            ));
        }
        let mapping =
            source.map_to_primary(engine, slot, Some(directory), &history_names(source))?;
        let placement = Placement::Rebuild {
            primary: AcceptedPrimary {
                registration: mapping.target,
                sequence: mapping.target_sequence,
            },
            directory: directory.as_str().to_owned(),
        };
        Self::build(source, operation_id, placement, Some(names))
    }

    /// A level 3 source may become level 2 only in a plan bound to a currently
    /// accepted, enabled level 1 registration. No orphan secondary is produced.
    pub fn secondary(
        source: &ForeignImportPreview,
        operation_id: OperationId,
        engine: &Engine,
        primary_slot: &str,
    ) -> Result<Self> {
        if source.source_level != 3 {
            return Err(invalid("only a tertiary source may become a secondary"));
        }
        let primary = AcceptedPrimary::read(engine, primary_slot)?;
        Self::build(source, operation_id, Placement::Secondary { primary }, None)
    }

    fn build(
        source: &ForeignImportPreview,
        operation_id: OperationId,
        placement: Placement,
        names: Option<&BTreeMap<ForeignPrimaryOrigin, String>>,
    ) -> Result<Self> {
        if !(1..=3).contains(&source.source_level) {
            return Err(invalid("unsupported source role"));
        }
        // The existing cheap revalidation does not cover bodies and all graph
        // pages. Re-inspect before freezing a raw inventory, then verify again
        // from the copy before its sealed marker may be written.
        let refreshed = ForeignImportPreview::inspect(
            source.root.directory(source.root.path())?,
            source.root.path().into(),
            source.limits,
        )?;
        if refreshed.digest != source.digest {
            return Err(changed("source changed after migration review"));
        }
        let budget = InspectionBudget {
            metadata_bytes: source.limits.metadata_bytes,
            journal_bytes: source.limits.journal_bytes,
            versions: source.limits.versions,
            payload_bytes: source.limits.payload_bytes,
        };
        let record = PlanRecord {
            format: FORMAT.into(),
            operation_id,
            source_digest: source.digest.to_string(),
            source_registration: source.source.registration.clone(),
            source_level: source.source_level,
            source_path: source.root.path().into(),
            source_local: PhysicalBinding::of(&source.root)?,
            placement,
            groups: groups(source, names)?,
            inventory: Inventory::capture(&source.root, &budget)?,
            budget,
        };
        record.validate()?;
        source.revalidate_source()?;
        if canonical_json(&record)?.len() > MAX_INTENT / 2 {
            return Err(Error::new(
                ErrorCode::ResourceLimit,
                "migration plan exceeds durable intent budget",
            ));
        }
        let digest = digest(&record)?;
        Ok(Self { record, digest })
    }
}

fn groups(
    source: &ForeignImportPreview,
    names: Option<&BTreeMap<ForeignPrimaryOrigin, String>>,
) -> Result<Vec<OriginGroup>> {
    let mut origins = BTreeMap::<ForeignPrimaryOrigin, BTreeMap<String, String>>::new();
    for history in &source.histories {
        origins.entry(history.origin.clone()).or_default().insert(
            history.manifest.history.history_tree_id.clone(),
            history.manifest_hash.clone(),
        );
    }
    if let Some(names) = names
        && (names.len() != origins.len()
            || origins.keys().any(|origin| !names.contains_key(origin)))
    {
        return Err(invalid(
            "origin mapping must cover exactly all original primary identities",
        ));
    }
    let mut seen = BTreeSet::new();
    let mut groups = Vec::new();
    for (origin, histories) in origins {
        let provenance = Origin::from(&origin);
        let directory = match names {
            Some(names) => names[&origin].clone(),
            None => object_hash("foreign-primary-origin", &provenance)?,
        };
        name_policy::validate_name(&directory)?;
        if !seen.insert(name_policy::lookup_key(&directory)) {
            return Err(Error::new(
                ErrorCode::NameCollision,
                "original primaries map to colliding directory names",
            ));
        }
        groups.push(OriginGroup {
            origin: provenance,
            directory,
            histories,
        });
    }
    Ok(groups)
}

fn history_names(source: &ForeignImportPreview) -> BTreeMap<String, String> {
    source
        .histories
        .iter()
        .map(|history| {
            (
                history.group_id.clone(),
                history.manifest.history.history_tree_id.clone(),
            )
        })
        .collect()
}

impl AcceptedPrimary {
    fn read(engine: &Engine, slot: &str) -> Result<Self> {
        engine.require_write()?;
        let registered = engine
            .registered_disks()
            .find(|disk| disk.slot_id.eq_ignore_ascii_case(slot))
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::DiskNotFound,
                    "migration upstream is not accepted",
                )
            })?;
        if registered.registration_mode != RegistrationMode::Enabled
            || registered.level != 1
            || registered.upstream_slot_id.is_some()
        {
            return Err(invalid(
                "migration upstream must be an accepted enabled primary",
            ));
        }
        let disk = engine.disk(registered.slot_id)?;
        let held = engine.disk_resources.get(disk)?;
        DiskRoot::from_directory(
            held.root.path().into(),
            held.root.directory(held.root.path())?,
        )?;
        if let Some(namespaces) = engine.admitted_current_namespaces()? {
            namespaces.require_write_slot(registered.slot_id)?;
        }
        Ok(Self {
            registration: registered.identity(),
            sequence: engine.state.sequence,
        })
    }
}

impl PlanRecord {
    pub(super) fn validate(&self) -> Result<()> {
        if self.format != FORMAT {
            return Err(Error::new(
                ErrorCode::FormatUnsupported,
                "unsupported foreign capsule format",
            ));
        }
        self.budget.limits()?;
        self.inventory.validate(&self.budget)?;
        self.source_digest
            .parse::<ContentHash>()
            .map_err(|error| Error::from(std::io::Error::other(error)))?;
        if !self.source_path.is_absolute()
            || self
                .source_path
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        {
            return Err(invalid("invalid original source path"));
        }
        match &self.placement {
            Placement::Primary if (1..=3).contains(&self.source_level) => (),
            Placement::Rebuild { directory, .. } if self.source_level == 2 => {
                PortablePath::parse(directory)?;
                for group in &self.groups {
                    name_policy::validate_name(&group.directory)?;
                    PortablePath::parse(&format!("{directory}/{}", group.directory))?;
                    for history in group.histories.keys() {
                        PortablePath::parse(&format!("{directory}/{}/{history}", group.directory))?;
                    }
                }
            }
            Placement::Secondary { .. } if self.source_level == 3 => (),
            _ => return Err(invalid("unsupported foreign migration role transition")),
        }
        Ok(())
    }

    pub(super) fn check_target(
        &self,
        replica: &ForeignImportPreview,
        engine: Option<&Engine>,
    ) -> Result<()> {
        let primary = match &self.placement {
            Placement::Primary => return Ok(()),
            Placement::Rebuild { primary, .. } | Placement::Secondary { primary } => primary,
        };
        let engine =
            engine.ok_or_else(|| invalid("migration requires its accepted primary engine"))?;
        if AcceptedPrimary::read(engine, &primary.registration.slot_id)? != *primary {
            return Err(Error::new(
                ErrorCode::PlanStale,
                "accepted primary registration or checkpoint changed",
            ));
        }
        if let Placement::Rebuild { directory, .. } = &self.placement {
            replica.map_to_primary(
                engine,
                &primary.registration.slot_id,
                Some(&PortablePath::parse(directory)?),
                &history_names(replica),
            )?;
        }
        Ok(())
    }

    pub(super) fn check_replica(&self, replica: &ForeignImportPreview) -> Result<()> {
        if replica.digest.to_string() != self.source_digest
            || replica.source.registration != self.source_registration
            || replica.source_level != self.source_level
        {
            return Err(changed(
                "copied source does not match the reviewed checkpoint",
            ));
        }
        let names = self
            .groups
            .iter()
            .map(|group| {
                (
                    ForeignPrimaryOrigin {
                        pool_id: group.origin.pool_id.clone(),
                        disk_id: group.origin.disk_id.clone(),
                        slot_id: group.origin.slot_id.clone(),
                        binding_generation: group.origin.binding_generation,
                    },
                    group.directory.clone(),
                )
            })
            .collect();
        if groups(replica, Some(&names))? != self.groups {
            return Err(invalid(
                "copied history provenance differs from origin mapping",
            ));
        }
        Ok(())
    }
}
