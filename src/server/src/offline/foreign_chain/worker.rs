//! Service-UID owner. All roots in this batch are real, retained handles. The
//! currently admitted pool is used only for exclusion/authentication, never Add.
use super::*;
use crate::offline::managed_pool::ManagedPool;
use platform_io::fs_safety::{AtomicFile, linux::OpenAt2Root};
use std::path::PathBuf;
use storage_engine::{Config, storage_directories::InspectRequest};
use storage_engine::{
    ForeignImportLimits, ForeignImportPreview, ForeignMigrationPlan, ForeignMigrationStatus,
    ForeignMigrationTransaction, ForeignVersionAvailability,
};
use storage_server::{management::storage_addition::ActiveAddition, storage_directories};

pub(super) fn execute(request: WorkerRequest) -> Result<Output> {
    let target = ManagedPool::open(&request.context)?;
    let identity = target.auth().sole_identity()?;
    let control = ActiveAddition::open(
        target.backend(),
        &identity,
        target.layout().management()?,
        layout::DISKS,
    )?;
    let (configuration, stopped) = control.current_directory_scope()?;
    if !stopped.is_empty() {
        return Err("this foreign material lane requires all historical pool roots enabled; stopped/retired roots were not probed or reused".into());
    }
    let output: Result<Output> = match request.action {
        Action::Preview {
            operation_id,
            source_root,
            source_slot_id,
            source_upstream_slot_id,
            source_metadata_member,
            lower,
        } => {
            let registered_roots = root_paths(&configuration)?;
            let all = registered_roots
                .iter()
                .cloned()
                .chain(std::iter::once(source_root.clone()))
                .chain(lower.iter().map(|item| item.root_path.clone()))
                .collect::<Vec<_>>();
            inventory(&target, &all)?;
            let source = inspect(&target, &source_root)?;
            complete_chain(
                source.source_level(),
                &source_slot_id,
                &source_upstream_slot_id,
                &lower.iter().collect::<Vec<_>>(),
            )?;
            foreign_identity(
                &configuration,
                &source,
                &source_slot_id,
                &lower.iter().collect::<Vec<_>>(),
            )?;
            let mut excluded = vec![
                PathBuf::from(layout::STATE),
                PathBuf::from(layout::MANAGEMENT),
                PathBuf::from(&source_root),
            ];
            excluded.extend(configuration.disks.iter().map(|disk| disk.root.clone()));
            let mut selected = Vec::with_capacity(lower.len());
            for directory in lower {
                let listing = storage_directories::browse(&directory.root_path)?;
                let selection = storage_directories::inspect_within(
                    layout::DISKS,
                    &InspectRequest {
                        mount_path: listing.mount_path,
                        root_path: directory.root_path.clone(),
                    },
                    &excluded,
                )?;
                excluded.push(PathBuf::from(&directory.root_path));
                selected.push(SelectedLower {
                    directory,
                    selection,
                });
            }
            let binding = Binding {
                format: Format::ChainMaterial,
                context: request.context,
                credential: target.auth().credential_revision()?,
                configuration_digest: digest(&configuration)?,
                registered_roots,
                operation_id,
                source: source_record(&source_root, &source),
                source_slot_id,
                source_upstream_slot_id,
                source_metadata_member,
                lower: selected,
            };
            let chain_digest = digest(&binding)?.parse()?;
            let capsule_digests = binding
                .lower
                .iter()
                .map(|item| {
                    Ok(ForeignMigrationPlan::chain_material(
                        &source,
                        operation_id,
                        &chain_digest,
                        item.directory.level,
                    )?
                    .digest()
                    .to_string())
                })
                .collect::<Result<Vec<_>>>()?;
            let plan = Plan {
                binding,
                capsule_digests,
            };
            plan.validate()?;
            original(&target, &configuration, &plan)?;
            Ok(Output::Preview { plan })
        }
        Action::Status { plan } => {
            original(&target, &configuration, &plan)?;
            let (_, report) = status(&target, &plan)?;
            original(&target, &configuration, &plan)?;
            Ok(Output::Report { report })
        }
        Action::Prepare { plan, completed } => {
            original(&target, &configuration, &plan)?;
            let (roots, observed) = status(&target, &plan)?;
            if completed && !observed.all_published() {
                return Err("completed batch lost original material; nothing was recreated".into());
            }
            prepare(&target, &configuration, &plan, roots, observed)?;
            original(&target, &configuration, &plan)?;
            let (_, report) = status(&target, &plan)?;
            Ok(Output::Report { report })
        }
        Action::Identity { plan, expected } => {
            original(&target, &configuration, &plan)?;
            let (_, material) = status(&target, &plan)?;
            if !material.all_published() {
                return Err("target identity mapping requires every original lower capsule".into());
            }
            let observation = original_databases(&target, &plan)?;
            databases::match_expected(&plan, &observation, &expected)?;
            exact_source(&target, &plan)?;
            let proposal = identity::Proposal::capture(&configuration, &plan, &observation)?;
            original(&target, &configuration, &plan)?;
            Ok(Output::Identity { proposal })
        }
        Action::Objects {
            plan,
            databases,
            prepare,
            completed,
        } => {
            let report = object_identities(
                &target,
                &configuration,
                &plan,
                &databases,
                prepare.as_ref(),
                completed,
            )?;
            Ok(Output::Objects { report })
        }
        Action::Databases {
            plan,
            prepare,
            completed,
        } => {
            original(&target, &configuration, &plan)?;
            exact_source(&target, &plan)?;
            let (_, material) = status(&target, &plan)?;
            if !material.all_published() {
                return Err("the entire original lower chain must retain every published capsule before any database work".into());
            }
            let observation = original_databases(&target, &plan)?;
            if completed && !observation.completed() {
                return Err("completed original database batch is incomplete; no missing target or image was recreated".into());
            }
            if let Some(expected) = prepare {
                databases::match_expected(&plan, &observation, &expected)?;
                for (item, digest) in plan.binding.lower.iter().zip(&plan.capsule_digests) {
                    let root = exact_lower(&target, item)?;
                    let owner = ForeignMigrationTransaction::open_published_in(
                        root.into_directory(),
                        &Path::new(&item.directory.root_path).join(CAPSULE),
                        &plan.binding.operation_id,
                        &digest.parse()?,
                    )?;
                    owner.prepare_original_databases()?;
                }
                original(&target, &configuration, &plan)?;
                let report = original_databases(&target, &plan)?;
                databases::match_expected(&plan, &report, &expected)?;
                Ok(Output::Databases { report })
            } else {
                original(&target, &configuration, &plan)?;
                Ok(Output::Databases {
                    report: observation,
                })
            }
        }
    };
    let output = output?;
    target.recheck()?;
    Ok(output)
}

