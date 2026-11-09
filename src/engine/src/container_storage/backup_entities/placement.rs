//! Actual image addresses, not active State placements. One retained file and
//! each initialized directory repository has one home on this complete target.
use super::*;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct EntityPlacement {
    pub(crate) files: Vec<FileHome>,
    pub(crate) directories: Vec<DirectoryHome>,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct FileHome {
    pub(crate) history_tree_id: String,
    pub(crate) file_id: String,
    pub(crate) path: PathBuf,
    pub(crate) directory_id: Option<String>,
    pub(crate) version_ids: BTreeSet<String>,
}
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
pub(crate) struct DirectoryHome {
    pub(crate) history_tree_id: String,
    pub(crate) directory_id: String,
    pub(crate) outer_repository_id: String,
    pub(crate) parent_directory_id: Option<String>,
    pub(crate) path: PathBuf,
}
impl EntityPlacement {
    /// Strip only this target's already-admitted hop prefix. This exposes the
    /// common whole-unit addresses for comparison across siblings, never a
    /// writable path or an active layout locator. Original image bytes stay
    /// unchanged; in particular no sibling is treated as the preceding hop.
    pub(crate) fn relative_to(&self, prefix: &[String]) -> Result<Self> {
        let base = Self::image_base(prefix);
        let relative = |path: &Path| -> Result<PathBuf> {
            let path = path
                .strip_prefix(&base)
                .map_err(|_| invalid("whole entity lies outside its exact target hop prefix"))?;
            if path.as_os_str().is_empty()
                || path
                    .components()
                    .any(|part| !matches!(part, std::path::Component::Normal(_)))
            {
                return Err(invalid("whole entity has no safe history-relative home"));
            }
            Ok(path.to_owned())
        };
        let mut result = self.clone();
        for file in &mut result.files {
            file.path = relative(&file.path)?;
        }
        for directory in &mut result.directories {
            directory.path = relative(&directory.path)?;
        }
        Ok(result)
    }

    pub(crate) fn image_base(prefix: &[String]) -> PathBuf {
        let mut base = Path::new(ROOT).join("tree");
        for name in prefix {
            base.push(name);
        }
        base.join("histories")
    }

    pub(super) fn empty() -> Self {
        Self {
            files: Vec::new(),
            directories: Vec::new(),
        }
    }
    pub(super) fn capture(
        state: &State,
        paths: &Paths,
        root_name: &str,
        stop: &AtomicBool,
    ) -> Result<Self> {
        let mut directories = Vec::with_capacity(state.directory_git_repositories.len());
        for repository in state.directory_git_repositories.values() {
            preparation_checkpoint(stop)?;
            let history = &state.repositories[&repository.repository_id].history_tree_id;
            let path = paths
                .directories
                .get(&(history.clone(), repository.directory_id.clone()))
                .ok_or_else(|| invalid("repository has no whole image placement"))?;
            directories.push(DirectoryHome {
                history_tree_id: history.clone(),
                directory_id: repository.directory_id.clone(),
                outer_repository_id: repository.repository_id.clone(),
                parent_directory_id: None,
                path: Path::new(root_name).join(path),
            });
        }
        // Longest strict path ancestor is the actual repository boundary in the
        // already planned stable main forest, not a prefix of a display label.
        let mut by_path = BTreeMap::new();
        for directory in &directories {
            if by_path
                .insert(directory.path.clone(), directory.directory_id.clone())
                .is_some()
            {
                return Err(invalid("directory repositories share one image home"));
            }
        }
        for directory in &mut directories {
            directory.parent_directory_id = nearest(&by_path, &directory.path, true);
        }
        let mut retained = BTreeMap::<(String, String), BTreeSet<String>>::new();
        for (entries, version) in (directories.len()..).zip(state.versions.values()) {
            preparation_checkpoint(stop)?;
            if entries == MAX_ENTRIES {
                return Err(budget());
            }
            retained
                .entry((version.history_tree_id.clone(), version.file_id.clone()))
                .or_default()
                .insert(version.version_id.clone());
        }
        let mut files = Vec::with_capacity(retained.len());
        for ((history, file), version_ids) in retained {
            preparation_checkpoint(stop)?;
            let path = paths
                .files
                .get(&(history.clone(), file.clone()))
                .ok_or_else(|| invalid("retained file has no whole image placement"))?;
            let path = Path::new(root_name).join(path);
            files.push(FileHome {
                history_tree_id: history,
                file_id: file,
                directory_id: nearest(&by_path, &path, false),
                path,
                version_ids,
            });
        }
        Ok(Self { files, directories })
    }
}
fn nearest(directories: &BTreeMap<PathBuf, String>, path: &Path, strict: bool) -> Option<String> {
    path.ancestors()
        .skip(usize::from(strict))
        .find_map(|parent| directories.get(parent).cloned())
}
