//! A complete prospective fanout tree, separate from historical Config routes.
//! No single-successor restriction is copied into this preparation protocol.
use super::*;
use std::collections::BTreeMap;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(super) struct Topology {
    pub(super) source: DiskRegistrationIdentity,
    pub(super) replaced: DiskRegistrationIdentity,
    pub(super) nodes: Vec<BackupPreparedTopologyNode>,
}
fn identity(config: &crate::ValidatedConfig, disk: &DiskConfig) -> DiskRegistrationIdentity {
    DiskRegistrationIdentity {
        pool_id: config.pool_id.clone(),
        disk_id: disk.disk_id.clone(),
        slot_id: disk.slot_id.clone(),
        binding_generation: disk.binding_generation,
    }
}
fn name(path: &std::path::Path) -> Result<String> {
    let value = path.file_name().and_then(|v| v.to_str()).ok_or_else(|| {
        Error::new(
            ErrorCode::InvalidBackupPrefix,
            "a topology root needs a UTF-8 disk basename",
        )
    })?;
    name_policy::validate_name(value)?;
    Ok(value.into())
}
impl Topology {
    pub(super) fn derive(
        config: &crate::ValidatedConfig,
        selection: &Selection,
        targets: &[BackupTopologyTarget],
    ) -> Result<Self> {
        if !(1..=8).contains(&selection.jobs().len()) {
            return Err(invalid("topology requires a nonempty bounded original target set"));
        }
        let paths = selection.jobs().iter()
            .map(|job| job.request.target_paths[0].as_path()).collect::<Vec<_>>();
        Self::derive_request(config, &selection.first().request.source_slot_id, &paths, targets)
    }

    fn derive_request(
        config: &crate::ValidatedConfig,
        source_slot: &str,
        paths: &[&std::path::Path],
        targets: &[BackupTopologyTarget],
    ) -> Result<Self> {
        if targets.len() != paths.len() || !(1..=8).contains(&targets.len()) {
            return Err(invalid(
                "topology must name each original selected physical target exactly once",
            ));
        }
        let source = config
            .disks
            .iter()
            .find(|disk| disk.slot_id == source_slot)
            .ok_or_else(|| invalid("topology source is not the exact frozen source slot"))?;
        let old = config
            .disks
            .iter()
            .filter(|disk| disk.upstream_slot_id.as_deref() == Some(source.slot_id.as_str()))
            .collect::<Vec<_>>();
        if old.len() != 1
            || config
                .disks
                .iter()
                .any(|disk| disk.upstream_slot_id.as_deref() == Some(old[0].slot_id.as_str()))
        {
            return Err(Error::new(
                ErrorCode::UnsupportedOperation,
                "cutover inputs currently replace one original terminal edge; intermediate old subtrees remain retained and unsupported",
            ));
        }
        let mut slots = config
            .disks
            .iter()
            .map(|disk| disk.slot_id.to_ascii_lowercase())
            .collect::<BTreeSet<_>>();
        let mut ids = config
            .disks
            .iter()
            .map(|disk| disk.disk_id.as_str())
            .collect::<BTreeSet<_>>();
        let mut selected = BTreeMap::new();
        for (index, target) in targets.iter().enumerate() {
            name_policy::validate_slot(&target.slot_id)?;
            name_policy::validate_slot(&target.upstream_slot_id)?;
            validate_id(&target.disk_id, "disk")?;
            if !slots.insert(target.slot_id.to_ascii_lowercase()) || !ids.insert(&target.disk_id) {
                return Err(invalid(
                    "prospective slots and disk IDs must be fresh; historical bindings cannot be relabelled",
                ));
            }
            selected.insert(target.slot_id.as_str(), index);
        }
        let mut source_prefix = Vec::new();
        let mut ancestor = source;
        loop {
            source_prefix.push(name(&ancestor.root)?);
            match &ancestor.upstream_slot_id {
                None if ancestor.level == 1 => break,
                Some(upstream) => {
                    ancestor = config
                        .disks
                        .iter()
                        .find(|disk| &disk.slot_id == upstream)
                        .ok_or_else(|| invalid("original lower-level chain is incomplete"))?
                }
                None => {
                    return Err(invalid(
                        "original lower-level chain does not reach a primary",
                    ));
                }
            }
        }
        let mut nodes = Vec::with_capacity(targets.len());
        for target in targets {
            let mut prefix = Vec::new();
            let mut parent = target.upstream_slot_id.as_str();
            let mut seen = BTreeSet::from([target.slot_id.as_str()]);
            while parent != source.slot_id {
                if !seen.insert(parent) {
                    return Err(invalid("prospective topology contains a cycle"));
                }
                let index = *selected.get(parent).ok_or_else(|| invalid(
                    "every new lower-level ancestor must be in the same exact target batch; no placeholder or unrelated source is accepted"))?;
                let upstream = &targets[index];
                prefix.push(name(paths[index])?);
                parent = &upstream.upstream_slot_id;
            }
            let depth =
                u32::try_from(prefix.len() + 1).expect("invariant: eight bounded target ancestors");
            let level = source
                .level
                .checked_add(depth)
                .ok_or_else(|| invalid("prospective topology level overflows"))?;
            prefix.extend(source_prefix.iter().cloned());
            let index = selected[target.slot_id.as_str()];
            nodes.push(BackupPreparedTopologyNode {
                identity: DiskRegistrationIdentity {
                    pool_id: config.pool_id.clone(),
                    disk_id: target.disk_id.clone(),
                    slot_id: target.slot_id.clone(),
                    binding_generation: 1,
                },
                upstream_slot_id: target.upstream_slot_id.clone(),
                level,
                name: name(paths[index])?,
                source_prefix: prefix,
            });
        }
        Ok(Self {
            source: identity(config, source),
            replaced: identity(config, old[0]),
            nodes,
        })
    }
}

impl Engine {
    pub(in crate::engine::backup_preparation) fn check_backup_transaction_topology(
        &self,
        source_slot: &str,
        paths: &[std::path::PathBuf],
        targets: &[BackupTopologyTarget],
    ) -> Result<()> {
        let paths = paths.iter().map(|path| path.as_path()).collect::<Vec<_>>();
        Topology::derive_request(&self.config, source_slot, &paths, targets).map(|_| ())
    }
}
