//! Test fixtures use the real container API and current namespace admission.
use super::engine_api as engine;
use std::collections::BTreeSet;
use storage_contract::file_container_tags::{Context, ReadRequest, ReplaceCommand, Tag, TagSet};

pub fn tags(engine: &engine::Engine, file: &str) -> BTreeSet<String> {
    engine
        .state()
        .container_tags
        .get(&file.parse::<storage_contract::FileId>().unwrap())
        .unwrap()
        .tags()
        .iter()
        .map(|tag| tag.as_str().to_owned())
        .collect()
}

pub fn edit(
    engine: &mut engine::Engine,
    file: &str,
    add: &[String],
    remove: &[String],
    principal: &str,
    operation: engine::OperationId,
) -> engine::Result<engine::OperationResult> {
    let context = engine
        .state()
        .repositories
        .values()
        .find_map(|repository| {
            engine
                .namespace_tree(&repository.main_branch_id)
                .ok()
                .and_then(|tree| {
                    tree.visible_entries()
                        .find(|entry| entry.file_id == file)
                        .map(|entry| Context {
                            repository_id: repository.repository_id.parse().unwrap(),
                            path: name_policy::PortablePath::parse(entry.path()).unwrap(),
                        })
                })
        })
        .expect("fixture file has a current visible occurrence");
    let read = ReadRequest {
        file_id: file.parse::<storage_contract::FileId>().unwrap(),
        context,
    };
    let current = engine.file_container_tags(&read)?;
    let mut tags = current
        .tags
        .iter()
        .map(|tag| tag.as_str().to_owned())
        .collect::<BTreeSet<_>>();
    tags.retain(|tag| !remove.contains(tag));
    tags.extend(add.iter().cloned());
    let tags = TagSet::new(tags.into_iter().map(|tag| Tag::parse(tag).unwrap())).unwrap();
    engine.replace_container_tags(
        &ReplaceCommand {
            file_id: read.file_id,
            context: read.context,
            expected_generation: current.generation,
            tags,
        },
        &principal.parse().unwrap(),
        operation,
    )
}
