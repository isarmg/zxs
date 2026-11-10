use super::*;

#[derive(Debug, error_definition::Error)]
pub(super) enum Error {
    #[error("{error}")]
    Name {
        entry: String,
        #[source]
        error: super::super::naming::Error,
    },
    #[error(transparent)]
    Path(#[from] super::super::naming::Error),
    #[error("{0}")]
    Scope(String),
}
impl From<String> for Error {
    fn from(message: String) -> Self {
        Self::Scope(message)
    }
}

pub(super) fn destination_path(
    path: &str,
) -> name_policy::Result<Option<name_policy::PortablePath>> {
    if path.is_empty() {
        Ok(None)
    } else {
        name_policy::PortablePath::parse(path).map(Some)
    }
}
pub(super) fn destination_endpoint(
    slot: &str,
    prefix: &str,
    path: Option<&name_policy::PortablePath>,
) -> String {
    let directory = path.map(|path| join_path(prefix, path.as_str()));
    format!(
        "/api/file-manager/disks/{}/entries?path={}&limit=1",
        api::segment(slot),
        api::segment(directory.as_deref().unwrap_or(prefix))
    )
}
fn error_missing() -> String {
    t(
        "所选分支中有条目已不存在或依据子树缺失，请取消并刷新后重试。",
        "An entry or its base subtree is missing in a selected branch. Cancel and refresh before retrying.",
    )
}
fn branch_ref(branch: &ScopeBranch, entry: &SelectedFact) -> Value {
    let id = branch.summary().id();
    let tree = branch.summary().head_tree_id();
    let entry_id = entry.entry_id();
    let path = entry.path();
    json!({"directory_branch_id":id,"expected_tree_id":tree,"path":path,"entry_id":entry_id})
}
fn find_entry<'a>(branch: &'a ScopeBranch, chosen: &Entry) -> Result<&'a SelectedFact, String> {
    branch
        .entries()
        .iter()
        .find(|entry| {
            entry.entry_id() == chosen.entry_id() && entry.selected_path() == selected_path(chosen)
        })
        .ok_or_else(error_missing)
}
fn target_ref(branch: &ScopeBranch, path: String) -> Result<Value, Error> {
    name_policy::validate_path(&path)
        .map_err(|source| super::super::naming::Error::new(&path, source))?;
    let id = branch.summary().id();
    let tree = branch.summary().head_tree_id();
    Ok(json!({"directory_branch_id":id,"expected_tree_id":tree,"path":path}))
}
pub(super) fn request(draft: &Draft) -> Result<Value, Error> {
    if !draft.source_resolved || draft.loading {
        return Err(t(
            "请先读取源目录分支。",
            "Load the source folder branches first.",
        )
        .into());
    }
    if draft.selected_sources.is_empty() {
        return Err(t("至少选择一个源分支。", "Select at least one source branch.").into());
    }
    if draft.entries.is_empty() {
        return Err(error_missing().into());
    }
    if draft.kind == Kind::Separate
        && (draft.entries.len() != 1
            || !separable_entry(&draft.entries[0])
            || draft.selected_sources.len() != 1
            || draft.mappings.len() != 1)
    {
        return Err(t(
            "分离时请选择一个保留原件、一个源 main 分支和一个已有目标位置。",
            "Separate one retained original from one existing destination, with one source main branch.",
        )
        .into());
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
            )
            .into());
        }
        for mapping in &draft.mappings {
            if !draft.selected_sources.contains(&mapping.source) {
                return Err(error_missing().into());
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
                validate_name(&entry_key(chosen), name)?;
                let destination = join_path(
                    if draft.kind == Kind::Separate {
                        draft.target_path.trim_matches('/')
                    } else {
                        target.summary().path()
                    },
                    name,
                );
                let key = (
                    mapping.source.clone(),
                    id,
                    mapping.target.clone(),
                    destination.clone(),
                );
                if !unique.insert(key) {
                    return Err(
                        t("存在重复映射，请先移除。", "Remove the duplicate mapping.").into(),
                    );
                }
                items.push(json!({"source":branch_ref(source,entry),"target":target_ref(target,destination)?}));
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
                let mut item = json!({"source":branch_ref(source,entry)});
                if draft.kind == Kind::Rename {
                    let name = draft
                        .names
                        .get(&entry_key(chosen))
                        .ok_or_else(error_missing)?;
                    validate_name(&entry_key(chosen), name)?;
                    let path = entry.path();
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
fn validate_name(entry: &str, name: &str) -> Result<(), Error> {
    super::super::naming::validate(name).map_err(|error| Error::Name {
        entry: entry.into(),
        error,
    })
}
#[cfg(test)]
#[path = "../../../../../test/web/unit/files/batch/request/tests.rs"]
mod tests;
