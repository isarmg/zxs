//! Preserve original disk seals before selecting the same native Backup
//! generation. Every exchange is classified from its exact old/new inode pair.
use super::*;
use crate::disk::{FileStamp, HeldRegular, InstallExchange};

const DIRECTORY: &str = "backup-serving-seals";
const GENERATION: &str = "metadata/pool-membership/generation.json";
const MAX_ENVELOPE: usize = 16 * 1024;

#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Envelope {
    format: String,
    operation: OperationId,
    authority: String,
    root: PhysicalBinding,
    stage: PhysicalBinding,
    old: FileStamp,
    new: FileStamp,
}

pub(super) struct Original {
    root: DiskRoot,
    identity: DiskSeal,
    stage: DiskRoot,
    envelope: Envelope,
    // Prevent old inode reuse before the exchange and the all-root barrier.
    old: HeldRegular,
}
#[derive(Clone, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Recipe {
    identity: DiskSeal,
    stage_path: PathBuf,
    pair: Envelope,
}

pub(super) struct Roots {
    originals: Vec<Original>,
    targets: Vec<Arc<DiskHandle>>,
    authority: Vec<u8>,
    handles: Vec<Arc<DiskHandle>>,
}

impl Original {
    fn prepare(
        handle: Arc<DiskHandle>,
        config: &ValidatedConfig,
        operation: OperationId,
        authority: &str,
    ) -> Result<Self> {
        let root = &handle.root;
        let disk = config
            .disks
            .iter()
            .find(|disk| disk.disk_id == handle.seal.disk_id)
            .expect("invariant: native Backup retains every original disk");
        let seal = identity_at(config, disk, &root.directory(root.path())?)?;
        let bytes = canonical_json(&seal)?;
        let old = HeldRegular::capture(root, "disk.json", MAX_METADATA)?
            .ok_or_else(|| invalid("original disk seal disappeared before Backup selection"))?;
        validate_stored_seal(&decode_seal(old.bytes())?, &handle.seal)?;
        if root
            .optional_metadata(&root.path().join(GENERATION))?
            .is_some()
        {
            return Err(invalid(
                "original Backup selector already started; resume this operation through its native cold boundary",
            ));
        }
        let directory = root.path().join(DIRECTORY).join(operation.to_string());
        root.private_dir(&directory)?;
        let history = root.private_child(&directory)?;
        let mut selected = None;
        let mut count = 0;
        for entry in history.entries(history.path())? {
            count += 1;
            if count > 128 {
                return Err(invalid(
                    "Backup retained seal attempts exceed the bounded selection budget",
                ));
            }
            let stage =
                history.private_child(&history.path().join(entry.map_err(fs_error)?.as_path()))?;
            let Some(intent) = HeldRegular::capture(&stage, "intent.json", MAX_ENVELOPE)? else {
                // A pre-intent orphan never selected anything. Keep its bytes,
                // count it, and do not make it the next attempt's old seal.
                continue;
            };
            let envelope: Envelope = storage_contract::decode_strict(intent.bytes())?;
            if envelope.format != "zxs-backup-preserved-seal"
                || envelope.operation != operation
                || envelope.authority != authority
                || envelope.root != PhysicalBinding::of(root)?
                || envelope.stage != PhysicalBinding::of(&stage)?
                || envelope.old != *old.stamp()
                || envelope.new.digest != blake3::hash(&bytes).to_hex().as_str()
                || envelope.new.bytes != bytes.len() as u64
            {
                return Err(invalid(
                    "retained Backup seal attempt differs from the actual native selection",
                ));
            }
            require_stamp(&stage, "payload", &envelope.new)?;
            if selected.replace((stage, envelope)).is_some() {
                return Err(invalid(
                    "multiple completed seal intents claim this Backup operation",
                ));
            }
        }
        let (stage, envelope) = match selected {
            Some(pair) => pair,
            None => {
                if root.available()? < RESERVE + 64 * 1024 {
                    return Err(Error::new(
                        ErrorCode::NoSpace,
                        "original Backup seal lacks preserving-stage space",
                    ));
                }
                let path = history.path().join(crate::new_id("req")?);
                let physical = history.create_private_directory_synced(&path)?;
                let stage = history.private_child_matching(&path, physical)?;
                stage.create_install_file("payload", &bytes)?;
                let new = HeldRegular::capture(&stage, "payload", MAX_METADATA)?
                    .ok_or_else(|| invalid("staged Backup seal disappeared"))?;
                if new.bytes() != bytes {
                    return Err(invalid("staged Backup seal changed"));
                }
                let envelope = Envelope {
                    format: "zxs-backup-preserved-seal".into(),
                    operation,
                    authority: authority.into(),
                    root: PhysicalBinding::of(root)?,
                    stage: PhysicalBinding::of(&stage)?,
                    old: old.stamp().clone(),
                    new: new.stamp().clone(),
                };
                stage.write_json(&stage.path().join("intent.json"), &envelope, false)?;
                root.sync_dir(&directory)?;
                root.sync_dir(&root.path().join(DIRECTORY))?;
                root.sync_dir(root.path())?;
                (stage, envelope)
            }
        };
        Ok(Self {
            root: handle.root.private_child(handle.root.path())?,
            identity: handle.seal.clone(),
            stage,
            envelope,
            old,
        })
    }

