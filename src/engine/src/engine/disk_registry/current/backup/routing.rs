//! Ordinary continuation of an already selected Backup route. A numeric copy
//! count never chooses peers: only the exact native selection can span the fork.
use super::*;
use crate::route_topology::{Node, Topology};
use crate::{ProtectionPolicy, Repository};

pub(in crate::engine::disk_registry) struct BackupRouting {
    primary: String,
    route: Vec<String>,
    targets: BTreeMap<String, Vec<String>>,
    inherited_repositories: BTreeMap<String, (String, u32)>,
    inherited_policies: BTreeMap<String, ProtectionPolicy>,
}
impl BackupRouting {
    pub(super) fn capture(first: &PreparedBackupFirstCandidate, state: &State) -> Result<Self> {
        let targets = first.targets();
        let top = targets
            .iter()
            .min_by_key(|target| target.level)
            .expect("invariant: admitted Backup selected at least one target");
        let topology = Topology::new(
            first
                .original_config()
                .disks
                .iter()
                .map(|disk| Node {
                    slot: &disk.slot_id,
                    upstream: disk.upstream_slot_id.as_deref(),
                    level: disk.level,
                })
                .chain(targets.iter().map(|target| Node {
                    slot: &target.identity.slot_id,
                    upstream: Some(&target.upstream_slot_id),
                    level: target.level,
                })),
        )?;
        let mut retained = topology.upstream_chain(&top.upstream_slot_id)?;
        retained.reverse();
        let mut selected = retained
            .iter()
            .map(|node| node.slot.to_owned())
            .collect::<Vec<_>>();
        selected.push(top.upstream_slot_id.clone());
        let lower_copies = selected.len() as u32;
        let primary = selected[0].clone();
        selected.extend(targets.iter().map(|target| target.identity.slot_id.clone()));
        let route = topology.explicit(&primary, &selected)?;
        // Existing settings that protected the complete retained source chain
        // follow its explicit replacement. Later/new settings do not inherit
        // this translation merely by choosing an equal numeric count.
        let inherited = |copies| copies >= lower_copies && copies <= lower_copies + 1;
        Ok(Self {
            primary,
            route,
            targets: targets
                .iter()
                .map(|target| {
                    (
                        target.identity.slot_id.clone(),
                        target.source_prefix.clone(),
                    )
                })
                .collect(),
            inherited_repositories: state
                .repositories
                .iter()
                .filter(|(_, repository)| inherited(repository.target_copies))
                .map(|(id, repository)| {
                    (
                        id.clone(),
                        (repository.primary_slot_id.clone(), repository.target_copies),
                    )
                })
                .collect(),
            inherited_policies: state
                .protection_policies
                .iter()
                .filter(|(_, policy)| inherited(policy.target_copies))
                .map(|(id, policy)| (id.clone(), policy.clone()))
                .collect(),
        })
    }
}

impl CurrentNamespaces {
    pub(crate) fn native_backup_route(&self, primary: &str, copies: u32) -> Option<&[String]> {
        self.backup_routing
            .as_ref()
            .filter(|routing| {
                primary.eq_ignore_ascii_case(&routing.primary)
                    && copies as usize == routing.route.len()
            })
            .map(|routing| routing.route.as_slice())
    }

    pub(crate) fn inherited_backup_copies(
        &self,
        repository: &Repository,
        policy: Option<&ProtectionPolicy>,
        primary: &str,
        copies: u32,
    ) -> u32 {
        let Some(routing) = &self.backup_routing else {
            return copies;
        };
        if !primary.eq_ignore_ascii_case(&routing.primary) {
            return copies;
        }
        let inherited = match policy {
            Some(policy) => routing.inherited_policies.get(&policy.policy_id) == Some(policy),
            None => routing
                .inherited_repositories
                .get(&repository.repository_id)
                .is_some_and(|(slot, original)| {
                    slot == &repository.primary_slot_id && *original == repository.target_copies
                }),
        };
        if inherited {
            routing.route.len() as u32
        } else {
            copies
        }
    }

    /// Prefixes are held by the authenticated first-publication Current. This
    /// lookup grants no disk I/O or permission to alter an existing owner path.
    pub(crate) fn native_backup_prefix(&self, slot: &str) -> Option<&[String]> {
        self.backup_routing
            .as_ref()?
            .targets
            .get(slot)
            .map(Vec::as_slice)
    }

    pub(crate) fn apply_native_backup_paths(
        &self,
        state: &mut State,
        file: &str,
        path: &str,
        route: &[String],
    ) -> Result<()> {
        for slot in route {
            let Some(prefix) = self.native_backup_prefix(slot) else {
                continue;
            };
            let path = format!("{}/{path}", prefix.join("/"));
            name_policy::PortablePath::parse(&path)?;
            state
                .file_backup_paths
                .entry(file.to_owned())
                .or_default()
                .entry(slot.clone())
                .or_insert(path);
        }
        Ok(())
    }

    /// Recompute the route and new owner paths during native payload admission,
    /// including cold journal replay. A forged first receipt or numeric count
    /// cannot add a target, return to the replaced terminal, or redirect a body.
    pub(in crate::engine) fn verify_backup_payload_route(
        &self,
        previous: &State,
        next: &State,
        version: &crate::FileVersion,
        placement: &crate::Placement,
    ) -> Result<()> {
        if self.backup_routing.is_none() {
            return Ok(());
        }
        let invalid = || {
            Error::new(
                ErrorCode::PublicationInvalid,
                "ordinary Backup payload differs from its selected native route or retained owner path",
            )
        };
        let expected =
            self.backup_configuration_route(&placement.primary_slot_id, placement.target_copies)?;
        if expected != placement.route_slot_ids {
            return Err(invalid());
        }
        for slot in &expected {
            let Some(prefix) = self.native_backup_prefix(slot) else {
                continue;
            };
            let original = previous
                .file_backup_paths
                .get(&version.file_id)
                .and_then(|paths| paths.get(slot));
            let retained;
            let expected_path = match original {
                Some(path) => path.as_str(),
                None => {
                    let owner = next
                        .file_locations
                        .get(&version.file_id)
                        .or_else(|| next.file_names.get(&version.file_id))
                        .ok_or_else(invalid)?;
                    retained = format!("{}/{}", prefix.join("/"), owner);
                    retained.as_str()
                }
            };
            if next
                .file_backup_paths
                .get(&version.file_id)
                .and_then(|paths| paths.get(slot))
                .map(String::as_str)
                != Some(expected_path)
            {
                return Err(invalid());
            }
        }
        Ok(())
    }
}
