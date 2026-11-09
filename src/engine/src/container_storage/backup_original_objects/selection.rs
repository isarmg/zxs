//! The original frozen routes choose the source scopes, including retained
//! terminal FILEs. New target topology never elects replacement source entities.
use super::super::layout_goals;
use super::*;
use std::collections::BTreeSet;

const MAX_OBJECTS: usize = 4096;

pub(super) fn capture(
    root: &DiskRoot,
    slot: &str,
    original: &crate::State,
    stop: &AtomicBool,
) -> Result<BTreeMap<Key, Entity>> {
    let mut scopes = BTreeMap::new();
    for version in original.versions.values() {
        preparation_checkpoint(stop)?;
        if original.placements[&version.version_id]
            .route_slot_ids
            .iter()
            .any(|route| route == slot)
        {
            add(
                &mut scopes,
                backup_databases::Scope::File {
                    history: version.history_tree_id.clone(),
                    file: version.file_id.clone(),
                },
            )?;
        }
    }
    // Use the normal planner's actual primary forest coverage, not every
    // directory Git record on every original disk. Empty initialized forests
    // are included even when they contain no routed file versions.
    for history in layout_goals::build(original, None)?.histories.values() {
        preparation_checkpoint(stop)?;
        if history.slot_id != slot {
            continue;
        }
        for directory in history.directories.keys() {
            if let Some(git) = original.directory_git_repositories.get(directory.as_str()) {
                add(
                    &mut scopes,
                    backup_databases::Scope::Directory {
                        history: history.history_id.clone(),
                        repository: git.repository_id.clone(),
                        directory: directory.as_str().to_owned(),
                    },
                )?;
            }
        }
    }
    let mut entities = BTreeMap::new();
    let mut physical = BTreeSet::new();
    for (key, scope) in scopes {
        let entity = Entity::capture(root, scope, stop)?;
        if !physical.insert((entity.inode.device, entity.inode.inode)) {
            return Err(invalid(
                "one original ordinary entity has multiple database owners",
            ));
        }
        entities.insert(key, entity);
    }
    Ok(entities)
}
fn add(
    scopes: &mut BTreeMap<Key, backup_databases::Scope>,
    scope: backup_databases::Scope,
) -> Result<()> {
    let key = Key::of(&scope);
    if let Some(previous) = scopes.get(&key) {
        if previous != &scope {
            return Err(invalid("original ordinary object has conflicting scopes"));
        }
        return Ok(());
    }
    if scopes.len() == MAX_OBJECTS {
        return Err(budget());
    }
    scopes.insert(key, scope);
    Ok(())
}
