//! Source links stay mandatory. Links to excluded, local validation output are
//! reported separately and may be absent in a source-only checkout.
use std::{
    fs, io,
    path::{Component, Path, PathBuf},
};
type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub struct Target {
    pub path: Option<PathBuf>,
    pub generated: bool,
}
fn generated(relative: &Path) -> bool {
    // Archived reports retain legacy paths; only generated output is optional.
    (relative.starts_with("test/evidence") || relative.starts_with("tests/evidence"))
        && !matches!(
            relative.extension().and_then(|s| s.to_str()),
            Some("md" | "rs" | "mjs" | "py" | "sh")
        )
}
fn normalize(path: &Path) -> Result<PathBuf> {
    let mut result = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                if !result.pop() {
                    return Err("link escapes filesystem root".into());
                }
            }
            other => result.push(other.as_os_str()),
        }
    }
    Ok(result)
}
pub fn resolve(root: &Path, document: &Path, relative: &str) -> Result<Target> {
    let raw = if relative.is_empty() {
        document.to_owned()
    } else {
        document
            .parent()
            .ok_or("document parent missing")?
            .join(relative)
    };
    let path = normalize(&raw)?;
    let relative = path
        .strip_prefix(root)
        .map_err(|source| super::ContextError {
            context: format!("link outside repository: {}", path.display()),
            source: source.into(),
        })?;
    let generated = generated(relative);
    match path.canonicalize() {
        Ok(canonical) => {
            if !canonical.starts_with(root) {
                return Err(format!("link outside repository: {}", canonical.display()).into());
            }
            Ok(Target {
                path: Some(canonical),
                generated,
            })
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound && generated => {
            // An excluded output below a link to another repository must not
            // evade the same boundary enforced for available documents.
            for ancestor in path.ancestors() {
                match fs::symlink_metadata(ancestor) {
                    Ok(_) => {
                        if !ancestor.canonicalize()?.starts_with(root) {
                            return Err("generated link outside repository".into());
                        }
                        break;
                    }
                    Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                    Err(error) => return Err(error.into()),
                }
            }
            Ok(Target {
                path: None,
                generated: true,
            })
        }
        Err(source) => Err(super::ContextError {
            context: format!(
                "missing local link {} in {}",
                path.display(),
                document.display()
            ),
            source: source.into(),
        }
        .into()),
    }
}

#[cfg(test)]
#[path = "../../../test/tools/contract-check/unit/document_links_tests.rs"]
mod tests;
