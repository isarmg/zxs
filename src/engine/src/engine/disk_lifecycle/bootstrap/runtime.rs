//! System-side startup/stop barriers. No method probes a stopped directory.
use super::{store::BootstrapStore, *};
use crate::{
    Engine, Error, ErrorCode, ValidatedConfig,
    authority_selection::SelectionGuard,
    disk::{DiskHandle, DiskResources, existing_private_dir},
    engine::membership_transition::pending::activation::discover_startup_in_store,
    persist::layout,
};
use platform_io::fs_safety::{
    EntryName,
    linux::{MountPolicy, OpenAt2Root},
};
use std::sync::Arc;

pub(crate) const DIRECTORY: &str = "bootstrap-discovery";

pub(crate) fn management_error(error: BootstrapError) -> Error {
    Error::from(std::io::Error::other(error)).context(ErrorCode::RecoveryRequired,
        "bootstrap discovery is closed; use explicit retained metadata roots for management recovery")
}

/// Keep the ledger's exclusive lock until resource admission and the system
/// writer lock have completed. A cooperating stop cannot race this snapshot.
pub(crate) fn open_store(root: &Path) -> crate::Result<BootstrapStore> {
    let path = root.join(DIRECTORY);
    let reader = OpenAt2Root::open(&path, MountPolicy::SameMount)
        .map_err(|error| management_error(BootstrapError::fs("open system locator", error)))?;
    BootstrapStore::hold(reader.into_directory(), path).map_err(management_error)
}

fn create_store(root: &Path) -> crate::Result<BootstrapStore> {
    let system = existing_private_dir(root)?;
    system.create_child(&EntryName::new(DIRECTORY)?)?;
    open_store(root)
}

pub(in crate::engine) fn observe<'a>(
    config: &ValidatedConfig,
    handles: impl Iterator<Item = &'a Arc<DiskHandle>>,
) -> crate::Result<Vec<RootObservation>> {
    handles
        .map(|handle| {
            let disk = config
                .io_disks()
                .find(|disk| disk.disk_id == handle.seal.disk_id)
                .ok_or_else(|| {
                    Error::new(
                        ErrorCode::RecoveryRequired,
                        "bootstrap observation is outside admitted resource set",
                    )
                })?;
            RootObservation::capture(
                &DiskRegistrationIdentity {
                    pool_id: config.pool_id.clone(),
                    disk_id: disk.disk_id.clone(),
                    slot_id: disk.slot_id.clone(),
                    binding_generation: disk.binding_generation,
                },
                &disk.root,
                &handle.root.directory(&disk.root)?,
                disk.metadata_member,
            )
            .map_err(management_error)
        })
        .collect()
}

/// Only initialization and explicit retained-root recovery call this. Ordinary
/// open never recreates a missing directory/head from the system projection.
pub(crate) fn initialize(
    root: &Path,
    config: &ValidatedConfig,
    resources: &DiskResources,
) -> crate::Result<()> {
    let observations = observe(config, resources.handles())?;
    let recovered =
        crate::engine::membership_transition::readmission::persist_recovered_admissions(
            root, config, resources,
        )?;
    let mut store = create_store(root)?;
    let locator = if recovered.is_empty() {
        store.initialize(&config.pool_id, observations)
    } else {
        store.initialize_recovered_readmissions(&config.pool_id, observations, &recovered)
    }
    .map_err(management_error)?;
    store.sync_current(&locator).map_err(management_error)
}

/// A held restriction for inspecting the original activation directories. It
/// does not authenticate storage authority or permit ordinary pool admission.
/// Keep it alive until all unregistered probe handles have been dropped, or
/// consume it through the corresponding held activation/completion method.
pub struct InitialActivationProbeGuard {
    root: PathBuf,
    config: ValidatedConfig,
    store: BootstrapStore,
    selection: SelectionGuard,
    source: InitialProbeSource,
}

enum InitialProbeSource {
    Configuration,
    Initialization(ValidatedConfig),
}

