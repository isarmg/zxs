//! Install actual ordinary immutable objects before selecting any runtime
//! generation. All original native votes and all published images are required;
//! no Config, seal, generation marker, journal or writer is selected here.
use super::*;
use crate::engine::membership_transition::{
    AdmittedBackupDecision, BackupFirstPublicationHash, PreparedBackupFirstPublication,
};
use crate::journal::{PreparedMetadataObject, Publication};

mod store;
use store::{ObjectSet, require_target_selection, select_target};

const INTENT: &str = "backup-preparation/native-metadata.json";
const COMPLETE: &str = "backup-native-metadata-installed.json";
const MAX_DESCRIPTOR: usize = 4 * 1024 * 1024;
const MAX_OBJECTS: usize = 12_292;
const MAX_BYTES: usize = 256 * 1024 * 1024;

/// Fresh ordinary-object readback on every original and selected disk, plus
/// target Current replay using those actual objects. Not active Engine Current,
/// a generation locator, a target writer or permission to release any fence.
#[derive(Clone, Debug)]
pub struct BackupNativeMetadataReport {
    pub system: BackupSystemDatabaseReport,
    pub installation_hash: String,
    pub authority_hash: String,
    pub candidate_hash: String,
    pub registry: DiskRegistryCommitment,
    pub objects_per_disk: usize,
    pub bytes_per_disk: u64,
    pub original_disks: usize,
    pub selected_targets: usize,
}

#[derive(Serialize)]
struct ObjectReference<'a> {
    kind: &'static str,
    hash: &'a str,
    bytes: usize,
}
#[derive(Serialize)]
struct Manifest<'a> {
    format: &'static str,
    source_fence_hash: &'a str,
    decision_hash: &'a str,
    publication_hash: &'a str,
    system_installation_hash: &'a str,
    config_hash: &'a str,
    registry: &'a DiskRegistryCommitment,
    original_disks: &'a [DiskRegistrationIdentity],
    targets: &'a [BackupPreparedTopologyNode],
    objects: Vec<ObjectReference<'a>>,
    seal_hash: String,
}
struct Plan {
    resolved: configuration::ResolvedConfiguration,
    publication: PreparedBackupFirstPublication,
    finalize: PreparedMetadataObject,
    manifest: Vec<u8>,
    report: BackupNativeMetadataReport,
}
struct NativeReadback {
    published: crate::history_package::AdmittedState,
    registry: crate::VerifiedDiskRegistry,
    publication: PreparedBackupFirstPublication,
}

fn budget() -> Error {
    Error::new(
        ErrorCode::ResourceLimit,
        "Backup ordinary metadata exceeds 12292 objects, 256 MiB or its descriptor/read budget",
    )
}

fn object_set<'a>(
    resolved: &'a configuration::ResolvedConfiguration,
    publication: &'a PreparedBackupFirstPublication,
    finalize: &'a PreparedMetadataObject,
) -> Result<ObjectSet<'a>> {
    ObjectSet::new(
        resolved
            .candidate
            .native_objects()
            .chain(resolved.registry.objects())
            .chain([
                ("publication", publication.report().publication_hash.as_str(), publication.publication_bytes()),
                ("finalize", finalize.hash.as_str(), finalize.bytes()),
            ]),
        resolved.candidate.hash(),
        publication.seal_bytes(),
    )
}

