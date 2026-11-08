//! Actual image addresses, not active State placements. One retained file and
//! each initialized directory repository has one home on this complete target.
use super::*;

#[derive(Clone, Serialize)]
pub(crate) struct EntityPlacement {
    files: Vec<FileHome>,
    directories: Vec<DirectoryHome>,
}
#[derive(Clone, Serialize)]
struct FileHome {
    history_tree_id: String,
    file_id: String,
    path: PathBuf,
    directory_id: Option<String>,
    version_ids: BTreeSet<String>,
}
#[derive(Clone, Serialize)]
struct DirectoryHome {
    history_tree_id: String,
    directory_id: String,
    outer_repository_id: String,
    parent_directory_id: Option<String>,
    path: PathBuf,
}
impl EntityPlacement {
    pub(super) fn empty() -> Self {
        Self {
            files: Vec::new(),
            directories: Vec::new(),
        }
    }
    pub(super) fn capture(state: &State, paths: &Paths, stop: &AtomicBool) -> Result<Self> {
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
                path: Path::new(ROOT).join(path),
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
        let mut entries = directories.len();
        for version in state.versions.values() {
            preparation_checkpoint(stop)?;
            if entries == MAX_ENTRIES {
                return Err(budget());
            }
            entries += 1;
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
            let path = Path::new(ROOT).join(path);
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