fn root_paths(configuration: &Config) -> Result<Vec<String>> {
    configuration
        .disks
        .iter()
        .map(|disk| {
            disk.root
                .to_str()
                .map(str::to_owned)
                .ok_or_else(|| "registered root is not UTF-8; no path was substituted".into())
        })
        .collect()
}
fn inventory(target: &ManagedPool, paths: &[String]) -> Result<()> {
    layout::names(paths)?;
    target.layout().inventory_all(paths)?;
    let mut identities = BTreeSet::new();
    for path in paths {
        let root = target.layout().disk_root(path)?;
        let id = root.identity();
        if !identities.insert((id.device, id.inode)) {
            return Err("source, existing pool and lower roots must be physically distinct; aliases were preserved".into());
        }
    }
    Ok(())
}
fn inspect(target: &ManagedPool, path: &str) -> Result<ForeignImportPreview> {
    let root = target.layout().disk_root(path)?;
    Ok(ForeignImportPreview::inspect(
        root.into_directory(),
        path.into(),
        ForeignImportLimits::default(),
    )?)
}
fn exact_source(target: &ManagedPool, plan: &Plan) -> Result<ForeignImportPreview> {
    let source = inspect(target, &plan.binding.source.root_path)?;
    if source_record(&plan.binding.source.root_path, &source) != plan.binding.source {
        return Err(
            "foreign source evidence changed; only the original unchanged batch can continue"
                .into(),
        );
    }
    Ok(source)
}
fn source_record(path: &str, source: &ForeignImportPreview) -> Source {
    Source {
        root_path: path.into(),
        physical: Physical {
            device: source.local_binding().device().to_string(),
            inode: source.local_binding().inode().to_string(),
        },
        registration: source.source().registration().clone(),
        level: source.source_level(),
        evidence_digest: source.digest().to_string(),
        histories: source.histories().len(),
        temporarily_unavailable_versions: source
            .histories()
            .iter()
            .map(|history| {
                history
                    .availability()
                    .values()
                    .filter(|value| **value == ForeignVersionAvailability::TemporarilyUnavailable)
                    .count()
            })
            .sum(),
    }
}
fn foreign_identity(
    configuration: &Config,
    source: &ForeignImportPreview,
    source_slot: &str,
    lower: &[&LowerDirectory],
) -> Result<()> {
    if source.histories().is_empty() {
        return Err("foreign complete-chain material requires retained source histories, not an empty source placeholder".into());
    }
    let source_id = source.source().registration();
    if source_id.pool_id == configuration.pool_id
        || configuration.disks.iter().any(|disk| {
            disk.disk_id == source_id.disk_id
                || disk.slot_id.eq_ignore_ascii_case(source_slot)
                || lower
                    .iter()
                    .any(|item| disk.slot_id.eq_ignore_ascii_case(&item.slot_id))
        })
    {
        return Err("this lane requires a foreign pool and distinct new target slots; current/stopped members are not reimported".into());
    }
    Ok(())
}
fn original(target: &ManagedPool, configuration: &Config, plan: &Plan) -> Result<()> {
    plan.validate()?;
    target.recheck()?;
    if target.context() != plan.binding.context
        || target.auth().credential_revision()? != plan.binding.credential
        || digest(configuration)? != plan.binding.configuration_digest
        || root_paths(configuration)? != plan.binding.registered_roots
    {
        return Err("current pool/account/credentials differ from the original complete chain; no fresh identity was substituted".into());
    }
    inventory(target, &plan.roots())?;
    if Physical::from(
        target
            .layout()
            .disk_root(&plan.binding.source.root_path)?
            .identity(),
    ) != plan.binding.source.physical
    {
        return Err("original foreign source directory was replaced".into());
    }
    for item in &plan.binding.lower {
        exact_lower(target, item)?;
    }
    Ok(())
}
fn exact_lower(target: &ManagedPool, item: &SelectedLower) -> Result<OpenAt2Root> {
    let root = target.layout().disk_root(&item.directory.root_path)?;
    let id = root.identity();
    if id.device != item.selection.root_identity.device
        || id.inode != item.selection.root_identity.inode
    {
        return Err(
            "original lower directory identity changed; missing/replaced roots are not recreated"
                .into(),
        );
    }
    Ok(root)
}
fn contents(root: &OpenAt2Root, operation: OperationId) -> Result<()> {
    let stage = format!(".zxs-foreign-{operation}");
    let reservation = format!("{stage}.json");
    let database_reservation = format!(".zxs-foreign-databases-{operation}.json");
    let database_completion = format!(".zxs-foreign-databases-{operation}-complete.json");
    let identity_material = format!(".zxs-foreign-identities-{operation}.json");
    let identity_intent = format!(".zxs-foreign-identities-{operation}-intent.json");
    let identity_completion = format!(".zxs-foreign-identities-{operation}-complete.json");
    for (index, name) in root.entries()?.enumerate() {
        if index >= 64 {
            return Err("lower-root retained material exceeds its entry budget".into());
        }
        let name = name?;
        let text = name
            .as_os_str()
            .to_str()
            .ok_or("non-UTF-8 lower-root material was preserved")?;
        if text == CAPSULE
            || text == stage
            || text == reservation
            || text == "foreign-chain-databases"
            || text == database_reservation
            || text == database_completion
            || text == identity_material
            || text == identity_intent
            || text == identity_completion
        {
            continue;
        }

        if AtomicFile::is_temporary_name(&name) {
            root.open_file(&name.as_relative(), false)?;
        } else {
            return Err(
                "lower root contains material outside the one selected complete-chain operation"
                    .into(),
            );
        }
    }
    Ok(())
}