impl Plan {
    fn capture(
        context: &common_databases::Context<'_>,
        prepared: &PreparedBackupRegistry,
        decision: &AdmittedBackupDecision,
        expected: &BackupFirstPublicationHash,
        stop: &AtomicBool,
    ) -> Result<Self> {
        // This is the complete all-participant gate, not the SYSTEM progress
        // record. Ordinary SQLite remains closed for the source Engine.
        let system = system_database::observe(context, prepared, decision, expected, stop)?;
        let resolved = configuration::resolve(prepared, decision, expected, stop)?;
        let publication = context
            .choices
            .read_first_publication(prepared, &resolved.candidate, stop)?;
        if publication.report().publication_hash != expected.as_str() {
            return Err(invalid("ordinary metadata lost the original actual Publication"));
        }
        let native: Publication = crate::journal::decode_proof(publication.publication_bytes())?;
        let finalize = PreparedMetadataObject::new("finalize", &native.finalize, MAX_METADATA)?;
        let objects = object_set(&resolved, &publication, &finalize)?;
        let targets = prepared.projection.selected_nodes();
        let decision_hash = decision.hash();
        let manifest = canonical_json(&Manifest {
            format: "zxs-backup-ordinary-native-metadata",
            source_fence_hash: prepared.source_fence_hash(),
            decision_hash: decision_hash.as_str(),
            publication_hash: expected.as_str(),
            system_installation_hash: &system.installation_hash,
            config_hash: &resolved.report.config_hash,
            registry: resolved.registry.commitment(),
            original_disks: &system.roots.original_disks,
            targets: &targets,
            objects: objects.references(),
            seal_hash: blake3::hash(publication.seal_bytes()).to_hex().to_string(),
        })?;
        if manifest.len() > MAX_DESCRIPTOR {
            return Err(budget());
        }
        let report = BackupNativeMetadataReport {
            original_disks: system.roots.original_disks.len(),
            selected_targets: prepared.targets.len(),
            authority_hash: resolved.candidate.authority_hash().into(),
            candidate_hash: resolved.candidate.hash().into(),
            registry: resolved.registry.commitment().clone(),
            objects_per_disk: objects.len(),
            bytes_per_disk: objects.bytes() as u64,
            installation_hash: blake3::hash(&manifest).to_hex().to_string(),
            system,
        };
        drop(objects);
        Ok(Self { resolved, publication, finalize, manifest, report })
    }

