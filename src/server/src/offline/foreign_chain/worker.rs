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
    for (index, name) in root.entries()?.enumerate() {
        if index >= 64 {
            return Err("lower-root retained material exceeds its entry budget".into());
        }
        let name = name?;
        let text = name
            .as_os_str()
            .to_str()
            .ok_or("non-UTF-8 lower-root material was preserved")?;
        if text == CAPSULE || text == stage || text == reservation {
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
    let source = inspect(target, &plan.binding.source.root_path)?;
    if source_record(&plan.binding.source.root_path, &source) != plan.binding.source {
        return Err(
            "foreign source evidence changed; only this original unchanged batch can continue"
                .into(),
        );
    }
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