fn original_databases(target: &ManagedPool, plan: &Plan) -> Result<databases::Observation> {
    let mut targets = Vec::with_capacity(plan.binding.lower.len());
    let mut statuses = Vec::with_capacity(plan.binding.lower.len());
    for (item, digest) in plan.binding.lower.iter().zip(&plan.capsule_digests) {
        let root = exact_lower(target, item)?;
        contents(&root, plan.binding.operation_id)?;
        let owner = ForeignMigrationTransaction::open_published_in(
            root.into_directory(),
            &Path::new(&item.directory.root_path).join(CAPSULE),
            &plan.binding.operation_id,
            &digest.parse()?,
        )?;
        let (report, status) = owner.preview_original_databases()?;
        if report.target_level != item.directory.level
            || report.capsule_digest != *digest
            || report.chain_digest != super::digest(&plan.binding)?
        {
            return Err("original databases do not bind this exact complete-chain target".into());
        }
        targets.push(report);
        statuses.push(status);
    }
    let first = targets.first().ok_or("the complete lower chain is empty")?;
    if targets.iter().any(|report| {
        report.original != first.original
            || report.manifest_hash != first.manifest_hash
            || report.databases != first.databases
            || report.database_bytes != first.database_bytes
            || report.original_disks != first.original_disks
            || report.file_databases != first.file_databases
            || report.directory_databases != first.directory_databases
    }) {
        return Err("required lower targets disagree on the complete original State or database scopes; no peer fills another target".into());
    }
    Ok(databases::Observation { targets, statuses })
}
fn status(target: &ManagedPool, plan: &Plan) -> Result<(Vec<OpenAt2Root>, Report)> {
    let mut roots = Vec::with_capacity(plan.binding.lower.len());
    let mut targets = Vec::with_capacity(plan.binding.lower.len());
    for (item, digest) in plan.binding.lower.iter().zip(&plan.capsule_digests) {
        let root = exact_lower(target, item)?;
        contents(&root, plan.binding.operation_id)?;
        let path = Path::new(&item.directory.root_path).join(CAPSULE);
        let progress = match ForeignMigrationTransaction::status_in(
            root.directory().try_clone()?,
            &path,
            &plan.binding.operation_id,
            &digest.parse()?,
        )? {
            ForeignMigrationStatus::Unstarted => Progress::Unstarted,
            ForeignMigrationStatus::Reserved => Progress::Reserved,
            ForeignMigrationStatus::Published => Progress::PublishedCapsule,
        };
        targets.push(TargetProgress {
            level: item.directory.level,
            path: path
                .to_str()
                .expect("invariant: fixed capsule child of UTF-8 root")
                .into(),
            capsule_digest: digest.clone(),
            progress,
        });
        roots.push(root);
    }
    Ok((roots, Report { targets }))
}
fn prepare(
    target: &ManagedPool,
    configuration: &Config,
    plan: &Plan,
    roots: Vec<OpenAt2Root>,
    observed: Report,
) -> Result<()> {
    // Freeze and check every target before the first lower root can be written.
    // Even a partially prepared original may not omit another required level.
    let source = exact_source(target, plan)?;
    foreign_identity(
        configuration,
        &source,
        &plan.binding.source_slot_id,
        &plan
            .binding
            .lower
            .iter()
            .map(|item| &item.directory)
            .collect::<Vec<_>>(),
    )?;
    let chain_digest = digest(&plan.binding)?.parse()?;
    let mut native_plans = Vec::with_capacity(roots.len());
    for (item, expected) in plan.binding.lower.iter().zip(&plan.capsule_digests) {
        let native = ForeignMigrationPlan::chain_material(
            &source,
            plan.binding.operation_id,
            &chain_digest,
            item.directory.level,
        )?;
        if native.digest().to_string() != *expected {
            return Err("complete-chain native inventory differs from its original digest".into());
        }
        native_plans.push(native);
    }
    for (item, progress) in plan.binding.lower.iter().zip(&observed.targets) {
        if progress.progress == Progress::Unstarted {
            let excluded = plan
                .roots()
                .iter()
                .filter(|path| path.as_str() != item.directory.root_path.as_str())
                .map(PathBuf::from)
                .chain([
                    PathBuf::from(layout::STATE),
                    PathBuf::from(layout::MANAGEMENT),
                ])
                .collect::<Vec<_>>();
            storage_directories::revalidate_within(layout::DISKS, &item.selection, &excluded)?;
        }
    }
    original(target, configuration, plan)?;
    for (((root, native), item), progress) in roots
        .into_iter()
        .zip(native_plans)
        .zip(&plan.binding.lower)
        .zip(&observed.targets)
    {
        // Recheck each still-held parent against the reviewed physical identity.
        let named = exact_lower(target, item)?;
        if named.identity() != root.identity() {
            return Err("held lower root was rebound".into());
        }
        contents(&root, plan.binding.operation_id)?;
        let path = Path::new(&item.directory.root_path).join(CAPSULE);
        let mut transaction = match progress.progress {
            Progress::Unstarted => native.begin_in(root.into_directory(), &path)?,
            Progress::Reserved | Progress::PublishedCapsule => {
                ForeignMigrationTransaction::resume_in(
                    root.into_directory(),
                    &path,
                    &plan.binding.operation_id,
                    native.digest(),
                )?
            }
        };
        if !transaction.is_sealed()? {
            transaction.copy_source(&source)?;
            transaction.seal()?;
        }
        transaction.publish(None)?;
    }
    Ok(())
}