    fn verify(&self) -> Result<()> {
        self.old.check_running()?;
        let root = &self.root;
        if PhysicalBinding::of(root)? != self.envelope.root
            || PhysicalBinding::of(&DiskRoot::open(root.path())?)? != self.envelope.root
            || PhysicalBinding::of(&self.stage)? != self.envelope.stage
        {
            return Err(invalid(
                "Backup seal publication lost its original root or staging directory",
            ));
        }
        require_bytes(
            &self.stage,
            "intent.json",
            &canonical_json(&self.envelope)?,
            MAX_ENVELOPE,
        )?;
        require_stamp(root, "disk.json", &self.envelope.new)?;
        require_stamp(&self.stage, "payload", &self.envelope.old)
    }

    fn apply(&self, common: &[u8], authority: &[u8]) -> Result<()> {
        let root = &self.root;
        require_bytes(root, SELECTED, common, MAX_PLAN)?;
        self.old.check_running()?;
        require_bytes(
            &self.stage,
            "intent.json",
            &canonical_json(&self.envelope)?,
            MAX_ENVELOPE,
        )?;
        let actual = HeldRegular::capture(root, "disk.json", MAX_METADATA)?
            .ok_or_else(|| invalid("original seal disappeared during Backup cutover"))?;
        if actual.stamp() == &self.envelope.old {
            root.publish_install_preserving(
                &self.stage,
                "disk.json",
                InstallExchange {
                    staged: &self.envelope.new,
                    old: Some(&self.envelope.old),
                    maximum: MAX_METADATA,
                },
                &mut |_| Ok(()),
            )?;
        } else if actual.stamp() != &self.envelope.new {
            return Err(invalid(
                "Backup seal cutover found an unknown inode; all data retained",
            ));
        }
        self.verify()?;
        install_preparation_bytes(root, &root.path().join(GENERATION), authority)?;
        require_bytes(root, GENERATION, authority, MAX_METADATA)?;
        root.sync_dir(&root.path().join("metadata/pool-membership"))?;
        root.sync_dir(root.path())
    }
}

fn require_stamp(root: &DiskRoot, leaf: &str, expected: &FileStamp) -> Result<()> {
    let held = HeldRegular::capture(root, leaf, MAX_METADATA)?
        .ok_or_else(|| invalid("Backup cutover lost a selected native file"))?;
    if held.stamp() != expected {
        return Err(invalid(
            "Backup cutover native file changed inode or bytes; no replacement",
        ));
    }
    held.check_running()
}