    fn objects(&self) -> Result<ObjectSet<'_>> {
        object_set(&self.resolved, &self.publication, &self.finalize)
    }

    fn original_disk(&self, identity: &DiskRegistrationIdentity) -> Result<&DiskConfig> {
        self.resolved.config.disks.iter().find(|disk| {
            disk.disk_id == identity.disk_id
                && disk.slot_id == identity.slot_id
                && disk.binding_generation == identity.binding_generation
                && self.resolved.config.pool_id == identity.pool_id
        }).ok_or_else(|| invalid("ordinary metadata root is outside the exact published Config"))
    }

    fn target_disk(&self, prepared: &PreparedBackupRegistry, index: usize) -> Result<&DiskConfig> {
        self.original_disk(&prepared.projection.targets[index].node.identity)
    }

    fn require_record(&self, root: &DiskRoot, name: &str) -> Result<()> {
        if root.read_bytes(&root.path().join(name), MAX_DESCRIPTOR)? != self.manifest {
            return Err(invalid("ordinary metadata record differs from the original complete decision"));
        }
        Ok(())
    }

    fn verify_root(
        &self,
        prepared: &PreparedBackupRegistry,
        root: &DiskRoot,
        disk: &DiskConfig,
        stop: &AtomicBool,
    ) -> Result<NativeReadback> {
        let objects = self.objects()?;
        objects.verify(root, disk, stop)?;
        let published = self.resolved.candidate.read_state_at(
            &root.private_child(&root.path().join("metadata"))?, stop,
        )?;
        let mut count = 0usize;
        let mut bytes = 0usize;
        let registry = self.resolved.registry.verify_backup_with(|kind, hash| {
            preparation_checkpoint(stop)?;
            let expected = objects.get(kind, hash)?;
            count += 1;
            bytes = bytes.checked_add(expected.len()).ok_or_else(budget)?;
            if count > MAX_OBJECTS * 2 || bytes > MAX_BYTES * 2 {
                return Err(budget());
            }
            let actual = crate::journal::load_authenticated_object_at(root, disk, kind, hash)?;
            if actual.bytes() != expected {
                return Err(invalid("ordinary native registry page changed during readback"));
            }
            Ok(actual.bytes().to_vec())
        })?;
        registry.require_config(&self.resolved.config)?;
        registry.require_backup_namespace(&self.resolved.candidate, &published)?;
        let publication = crate::journal::load_authenticated_object_at(
            root, disk, "publication", &self.publication.report().publication_hash,
        )?;
        let seal = root.read_bytes(
            &crate::journal::publication_seal_path(disk, self.resolved.candidate.hash()), MAX_METADATA,
        )?;
        let actual = PreparedBackupFirstPublication::from_trusted_bytes(
            prepared,
            &self.resolved.candidate,
            &BackupFirstPublicationHash::parse(&self.publication.report().publication_hash)?,
            publication.bytes(),
            &seal,
        )?;
        Ok(NativeReadback { published, registry, publication: actual })
    }

    fn verify_all(
        &self,
        context: &common_databases::Context<'_>,
        prepared: &PreparedBackupRegistry,
        stop: &AtomicBool,
    ) -> Result<()> {
        self.require_record(context.system, INTENT)?;
        context.sources.for_each_disk(stop, |root, identity| {
            self.verify_root(prepared, root, self.original_disk(identity)?, stop)?;
            Ok(())
        }, &self.resolved.config.pool_id)?;
        for (index, target) in prepared.targets.iter().enumerate() {
            preparation_checkpoint(stop)?;
            require_target_selection(&target.root, &self.report.installation_hash)?;
            self.objects()?.inventory_target(&target.root, stop)?;
            let native = self.verify_root(
                prepared, &target.root, self.target_disk(prepared, index)?, stop,
            )?;
            // Unlike the earlier isolated reader, the State, registry and
            // Publication below came from this target's ordinary native store.
            // Full archived namespace replay still preserves D/undo/retirement
            // and recursive policy fences. The resulting view stays read-only.
            published_registry::with_archive_namespaces(
                prepared,
                index,
                published_registry::NamespaceInputs {
                    candidate: &self.resolved.candidate,
                    registry: &native.registry,
                    published: &native.published,
                    publication: &native.publication,
                },
                stop,
                |_| Ok(()),
            )?;
        }
        context.sources.recheck(context.system)?;
        prepared.recheck_names()
    }

    fn require_completions(
        &self,
        context: &common_databases::Context<'_>,
        prepared: &PreparedBackupRegistry,
        stop: &AtomicBool,
    ) -> Result<()> {
        self.require_record(context.system, COMPLETE)?;
        context.sources.for_each_disk(stop, |root, _| self.require_record(root, COMPLETE), &self.resolved.config.pool_id)?;
        for target in &prepared.targets {
            self.require_record(&target.root, COMPLETE)?;
        }
        Ok(())
    }
}

fn has_completion(
    context: &common_databases::Context<'_>,
    prepared: &PreparedBackupRegistry,
    plan: &Plan,
    stop: &AtomicBool,
) -> Result<bool> {
    let mut complete = false;
    let mut inspect = |root: &DiskRoot| -> Result<()> {
        if preparation_path_exists(root, &root.path().join(COMPLETE))? {
            plan.require_record(root, COMPLETE)?;
            complete = true;
        }
        Ok(())
    };
    inspect(context.system)?;
    context.sources.for_each_disk(stop, |root, _| inspect(root), &plan.resolved.config.pool_id)?;
    for target in &prepared.targets {
        inspect(&target.root)?;
    }
    Ok(complete)
}