#[derive(Clone, Copy)]
enum InitialProjection {
    Exact,
    CreatedRoots,
}

fn initial_configuration(
    selection: &SelectionGuard,
    expected: &ValidatedConfig,
    workspace_limits: Option<crate::RetainedWorkspaceLimits>,
    relation: InitialProjection,
) -> crate::Result<ValidatedConfig> {
    selection.require_legacy()?;
    let bytes = selection
        .read_system_entry(&layout::SYSTEM_CONFIGURATION, crate::disk::MAX_METADATA)
        .map_err(|source| {
            management_error(BootstrapError::fs("read initial system configuration", source))
        })?;
    let config: crate::Config = storage_contract::decode_strict(&bytes).map_err(|source| {
        management_error(BootstrapError::encoding(
            "decode initial system configuration",
            source,
        ))
    })?;
    let projected = ValidatedConfig::from_stored_layout(config)?.with_workspace_limits(workspace_limits)?;
    let matches = match relation {
        InitialProjection::Exact => storage_contract::canonical_json(&projected)?
            == storage_contract::canonical_json(expected)?,
        InitialProjection::CreatedRoots => projected.is_initialization_result_of(expected),
    };
    if !matches {
        return Err(management_error(BootstrapError::ManagementOnly(
            "initial activation configuration changed during probe admission",
        )));
    }
    Ok(projected)
}

impl Engine {
    /// Admit only the complete, unchanged initialization locator before the
    /// caller's first storage-directory stat/open. No storage root is touched.
    ///
    /// # Errors
    /// Rejects an unreadable or changed saved configuration before touching the
    /// bootstrap store. Also rejects a missing, damaged, locked or successor
    /// locator, or a root set differing from the supplied initial configuration.
    pub fn begin_initial_activation_probe(
        root: &Path,
        config: &ValidatedConfig,
    ) -> crate::Result<InitialActivationProbeGuard> {
        Self::begin_initial_probe(root, config, InitialProjection::Exact)
    }

    /// Correlate the original unbound initialization request with the saved
    /// created-root projection, before the first selected disk is inspected.
    /// This guard restricts discovery; completion still requires the actual
    /// held-root and fixed-Genesis readback in `open_initial_storage_completion`.
    ///
    /// # Errors
    /// Rejects any change other than the initializer's root-binding feedback,
    /// or any stopped, successor, damaged or competing discovery state.
    pub fn begin_initial_storage_completion(
        root: &Path,
        input: &ValidatedConfig,
    ) -> crate::Result<InitialActivationProbeGuard> {
        Self::begin_initial_probe(root, input, InitialProjection::CreatedRoots)
    }