impl Roots {
    pub(super) fn capture(
        engine: &Engine,
        native: &Native,
        targets: Vec<Arc<DiskHandle>>,
    ) -> Result<Self> {
        let config = native.writer.config();
        let generation = config
            .generation()
            .expect("invariant: native Backup generation");
        let authority = canonical_json(&generation.portable)?;
        let mut originals = Vec::with_capacity(engine.config.disks.len());
        for disk in &engine.config.disks {
            let handle = engine.disk_resources.get(disk)?;
            engine._locks.verify_disk(disk, &handle.root)?;
            originals.push(Original::prepare(
                Arc::clone(handle),
                config,
                native.registry.candidate().candidate().operation_id,
                &generation.hash,
            )?);
        }
        let handles = engine
            .disk_resources
            .handles()
            .map(Arc::clone)
            .chain(targets.iter().map(Arc::clone))
            .collect();
        Ok(Self {
            originals,
            targets,
            authority,
            handles,
        })
    }
    pub(super) fn recipes(&self) -> Vec<Recipe> {
        self.originals
            .iter()
            .map(|original| Recipe {
                identity: original.identity.clone(),
                stage_path: original.stage.path().to_owned(),
                pair: original.envelope.clone(),
            })
            .collect()
    }
    pub(super) fn visit(&self, mut action: impl FnMut(&DiskRoot) -> Result<()>) -> Result<()> {
        for original in &self.originals {
            action(&original.root)?;
        }
        for target in &self.targets {
            action(&target.root)?;
        }
        Ok(())
    }
    pub(super) fn apply(&self, common: &[u8]) -> Result<()> {
        self.visit(|root| require_bytes(root, SELECTED, common, MAX_PLAN))?;
        for original in &self.originals {
            original.apply(common, &self.authority)?;
        }
        self.verify()
    }
    pub(super) fn verify(&self) -> Result<()> {
        for original in &self.originals {
            original.verify()?;
        }
        self.visit(|root| require_bytes(root, GENERATION, &self.authority, MAX_METADATA))?;
        for target in &self.targets {
            let stored = decode_seal(
                &target
                    .root
                    .read_bytes(&target.root.path().join("disk.json"), MAX_METADATA)?,
            )?;
            validate_stored_seal(&stored, &target.seal)?;
        }
        Ok(())
    }
    pub(super) fn handles(&self) -> Vec<Arc<DiskHandle>> {
        self.handles.iter().map(Arc::clone).collect()
    }
    pub(super) fn completion(&self) -> Result<Vec<FileStamp>> {
        self.verify()?;
        let mut stamps = Vec::new();
        self.visit(|root| {
            for path in ["disk.json", GENERATION] {
                let parent = root.private_child(
                    root.path()
                        .join(path)
                        .parent()
                        .expect("invariant: native file has a parent"),
                )?;
                let leaf = Path::new(path)
                    .file_name()
                    .expect("invariant: fixed file name")
                    .to_str()
                    .expect("invariant: ASCII file name");
                let file = HeldRegular::capture(&parent, leaf, MAX_METADATA)?
                    .ok_or_else(|| invalid("completed Backup native file vanished"))?;
                stamps.push(file.stamp().clone());
            }
            Ok(())
        })?;
        Ok(stamps)
    }
}

