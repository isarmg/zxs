//! Complete published databases at SYSTEM and every original root. These are
//! immutable generation-local copies, not active aliases or a Current permit.
//! This stage alone leaves old aliases alone. Later ordinary-alias installation
//! explicitly suspends old-State consumers before replacing any alias.
use super::*;
use crate::disk::encode_metadata;
use crate::engine::backup_preparation::native_control::databases::materials::{
    self, FileReference, Namespace, PreparedFiles,
};
use crate::engine::membership_transition::{AdmittedBackupDecision, BackupFirstPublicationHash};
use published_scopes::projection::Scopes;

mod sources;
pub(super) use sources::Sources;

const INTENT: &str = "backup-preparation/common-databases.json";
const COMPLETE: &str = "common-databases-installed.json";
const MAX_DESCRIPTOR: usize = 64 * 1024;

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub struct BackupOriginalDatabaseReport {
    pub identity: DiskRegistrationIdentity,
    #[serialization(with = "storage_contract::u64_string")]
    pub bytes: u64,
    pub current_hash: String,
    pub history_hash: String,
}
#[derive(Clone, Debug)]
pub struct BackupCommonDatabasesReport {
    pub published: BackupPublishedScopesReport,
    pub common_manifest_hash: String,
    pub system_database_bytes: u64,
    pub system_database_hash: String,
    pub original_disks: Vec<BackupOriginalDatabaseReport>,
}
#[derive(Serialize)]
struct OriginalBinding {
    identity: DiskRegistrationIdentity,
    physical: PhysicalBinding,
    seal_hash: String,
}
#[derive(Serialize)]
struct CommonManifest<'a> {
    format: &'static str,
    source_fence_hash: &'a str,
    decision_hash: &'a str,
    publication_hash: &'a str,
    snapshot_hash: &'a str,
    registry: &'a DiskRegistryCommitment,
    system: &'a PhysicalBinding,
    system_writer: &'a PhysicalBinding,
    system_database_hash: String,
    original_bindings: Vec<OriginalBinding>,
    original_databases: &'a [BackupOriginalDatabaseReport],
    targets: &'a [BackupPublishedScopeTargetReport],
}
#[derive(Serialize)]
#[serialization(tag = "kind", rename_all = "kebab-case")]
enum LocalScope<'a> {
    System {
        pool_id: &'a str,
    },
    OriginalDisk {
        identity: &'a DiskRegistrationIdentity,
    },
}
#[derive(Serialize)]
struct LocalManifest<'a> {
    format: &'static str,
    common_manifest_hash: &'a str,
    scope: LocalScope<'a>,
    files: BTreeMap<&'a str, FileReference>,
}

