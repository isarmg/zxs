//! Exact whole-source selection, without inferring destination paths.
use super::{
    Batch, Escaped, Mapping, Reset, SourceTree, check_mappings, message, no_memory,
    same_name_candidate,
};
use std::{collections::HashMap, ffi::OsStr, io, os::unix::ffi::OsStrExt, path::Path};

impl Batch {
    pub(super) fn prepare_tree(
        reset: &Reset,
        source: &Path,
        target: &Path,
        overrides: &[Mapping],
    ) -> io::Result<Self> {
        let tree = SourceTree::open(&reset.selected, source)?;
        let before = tree.inventory(|| reset.require_original())?;
        before.report(&mut io::stderr().lock())?;
        let rows = resolve(before.files(), overrides)?;
        let batch = Self::prepare_rows(
            reset,
            source,
            target,
            rows.iter()
                .map(|&(path, name, stamp)| (path, name, Some((&tree, stamp)))),
        )?;
        // Never publish a changed or partially scanned selection. This second
        // pass compares all raw names and witnesses, including empty directories.
        if before != tree.inventory(|| reset.require_original())? {
            return Err(message(
                "the selected tree changed while preparing; no complete plan was written",
            ));
        }
        Ok(batch)
    }
}

/// Preserve every file in census order; unused mappings cannot disappear silently.
fn resolve<'a, T>(
    files: impl Iterator<Item = (&'a OsStr, T)>,
    overrides: &'a [Mapping],
) -> io::Result<Vec<(&'a OsStr, &'a OsStr, T)>> {
    check_mappings(
        overrides
            .iter()
            .map(|(path, name)| (path.as_bytes(), name.as_bytes())),
        &[],
    )?;
    let mut names = HashMap::new();
    names.try_reserve(overrides.len()).map_err(no_memory)?;
    for (path, name) in overrides {
        names.insert(path.as_os_str(), name.as_os_str());
    }
    let mut rows = Vec::new();
    for (path, stamp) in files {
        let name = if let Some(name) = names.remove(path) {
            name
        } else if !path.as_bytes().contains(&b'/') && same_name_candidate(path.as_bytes()) {
            path
        } else {
            return Err(message(format!(
                "missing explicit target name for source: {}",
                Escaped(path.as_bytes())
            )));
        };
        rows.try_reserve(1).map_err(no_memory)?;
        rows.push((path, name, stamp));
    }
    if let Some(path) = names.keys().min() {
        return Err(message(format!(
            "mapping does not select an observed regular file: {}",
            Escaped(path.as_bytes())
        )));
    }
    if rows.is_empty() {
        return Err(message(
            "the selected tree contains no regular files to import",
        ));
    }
    Ok(rows)
}

#[cfg(test)]
#[path = "../tests/maintenance/import_batch_tree.rs"]
mod tests;