/// Restore only the exact old/new pair selected before the first active seal.
/// Native admission has already fixed every root and the complete authority;
/// stored envelopes merely identify resources that must still be present.
pub(super) fn recover_originals(
    roots: &[(DiskConfig, DiskRoot)],
    first: &crate::engine::membership_transition::PreparedBackupFirstCandidate,
    config: &ValidatedConfig,
    recipes: &[Recipe],
    common: &[u8],
    verify_only: bool,
) -> Result<()> {
    let authority = canonical_json(
        &config
            .generation()
            .expect("invariant: native Backup startup")
            .portable,
    )?;
    if recipes.len() != first.original_config().disks.len() {
        return Err(invalid(
            "Backup serving selection omits an original seal pair",
        ));
    }
    for (disk, recipe) in first.original_config().disks.iter().zip(recipes) {
        let (_, root) = roots
            .iter()
            .find(|(actual, _)| actual.disk_id == disk.disk_id)
            .ok_or_else(|| invalid("Backup resume lacks an original held root"))?;
        let old_seal = identity_at(first.original_config(), disk, &root.directory(root.path())?)?;
        validate_stored_seal(&recipe.identity, &old_seal)?;
        let expected = identity_at(config, disk, &root.directory(root.path())?)?;
        let selected_parent = root
            .path()
            .join(DIRECTORY)
            .join(first.result().operation_id.to_string());
        if recipe.stage_path.parent() != Some(selected_parent.as_path())
            || recipe.stage_path.components().any(|part| {
                !matches!(
                    part,
                    std::path::Component::RootDir | std::path::Component::Normal(_)
                )
            })
            || recipe.pair.format != "zxs-backup-preserved-seal"
            || recipe.pair.operation != first.result().operation_id
            || recipe.pair.authority != first.authority_hash()
            || recipe.pair.root != PhysicalBinding::of(root)?
            || recipe.pair.old.digest != blake3::hash(&canonical_json(&old_seal)?).to_hex().as_str()
            || recipe.pair.new.digest != blake3::hash(&canonical_json(&expected)?).to_hex().as_str()
        {
            return Err(invalid(
                "Backup resume seal pair differs from native identity and original operation",
            ));
        }
        require_bytes(root, SELECTED, common, MAX_PLAN)?;
        let stage = root.private_child(&recipe.stage_path)?;
        if PhysicalBinding::of(&stage)? != recipe.pair.stage {
            return Err(invalid("Backup resume preserving stage was replaced"));
        }
        require_bytes(
            &stage,
            "intent.json",
            &canonical_json(&recipe.pair)?,
            MAX_ENVELOPE,
        )?;
        let current = HeldRegular::capture(root, "disk.json", MAX_METADATA)?
            .ok_or_else(|| invalid("Backup resume disk seal is absent"))?;
        let old = if current.stamp() == &recipe.pair.old {
            if verify_only {
                return Err(invalid(
                    "completed Backup switch regressed an original seal; no repair",
                ));
            }
            current
        } else if current.stamp() == &recipe.pair.new {
            HeldRegular::capture(&stage, "payload", MAX_METADATA)?
                .ok_or_else(|| invalid("Backup displaced original seal is absent"))?
        } else {
            return Err(invalid(
                "Backup resume found neither selected old nor selected new seal",
            ));
        };
        if old.stamp() != &recipe.pair.old {
            return Err(invalid("Backup resume changed the preserved original seal"));
        }
        let original = Original {
            root: root.private_child(root.path())?,
            identity: recipe.identity.clone(),
            stage,
            envelope: recipe.pair.clone(),
            old,
        };
        if verify_only {
            original.verify()?;
            require_bytes(root, GENERATION, &authority, MAX_METADATA)?;
        } else {
            original.apply(common, &authority)?;
        }
    }
    Ok(())
}

pub(super) fn completed_files(
    roots: &[(DiskConfig, DiskRoot)],
    config: &ValidatedConfig,
) -> Result<Vec<FileStamp>> {
    let generation = config
        .generation()
        .expect("invariant: native Backup startup");
    let authority = canonical_json(&generation.portable)?;
    let mut files = Vec::with_capacity(2 * roots.len());
    for (disk, root) in roots {
        let actual = decode_seal(&root.read_bytes(&root.path().join("disk.json"), MAX_METADATA)?)?;
        validate_stored_seal(
            &actual,
            &identity_at(config, disk, &root.directory(root.path())?)?,
        )?;
        require_bytes(root, GENERATION, &authority, MAX_METADATA)?;
        for path in ["disk.json", GENERATION] {
            let full = root.path().join(path);
            let parent =
                root.private_child(full.parent().expect("invariant: native file parent"))?;
            let leaf = full
                .file_name()
                .expect("invariant: native file leaf")
                .to_str()
                .expect("invariant: ASCII leaf");
            files.push(
                HeldRegular::capture(&parent, leaf, MAX_METADATA)?
                    .ok_or_else(|| invalid("Backup completion lost a native selected file"))?
                    .stamp()
                    .clone(),
            );
        }
    }
    Ok(files)
}