    fn begin_initial_probe(
        root: &Path,
        config: &ValidatedConfig,
        relation: InitialProjection,
    ) -> crate::Result<InitialActivationProbeGuard> {
        let root = existing_private_dir(root)?.path().to_path_buf();
        // The first bootstrap constructor may recreate its idle lock. Reject
        // unreadable, unsupported or changed persisted configuration before it,
        // and retain this same SYSTEM selection until the actual writer takes it.
        let selection = SelectionGuard::legacy(&root)?;
        crate::engine::disk_lifecycle::terminal::require_active_startup(&root)?;
        let projected = initial_configuration(&selection, config, None, relation)?;
        let store = open_store(&root)?;
        crate::engine::membership_transition::pending::activation::exit::stop_plan::require_no_pending_stop(&root, &store)?;
        let locator = store.reload(&config.pool_id).map_err(management_error)?;
        if config.generation().is_some()
            || locator.head.generation != 1
            || !locator.record.stops.is_empty()
            || locator.record.roots.len() != config.disks.len()
            || config.disks.iter().any(|disk| {
                !locator.record.roots.iter().any(|observed| {
                    observed.disk_id == disk.disk_id
                        && observed.slot_id == disk.slot_id
                        && observed.binding_generation == disk.binding_generation
                        && Path::new(&observed.path) == disk.root.as_path()
                        && observed.metadata_member == disk.metadata_member
                })
            })
        {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "initial activation cannot probe roots after locator lifecycle changes",
            ));
        }
        Ok(InitialActivationProbeGuard {
            root,
            config: projected,
            store,
            selection,
            source: match relation {
                InitialProjection::Exact => InitialProbeSource::Configuration,
                InitialProjection::CreatedRoots => {
                    InitialProbeSource::Initialization(config.clone())
                }
            },
        })
    }

    /// Transfer a first-activation probe guard and its selected directory handles
    /// to ordinary registered resources without releasing the bootstrap lock.
    /// Storage authority still comes from the existing restricted verifier.
    ///
    /// # Errors
    /// Rejects changed configuration, a discovered successor, or any ordinary
    /// held-resource, writer-lock or recovery failure. All supplied handles are
    /// consumed, including on failure.
    pub fn open_initial_activation_for_recovery_held(
        admission: InitialActivationProbeGuard,
        workspace_limits: Option<crate::RetainedWorkspaceLimits>,
        directories: Vec<std::fs::File>,
    ) -> crate::Result<Self> {
        if !matches!(&admission.source, InitialProbeSource::Configuration) {
            return Err(management_error(BootstrapError::ManagementOnly(
                "an initialization result requires its fixed-genesis completion readback",
            )));
        }
        Self::open_initial_probe(admission, workspace_limits, directories, |_, _| Ok(()))
            .map(|(engine, ())| engine)
    }

    /// Transfer the same probe, SYSTEM selection and original directory handles
    /// into recovery, retaining actual initialization-result evidence. The full
    /// fixed-Genesis/five-area readback precedes writer creation and recovery.
    ///
    /// # Errors
    /// Rejects a non-initialization probe or any changed original request,
    /// physical source, fixed Genesis, successor, lock or recovery failure.
    pub fn open_initial_storage_completion(
        admission: InitialActivationProbeGuard,
        workspace_limits: Option<crate::RetainedWorkspaceLimits>,
        directories: Vec<std::fs::File>,
    ) -> crate::Result<(Self, crate::InitialStorageObservation)> {
        let input = match &admission.source {
            InitialProbeSource::Initialization(input) => input.clone(),
            InitialProbeSource::Configuration => {
                return Err(management_error(BootstrapError::ManagementOnly(
                    "storage completion requires its original initialization request",
                )));
            }
        };
        Self::open_initial_probe(admission, workspace_limits, directories, |config, resources| {
            crate::engine::initial_storage::observe(&input, config, resources)
        })
    }

    fn open_initial_probe<T>(
        admission: InitialActivationProbeGuard,
        workspace_limits: Option<crate::RetainedWorkspaceLimits>,
        directories: Vec<std::fs::File>,
        observe: impl FnOnce(&ValidatedConfig, &DiskResources) -> crate::Result<T>,
    ) -> crate::Result<(Self, T)> {
        let InitialActivationProbeGuard { root, config, store, selection, source: _ } = admission;
        // Every raw directory is consumed on failure. The same SYSTEM selection
        // and bootstrap lock survive physical admission into the actual writer.
        let prepared: crate::Result<(Self, T)> = (|| {
            crate::engine::disk_lifecycle::terminal::require_active_startup(&root)?;
            let projected = initial_configuration(
                &selection, &config, workspace_limits, InitialProjection::Exact,
            )?;
            let config = discover_startup_in_store(&store, projected)?;
            if config.generation().is_some() {
                return Err(management_error(BootstrapError::ManagementOnly(
                    "a discovered successor cannot complete initial storage activation",
                )));
            }
            let resources = Arc::new(DiskResources::recover_held(&config, directories)?);
            let observation = observe(&config, &resources)?;
            let locks = crate::engine::config::acquire_locks_selected(
                &root, &resources, selection,
            )?;
            let engine = Self::recovery_resources_with_locks(root, config, resources, locks)?;
            Ok((engine, observation))
        })();
        let (engine, observation) = prepared?;
        Ok((Self::finish_held_recovery(engine, store)?, observation))
    }
}