pub(super) fn install(
    context: &common_databases::Context<'_>,
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    expected: &BackupFirstPublicationHash,
    stop: &AtomicBool,
) -> Result<BackupNativeMetadataReport> {
    let plan = Plan::capture(context, prepared, decision, expected, stop)?;
    if has_completion(context, prepared, &plan, stop)? {
        // Any completion follows the ALL-root verification barrier. Missing
        // native bytes or a target selection are loss, not preparation work.
        plan.verify_all(context, prepared, stop)?;
    } else {
        let system_required = (plan.manifest.len() as u64)
            .checked_mul(2)
            .and_then(|n| n.checked_add(plan.resolved.config.metadata_reserve_bytes))
            .and_then(|n| n.checked_add(1024 * 1024))
            .ok_or_else(budget)?;
        if context.system.available()? < system_required {
            return Err(Error::new(ErrorCode::NoSpace,
                "SYSTEM cannot retain the complete native metadata decision and completion records"));
        }
        if preparation_path_exists(context.system, &context.system.path().join(INTENT))? {
            plan.require_record(context.system, INTENT)?;
        }
        let objects = plan.objects()?;
        context.sources.for_each_disk(stop, |root, identity| {
            objects.preflight(root, plan.original_disk(identity)?, plan.resolved.config.metadata_reserve_bytes, stop)
        }, &plan.resolved.config.pool_id)?;
        for (index, target) in prepared.targets.iter().enumerate() {
            store::preflight_target(&target.root, &plan.report.installation_hash)?;
            objects.inventory_target(&target.root, stop)?;
            objects.preflight(&target.root, plan.target_disk(prepared, index)?, plan.resolved.config.metadata_reserve_bytes, stop)?;
        }
        install_preparation_bytes(context.system, &context.system.path().join(INTENT), &plan.manifest)?;
        // Every target carries its exact no-activation boundary BEFORE the
        // first ordinary metadata directory can appear. It never chooses State.
        for target in &prepared.targets {
            select_target(&target.root, &plan.report.installation_hash)?;
        }
        context.sources.recheck(context.system)?;
        context.sources.for_each_disk(stop, |root, identity| {
            objects.install(root, plan.original_disk(identity)?, stop)
        }, &plan.resolved.config.pool_id)?;
        for (index, target) in prepared.targets.iter().enumerate() {
            preparation_checkpoint(stop)?;
            prepared.recheck_names()?;
            require_target_selection(&target.root, &plan.report.installation_hash)?;
            objects.install(&target.root, plan.target_disk(prepared, index)?, stop)?;
        }
        plan.verify_all(context, prepared, stop)?;
    }
    // Re-observe actual journals, databases and every target body immediately
    // before recording completion. A changed full binding cannot be relabeled.
    let expected_manifest = plan.manifest.clone();
    drop(plan);
    let after = Plan::capture(context, prepared, decision, expected, stop)?;
    if after.manifest != expected_manifest {
        return Err(invalid("ordinary metadata proof changed before completion"));
    }
    after.verify_all(context, prepared, stop)?;
    context.sources.for_each_disk(stop, |root, _| {
        install_preparation_bytes(root, &root.path().join(COMPLETE), &after.manifest)
    }, &after.resolved.config.pool_id)?;
    for target in &prepared.targets {
        preparation_checkpoint(stop)?;
        install_preparation_bytes(&target.root, &target.root.path().join(COMPLETE), &after.manifest)?;
    }
    install_preparation_bytes(context.system, &context.system.path().join(COMPLETE), &after.manifest)?;
    after.require_completions(context, prepared, stop)?;
    Ok(after.report)
}

pub(super) fn observe(
    context: &common_databases::Context<'_>,
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    expected: &BackupFirstPublicationHash,
    stop: &AtomicBool,
) -> Result<BackupNativeMetadataReport> {
    let plan = Plan::capture(context, prepared, decision, expected, stop)?;
    plan.verify_all(context, prepared, stop)?;
    plan.require_completions(context, prepared, stop)?;
    context.choices.observe_first_publication(prepared, &plan.resolved.candidate, stop)?;
    Ok(plan.report)
}

pub(super) use store::require_unselected_target;
