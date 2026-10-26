//! Pure scope validation and transaction payload construction.
use super::*;
pub(super) fn valid_path(path: &str) -> bool {
    path.is_empty() || name_policy::validate_path(path).is_ok()
}
fn error_missing() -> String {
    t(
        "所选分支中有条目已不存在或依据子树缺失，请取消并刷新后重试。",
        "An entry or its base subtree is missing in a selected branch. Cancel and refresh before retrying.",
    )
}
fn branch_ref(branch: &Value, entry: &Value) -> Result<Value, String> {
    let id = branch_id(branch);
    let tree = text(branch, &["head_tree_id"]);
    let entry_id = entry_id(entry);
    let path = text(entry, &["path"]);
    if id.is_empty() || tree.is_empty() || entry_id.is_empty() || path.is_empty() {
        return Err(error_missing());
    }
    Ok(json!({"directory_branch_id":id,"expected_tree_id":tree,"path":path,"entry_id":entry_id}))
}
fn find_entry<'a>(branch: &'a Value, chosen: &Value) -> Result<&'a Value, String> {
    let id = entry_id(chosen);
    let logical_path = selected_path(chosen);
    branch["entries"]
        .as_array()
        .and_then(|entries| {
            let mut matches = entries
                .iter()
                .filter(|entry| entry["entry_id"] == id && entry["selected_path"] == logical_path);
            let first = matches.next()?;
            matches.next().is_none().then_some(first)
        })
        .ok_or_else(error_missing)
}
fn target_ref(branch: &Value, path: String) -> Result<Value, String> {
    if name_policy::validate_path(&path).is_err() {
        return Err(t(
            "目标路径不符合共享命名规则。",
            "The destination path does not meet the shared naming rules.",
        ));
    }
    let id = branch_id(branch);
    let tree = text(branch, &["head_tree_id"]);
    if id.is_empty() || tree.is_empty() {
        return Err(error_missing());
    }
    Ok(json!({"directory_branch_id":id,"expected_tree_id":tree,"path":path}))
}
/// Scope and identity are captured before submission. Never infer all possible
/// source/target pairs or silently skip an entry absent in another branch.
pub(super) fn request(draft: &Draft) -> Result<Value, String> {
    if !draft.source_resolved || draft.loading {
        return Err(t(
            "请先读取源目录分支。",
            "Load the source folder branches first.",
        ));
    }
    if draft.selected_sources.is_empty() {
        return Err(t(
            "至少选择一个源分支。",
            "Select at least one source branch.",
        ));
    }
    if draft.entries.is_empty() {
        return Err(error_missing());
    }
    let mut items = vec![];
    let mut unique = BTreeSet::new();
    if draft.kind.has_destination() {
        if draft.mappings.is_empty()
            || draft.selected_sources.iter().any(|source| {
                !draft
                    .mappings
                    .iter()
                    .any(|mapping| &mapping.source == source)
            })
        {
            return Err(t(
                "请为每个源分支指定目标映射。",
                "Specify a destination mapping for every source branch.",
            ));
        }
        for mapping in &draft.mappings {
            if !draft.selected_sources.contains(&mapping.source) {
                return Err(error_missing());
            }
            let source = draft
                .sources
                .iter()
                .find(|branch| branch_id(branch) == mapping.source)
                .ok_or_else(error_missing)?;
            let target = draft
                .targets
                .iter()
                .find(|branch| branch_id(branch) == mapping.target)
                .ok_or_else(|| {
                    t(
                        "请明确选择每条映射的目标分支。",
                        "Explicitly select the destination branch for each mapping.",
                    )
                })?;
            for chosen in &draft.entries {
                let id = entry_id(chosen);
                let entry = find_entry(source, chosen)?;
                let name = draft
                    .names
                    .get(&entry_key(chosen))
                    .ok_or_else(error_missing)?;
                validate_name(name)?;
                let destination = join_path(&text(target, &["path"]), name);
                let key = (
                    mapping.source.clone(),
                    id,
                    mapping.target.clone(),
                    destination.clone(),
                );
                if !unique.insert(key) {
                    return Err(t(
                        "存在重复映射，请先移除。",
                        "Remove the duplicate mapping.",
                    ));
                }
                items.push(json!({"source":branch_ref(source,entry)?,"target":target_ref(target,destination)?}));
            }
        }
    } else {
        for source_id in &draft.selected_sources {
            let source = draft
                .sources
                .iter()
                .find(|branch| branch_id(branch) == *source_id)
                .ok_or_else(error_missing)?;
            for chosen in &draft.entries {
                let entry = find_entry(source, chosen)?;
                let mut item = json!({"source":branch_ref(source,entry)?});
                if draft.kind == Kind::Rename {
                    let name = draft
                        .names
                        .get(&entry_key(chosen))
                        .ok_or_else(error_missing)?;
                    validate_name(name)?;
                    let path = text(entry, &["path"]);
                    let parent = path
                        .rsplit_once('/')
                        .map(|(parent, _)| parent)
                        .unwrap_or_default();
                    item["target"] = target_ref(source, join_path(parent, name))?;
                }
                items.push(item);
            }
        }
    }
    Ok(json!({"kind":draft.kind.contract_kind(),"items":items}))
}
fn validate_name(name: &str) -> Result<(), String> {
    if valid_name(name) {
        Ok(())
    } else {
        Err(t(
            "名称不符合共享文件命名规则。",
            "The name does not meet the shared file naming rules.",
        ))
    }
}
#[cfg(test)]
#[path = "../../../../../test/web/unit/files/batch/request/tests.rs"]
mod tests;