/// Not serializable/constructible by callers. A live system ledger lock and
/// exact reload + file/directory barriers back this token, not a boolean.
pub(crate) struct DurableDiskStop {
    binding: DiskRegistrationIdentity,
    operation_id: OperationId,
    intent_digest: IntentDigest,
    _store: BootstrapStore,
}
/// Exact admitted exclusion, not proof that older logical I/O domains drained.
/// The disk layer must independently acquire and retain that global guard.
pub(crate) struct RecoveredDiskStop {
    binding: DiskRegistrationIdentity,
    _admission: ValidatedConfig,
}
impl RecoveredDiskStop {
    pub(crate) fn admission_epoch(&self) -> crate::Result<Option<String>> {
        self._admission.io_admission_epoch(&self.binding.disk_id)
    }

    pub(crate) fn identity(&self) -> &DiskRegistrationIdentity {
        &self.binding
    }
}
impl DurableDiskStop {
    /// Continue startup with the same discovery lock, including when a
    /// generation authenticates more than one historical stop.
    pub(in crate::engine) fn into_store(self) -> BootstrapStore {
        self._store
    }

    pub(crate) fn matches(
        &self,
        binding: &DiskRegistrationIdentity,
        operation_id: OperationId,
        intent_hash: &str,
    ) -> bool {
        self.binding == *binding
            && self.operation_id == operation_id
            && self.intent_digest.to_string() == intent_hash
    }
}

impl Engine {
    /// The exact retained closure is prepared before the irreversible stop.
    /// The caller cannot close a root or claim D3 before this barrier succeeds.
    pub(crate) fn synchronize_disk_exit_stop(
        &self,
        binding: &DiskRegistrationIdentity,
        operation_id: OperationId,
        intent_hash: &str,
    ) -> crate::Result<DurableDiskStop> {
        self.disk_exit_stop(binding, operation_id, intent_hash, true, || open_store(&self.root))
    }

    /// A startup exclusion plus exact authenticated Exit, not an absent-file
    /// observation. Global logical I/O domains must still be closed separately.
    pub(crate) fn recover_disk_exit_io(
        &self,
        stop: &DurableDiskStop,
        binding: &DiskRegistrationIdentity,
    ) -> crate::Result<RecoveredDiskStop> {
        self.recover_disk_exit_io_for_config(stop, binding, &self.config)
    }