fn object_identities(
    target: &ManagedPool,
    configuration: &Config,
    plan: &Plan,
    expected_databases: &databases::Expected,
    prepare: Option<&storage_engine::ForeignIdentityClosureReport>,
    completed: bool,
) -> Result<objects::Observation> {
    original(target, configuration, plan)?;
    exact_source(target, plan)?;
    let (_, material) = status(target, plan)?;
    if !material.all_published() {
        return Err("identity closure requires every original lower capsule".into());
    }
    let databases = original_databases(target, plan)?;
    databases::match_expected(plan, &databases, expected_databases)?;
    let proposal = identity::Proposal::capture(configuration, plan, &databases)?;
    let actor = target.auth().sole_identity()?;
    let naming = target.backend().capture_foreign_identity_target(
        &actor,
        storage_engine::ForeignIdentityTargetRequest {
            proposed_configuration: proposal.configuration(configuration)?,
            operation_id: plan.binding.operation_id,
            request_digest: digest(plan)?.parse()?,
            chain_digest: digest(&plan.binding)?.parse()?,
            registration_proposal_digest: digest(&proposal)?.parse()?,
        },
    )?;
    let before = observe_object_identities(target, plan, &naming)?;
    if prepare.is_some_and(|expected| expected != &before.expected) {
        return Err("reviewed object identity plan changed; no target material was written".into());
    }
    if completed && !before.completed(plan) {
        return Err(
            "completed object identity batch lost a target; no missing map was recreated".into(),
        );
    }
    original(target, configuration, plan)?;
    target
        .backend()
        .recheck_foreign_identity_target(&actor, &naming)?;
    if prepare.is_none() {
        return Ok(before);
    }
    for (item, digest) in plan.binding.lower.iter().zip(&plan.capsule_digests) {
        let owner = object_owner(target, plan, item, digest)?;
        // The exact administrator and complete current State are checked at
        // each inert write boundary, not inferred from a saved preparation.
        target
            .backend()
            .recheck_foreign_identity_target(&actor, &naming)?;
        if owner.prepare_identity_closure(&naming)? != before.expected {
            return Err("target independently derived a different object identity closure".into());
        }
    }
    let after = observe_object_identities(target, plan, &naming)?;
    if after.expected != before.expected || !after.completed(plan) {
        return Err("entire object identity batch did not complete independently".into());
    }
    original(target, configuration, plan)?;
    exact_source(target, plan)?;
    target
        .backend()
        .recheck_foreign_identity_target(&actor, &naming)?;
    Ok(after)
}
fn object_owner(
    target: &ManagedPool,
    plan: &Plan,
    item: &SelectedLower,
    digest: &str,
) -> Result<ForeignMigrationTransaction> {
    let root = exact_lower(target, item)?;
    contents(&root, plan.binding.operation_id)?;
    Ok(ForeignMigrationTransaction::open_published_in(
        root.into_directory(),
        &Path::new(&item.directory.root_path).join(CAPSULE),
        &plan.binding.operation_id,
        &digest.parse()?,
    )?)
}
fn observe_object_identities(
    target: &ManagedPool,
    plan: &Plan,
    naming: &storage_engine::ForeignIdentityTarget,
) -> Result<objects::Observation> {
    use storage_engine::ForeignIdentityClosureStatus;
    let mut expected = None;
    let mut statuses = Vec::with_capacity(plan.binding.lower.len());
    for (item, digest) in plan.binding.lower.iter().zip(&plan.capsule_digests) {
        let (report, status) =
            object_owner(target, plan, item, digest)?.preview_identity_closure(naming)?;
        if expected.as_ref().is_some_and(|saved| saved != &report) {
            return Err(
                "required lower targets disagree on their complete object identity closure".into(),
            );
        }
        let progress = match status {
            ForeignIdentityClosureStatus::NotStarted => objects::Progress::NotStarted,
            ForeignIdentityClosureStatus::Preparing => objects::Progress::Preparing,
            ForeignIdentityClosureStatus::Verified { report: verified } => {
                if *verified != report {
                    return Err(
                        "local identity completion differs from derived native State".into(),
                    );
                }
                objects::Progress::Verified
            }
        };
        expected = Some(report);
        statuses.push(progress);
    }
    Ok(objects::Observation {
        expected: expected.ok_or("object identity closure has no real lower target")?,
        statuses,
    })
}