pub(super) struct Context<'a> {
    pub(super) system: &'a DiskRoot,
    pub(super) sources: &'a Sources,
    pub(super) choices: &'a BackupChoiceContext,
}
struct Plan {
    registry: published_registry::Plan,
    scopes: Scopes,
    common: Vec<u8>,
    report: BackupCommonDatabasesReport,
}
impl Plan {
    fn capture(
        context: &Context<'_>,
        prepared: &PreparedBackupRegistry,
        decision: &AdmittedBackupDecision,
        expected: &BackupFirstPublicationHash,
        stop: &AtomicBool,
    ) -> Result<Self> {
        context.sources.require_task(prepared)?;
        context.sources.recheck(context.system)?;
        let published = published_scopes::observe(prepared, decision, expected, stop)?;
        let registry = published_registry::Plan::capture(prepared, decision, expected, stop)?;
        context
            .choices
            .observe_first_publication(prepared, registry.candidate(), stop)?;
        // All targets have been independently verified above. The fixed first
        // target supplies the common source projection; there is no peer retry
        // or source-SQLite fallback if this particular native input is lost.
        let scopes = Scopes::capture(prepared, &registry, 0, stop)?;
        let pool_id = &prepared.task.selection().first().pool_id;
        let bindings = context.sources.bindings(pool_id);
        let originals = bindings
            .iter()
            .map(|binding| {
                let [(_, current), (_, history)] = scopes.disk(&binding.identity)?;
                Ok(BackupOriginalDatabaseReport {
                    identity: binding.identity.clone(),
                    bytes: (current.len() + history.len()) as u64,
                    current_hash: blake3::hash(current).to_hex().to_string(),
                    history_hash: blake3::hash(history).to_hex().to_string(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let configuration = registry.configuration();
        let publication = &configuration.publication.publication;
        let common = encode_metadata(
            &CommonManifest {
                format: "zxs-backup-common-published-databases",
                source_fence_hash: prepared.source_fence_hash(),
                decision_hash: publication.decision_hash.as_str(),
                publication_hash: &publication.publication_hash,
                snapshot_hash: &publication.snapshot_hash,
                registry: &configuration.registry,
                system: context.sources.system_binding(),
                system_writer: context.sources.system_writer_binding(),
                system_database_hash: blake3::hash(scopes.system()).to_hex().to_string(),
                original_bindings: bindings,
                original_databases: &originals,
                targets: &published.targets,
            },
            MAX_DESCRIPTOR,
        )?;
        let report = BackupCommonDatabasesReport {
            published,
            common_manifest_hash: blake3::hash(&common).to_hex().to_string(),
            system_database_bytes: scopes.system().len() as u64,
            system_database_hash: blake3::hash(scopes.system()).to_hex().to_string(),
            original_disks: originals,
        };
        Ok(Self {
            registry,
            scopes,
            common,
            report,
        })
    }
    fn files<'a>(
        &'a self,
        scope: LocalScope<'_>,
        files: impl IntoIterator<Item = (&'a str, &'a [u8])>,
    ) -> Result<PreparedFiles<'a>> {
        let files: BTreeMap<_, _> = files.into_iter().collect();
        let manifest = encode_metadata(
            &LocalManifest {
                format: "zxs-backup-local-published-databases",
                common_manifest_hash: &self.report.common_manifest_hash,
                scope,
                files: files
                    .iter()
                    .map(|(name, bytes)| (*name, FileReference::from_bytes(bytes)))
                    .collect(),
            },
            MAX_DESCRIPTOR,
        )?;
        PreparedFiles::new(
            Namespace::CommonDatabases,
            manifest,
            files
                .into_iter()
                .map(|(name, bytes)| (PathBuf::from(name), bytes)),
        )
    }
    fn system_files(&self, prepared: &PreparedBackupRegistry) -> Result<PreparedFiles<'_>> {
        self.files(
            LocalScope::System {
                pool_id: &prepared.task.selection().first().pool_id,
            },
            [("index.sqlite", self.scopes.system())],
        )
    }
    fn disk_files(&self, identity: &DiskRegistrationIdentity) -> Result<PreparedFiles<'_>> {
        self.files(
            LocalScope::OriginalDisk { identity },
            self.scopes.disk(identity)?,
        )
    }
    fn record<'a>(&'a self, name: &'a str) -> materials::RootRecord<'a> {
        materials::RootRecord {
            name,
            bytes: self.common.as_slice().into(),
            maximum: MAX_DESCRIPTOR,
        }
    }
    fn visit_roots(
        &self,
        context: &Context<'_>,
        prepared: &PreparedBackupRegistry,
        stop: &AtomicBool,
        mut action: impl FnMut(&DiskRoot) -> Result<()>,
    ) -> Result<()> {
        context.sources.recheck(context.system)?;
        action(context.system)?;
        context.sources.for_each(
            stop,
            |root, _| action(root),
            &prepared.task.selection().first().pool_id,
        )?;
        for target in &prepared.targets {
            preparation_checkpoint(stop)?;
            action(&target.root)?;
        }
        context.sources.recheck(context.system)?;
        prepared.recheck_names()
    }
    fn verify_data(
        &self,
        context: &Context<'_>,
        prepared: &PreparedBackupRegistry,
        decision: &AdmittedBackupDecision,
        expected: &BackupFirstPublicationHash,
        stop: &AtomicBool,
    ) -> Result<()> {
        context.sources.recheck(context.system)?;
        materials::verify(context.system, &self.system_files(prepared)?, stop)?;
        context.sources.for_each(
            stop,
            |root, identity| materials::verify(root, &self.disk_files(identity)?, stop),
            &prepared.task.selection().first().pool_id,
        )?;
        published_scopes::observe(prepared, decision, expected, stop)?;
        context
            .choices
            .observe_first_publication(prepared, self.registry.candidate(), stop)?;
        context.sources.recheck(context.system)?;
        prepared.recheck_names()
    }
    fn preflight(
        &self,
        context: &Context<'_>,
        prepared: &PreparedBackupRegistry,
        decision: &AdmittedBackupDecision,
        expected: &BackupFirstPublicationHash,
        stop: &AtomicBool,
    ) -> Result<()> {
        let mut complete = false;
        self.visit_roots(context, prepared, stop, |root| {
            if preparation_path_exists(root, &root.path().join(COMPLETE))? {
                self.record(COMPLETE).require(root)?;
                complete = true;
            }
            Ok(())
        })?;
        let intent = context.system.path().join(INTENT);
        if complete || preparation_path_exists(context.system, &intent)? {
            self.record(INTENT).require(context.system)?;
        }
        // Any completed participant proves that ALL data was already read back.
        // Missing data is then damage, not permission to downgrade and refill it.
        if complete {
            self.verify_data(context, prepared, decision, expected, stop)?;
        }
        materials::require_capacity(context.system, &self.system_files(prepared)?, stop)?;
        context.sources.for_each(
            stop,
            |root, identity| materials::require_capacity(root, &self.disk_files(identity)?, stop),
            &prepared.task.selection().first().pool_id,
        )?;
        for target in &prepared.targets {
            if target.root.available()? < 16 * 1024 * 1024 + self.common.len() as u64 {
                return Err(Error::new(
                    ErrorCode::NoSpace,
                    "target lacks common completion space",
                ));
            }
        }
        Ok(())
    }
    fn observe(
        &self,
        context: &Context<'_>,
        prepared: &PreparedBackupRegistry,
        decision: &AdmittedBackupDecision,
        expected: &BackupFirstPublicationHash,
        stop: &AtomicBool,
    ) -> Result<BackupCommonDatabasesReport> {
        self.record(INTENT).require(context.system)?;
        self.verify_data(context, prepared, decision, expected, stop)?;
        self.visit_roots(context, prepared, stop, |root| {
            self.record(COMPLETE).require(root)
        })?;
        Ok(self.report.clone())
    }
}

pub(super) fn install(
    context: &Context<'_>,
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    expected: &BackupFirstPublicationHash,
    stop: &AtomicBool,
) -> Result<BackupCommonDatabasesReport> {
    let plan = Plan::capture(context, prepared, decision, expected, stop)?;
    plan.preflight(context, prepared, decision, expected, stop)?;
    context.sources.recheck(context.system)?;
    plan.record(INTENT).install(context.system)?;
    // These names are deliberately NOT the active index or disk-root aliases.
    // Until the native Current/Config selector is complete, replacing those
    // aliases would let old in-memory State observe the successor's databases.
    materials::install(context.system, &plan.system_files(prepared)?, stop)?;
    context.sources.for_each(
        stop,
        |root, identity| materials::install(root, &plan.disk_files(identity)?, stop),
        &prepared.task.selection().first().pool_id,
    )?;
    plan.verify_data(context, prepared, decision, expected, stop)?;
    plan.visit_roots(context, prepared, stop, |root| {
        plan.record(COMPLETE).install(root)?;
        plan.record(COMPLETE).require(root)
    })?;
    plan.observe(context, prepared, decision, expected, stop)
}
pub(super) fn observe(
    context: &Context<'_>,
    prepared: &PreparedBackupRegistry,
    decision: &AdmittedBackupDecision,
    expected: &BackupFirstPublicationHash,
    stop: &AtomicBool,
) -> Result<BackupCommonDatabasesReport> {
    Plan::capture(context, prepared, decision, expected, stop)?
        .observe(context, prepared, decision, expected, stop)
}