    /// The prepared Exit driver has already installed and independently read
    /// every retained control copy. Its authenticated restricted generation can
    /// exclude an unopened target before the active Engine projection changes.
    /// This is not a drain proof: the caller must still acquire RecoveredDiskIo.
    pub(crate) fn recover_disk_exit_io_for_config(
        &self,
        stop: &DurableDiskStop,
        binding: &DiskRegistrationIdentity,
        config: &ValidatedConfig,
    ) -> crate::Result<RecoveredDiskStop> {
        let disk = config
            .disks
            .iter()
            .find(|disk| disk.disk_id == binding.disk_id)
            .ok_or_else(|| {
                Error::new(
                    ErrorCode::RecoveryRequired,
                    "stopped registration is absent",
                )
            })?;
        if &stop.binding != binding
            || !config.excludes_io_disk(&binding.disk_id)
            || self.disk_resources.available(disk).is_some()
        {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "recovered stop requires exact startup resource exclusion",
            ));
        }
        let generation = config.generation().ok_or_else(|| {
            Error::new(
                ErrorCode::RecoveryRequired,
                "recovered stop lacks authenticated generation",
            )
        })?;
        generation.portable.verify_bootstrap_stop(
            binding,
            stop.operation_id,
            &stop.intent_digest.to_string(),
        )?;
        Ok(RecoveredDiskStop {
            binding: binding.clone(),
            _admission: config.clone(),
        })
    }

    /// Recovery may re-establish barriers, never invent a previously absent stop.
    pub(crate) fn recover_disk_exit_stop(
        &self,
        binding: &DiskRegistrationIdentity,
        operation_id: OperationId,
        intent_hash: &str,
    ) -> crate::Result<DurableDiskStop> {
        self.disk_exit_stop(binding, operation_id, intent_hash, false, || open_store(&self.root))
    }

    /// Startup has already retained this locator across read-only preflight.
    /// Reuse the same stop admission, without releasing/reacquiring its lock.
    pub(in crate::engine) fn recover_disk_exit_stop_held(
        &self,
        binding: &DiskRegistrationIdentity,
        operation_id: OperationId,
        intent_hash: &str,
        store: BootstrapStore,
    ) -> crate::Result<DurableDiskStop> {
        self.disk_exit_stop(binding, operation_id, intent_hash, false, || Ok(store))
    }

    fn disk_exit_stop(
        &self,
        binding: &DiskRegistrationIdentity,
        operation_id: OperationId,
        intent_hash: &str,
        append: bool,
        acquire: impl FnOnce() -> crate::Result<BootstrapStore>,
    ) -> crate::Result<DurableDiskStop> {
        if binding.pool_id != self.config.pool_id
            || !self.config.disks.iter().any(|disk| {
                disk.disk_id == binding.disk_id
                    && disk.slot_id == binding.slot_id
                    && disk.binding_generation == binding.binding_generation
            })
        {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "durable stop does not identify this admitted registration",
            ));
        }
        let intent_digest = IntentDigest::parse(intent_hash).map_err(management_error)?;
        let mut store = acquire()?;
        let prepared = crate::engine::membership_transition::pending::activation::exit::stop_plan::synchronize(
            self, &mut store, binding, operation_id, intent_digest, append,
        )?;
        let locator = match prepared {
            Some(locator) => locator,
            None => {
                let locator = store
                    .reload(&self.config.pool_id)
                    .map_err(management_error)?;
                store.sync_current(&locator).map_err(management_error)?;
                locator
            }
        };
        let present = locator.stop_hints().any(|stop| {
            stop.disk_id == binding.disk_id
                && stop.slot_id == binding.slot_id
                && stop.binding_generation == binding.binding_generation
                && stop.operation_id == operation_id
                && stop.intent_digest == intent_digest
        });
        if !present {
            // Explicit system-loss recovery records only supplied retained
            // addresses. Unlisted roots remain forbidden without probing
            // them to manufacture a historical directory observation.
            if !locator.excludes_binding(binding) || !self.config.excludes_io_disk(&binding.disk_id)
            {
                return Err(Error::new(
                    ErrorCode::RecoveryRequired,
                    "exact stop or durable retained-root exclusion is absent",
                ));
            }
            let generation = self.config.generation().ok_or_else(|| {
                Error::new(
                    ErrorCode::RecoveryRequired,
                    "retained-root exclusion has no Exit authority",
                )
            })?;
            generation
                .portable
                .verify_bootstrap_stop(binding, operation_id, intent_hash)?;
        }
        Ok(DurableDiskStop {
            binding: binding.clone(),
            operation_id,
            intent_digest,
            _store: store,
        })
    }

    /// Called with the already authenticated successor and held pending/retained
    /// resources before the first successor marker. Never follow Config roots.
    pub(crate) fn synchronize_bootstrap_roots(
        &self,
        config: &ValidatedConfig,
        handles: &[Arc<DiskHandle>],
    ) -> crate::Result<()> {
        if config.generation().is_none() || config.pool_id != self.config.pool_id {
            return Err(Error::new(
                ErrorCode::RecoveryRequired,
                "bootstrap root expansion requires an authenticated same-pool successor",
            ));
        }
        let observations = observe(config, handles.iter())?;
        let mut store = open_store(&self.root)?;
        crate::engine::membership_transition::pending::activation::root_extension::synchronize(
            self,
            config,
            observations,
            &mut store,
        )
    }
}
