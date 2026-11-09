//! Native active-Backup targets retain the same ordinary forest geometry as
//! their first installation. This is a metadata projection, not route authority.
use super::*;
use crate::ValidatedConfig;

pub(super) struct Target<'a> {
    slot: &'a str,
    prefix: &'a [String],
    generation: u64,
}

pub(super) fn selected(config: &ValidatedConfig) -> Result<Vec<Target<'_>>> {
    let Some(generation) = config.generation() else {
        return Ok(Vec::new());
    };
    // Only a continuation admitted from the exact original native Publication
    // may supply these slots/prefixes. A bare Backup config remains closed.
    let Some(targets) = generation.backup_layout_targets()? else {
        return Ok(Vec::new());
    };
    targets
        .iter()
        .map(|(slot, prefix)| {
            let disk = config
                .disks
                .iter()
                .find(|disk| &disk.slot_id == slot)
                .ok_or_else(|| invalid("BACKUP_LAYOUT_BINDING_MISSING", slot.clone()))?;
            if disk.metadata_member || prefix.is_empty() {
                return Err(invalid("BACKUP_LAYOUT_BINDING_INVALID", slot.clone()));
            }
            for name in prefix {
                name_policy::validate_name(name)?;
            }
            Ok(Target {
                slot,
                prefix,
                generation: disk.binding_generation,
            })
        })
        .collect()
}

pub(super) fn add_tree(
    builder: &mut Builder,
    state: &State,
    repository: &Repository,
    plan: &LayoutPlan<'_>,
    targets: &[Target<'_>],
) -> Result<()> {
    for target in targets {
        let skeleton = skeleton(plan, target)?;
        builder.skeleton(
            target.slot,
            &repository.history_tree_id,
            plan.repository_id(),
            &skeleton,
        )?;
        for (key, file) in plan.files() {
            let versions = versions(state, file, target)?;
            if versions.is_empty() {
                continue;
            }
            builder.skeleton(
                target.slot,
                key.history_tree_id,
                plan.repository_id(),
                &skeleton,
            )?;
            builder.file(
                target.slot,
                key.history_tree_id,
                plan.repository_id(),
                file_goal(plan, &skeleton, *key, file, target.slot, &versions)?,
            )?;
        }
    }
    Ok(())
}

fn skeleton(plan: &LayoutPlan<'_>, target: &Target<'_>) -> Result<Skeleton> {
    let mut skeleton = Skeleton::new(plan)?;
    skeleton.prefix =
        super::super::backup_ordinary::target_directory(&skeleton.prefix, target.prefix)?;
    skeleton.paths.clear();
    for goal in skeleton.directories.values_mut() {
        goal.to = super::super::backup_ordinary::target_directory(&goal.to, target.prefix)?;
        goal.name = basename(&goal.to).to_owned();
        goal.binding = LayoutBinding::parse(&storage_contract::object_hash(
            "backup-directory-layout-directory",
            &(
                target.slot,
                target.generation,
                target.prefix,
                goal.binding.as_str(),
                goal.to.as_str(),
            ),
        )?)?;
        if skeleton
            .paths
            .insert(name_policy::path_key(goal.to.as_str()), goal.id.clone())
            .is_some()
        {
            return Err(invalid(
                "BACKUP_LAYOUT_DIRECTORY_COLLISION",
                target.slot.into(),
            ));
        }
    }
    // The root remains an independent entity below its fixed native prefix;
    // direct parent IDs for ordinary and retained children are unchanged.
    skeleton.attach_parents()?;
    Ok(skeleton)
}

fn versions<'a>(
    state: &'a State,
    file: &FileLayout<'a>,
    target: &Target<'_>,
) -> Result<Vec<VersionProof<'a>>> {
    let mut selected = Vec::new();
    for version in &file.versions {
        let placement = &state.placements[&version.version_id];
        if !placement
            .route_slot_ids
            .iter()
            .any(|slot| slot == target.slot)
        {
            continue;
        }
        if placement.primary_slot_id == target.slot
            || placement.binding_generations.get(target.slot) != Some(&target.generation)
        {
            return Err(invalid(
                "BACKUP_LAYOUT_ROUTE_CONFLICT",
                version.version_id.clone(),
            ));
        }
        selected.push(VersionProof { version, placement });
    }
    Ok(selected)
}

#[cfg(test)]
#[path = "../../../../../test/engine/unit/container_storage/backup_layout_goals_tests.rs"]
mod tests;
