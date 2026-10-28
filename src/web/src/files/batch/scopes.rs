//! The selection-facts response has its own boundary, separate from a disk page.
use super::*;
use crate::files::page::DirectoryBranch;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct ScopeBranch {
    summary: DirectoryBranch,
    entries: Vec<SelectedFact>,
}
impl ScopeBranch {
    pub(super) fn summary(&self) -> &DirectoryBranch {
        &self.summary
    }
    pub(super) fn entries(&self) -> &[SelectedFact] {
        &self.entries
    }
}
impl From<DirectoryBranch> for ScopeBranch {
    fn from(summary: DirectoryBranch) -> Self {
        Self {
            summary,
            entries: Vec::new(),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct SelectedFact {
    entry_id: String,
    path: name_policy::LookupPath,
    selected_path: name_policy::LookupPath,
}
impl SelectedFact {
    pub(super) fn entry_id(&self) -> &str {
        &self.entry_id
    }
    pub(super) fn path(&self) -> &str {
        self.path.as_str()
    }
    pub(super) fn selected_path(&self) -> &str {
        self.selected_path.as_str()
    }
}
#[derive(Clone, Copy)]
pub(super) enum Projection {
    Summary,
    Selected,
}
pub(super) struct Scopes {
    pub(super) repository: String,
    pub(super) branches: Vec<ScopeBranch>,
}
impl Scopes {
    pub(super) fn parse(value: Value, projection: Projection) -> Result<Self, api::Error> {
        let Value::Object(mut object) = value else {
            return Err(invalid("scope", "expected an object"));
        };
        let repository = string(object.remove("repository_id"), "repository_id")?;
        if repository.len() > 256 {
            return Err(invalid("repository_id", "an identity of at most 256 bytes"));
        }
        let Some(Value::Array(branches)) = object.remove("directory_branches") else {
            return Err(invalid("directory_branches", "expected an array"));
        };
        if !object.is_empty() {
            return Err(invalid("scope", "unknown field"));
        }
        let mut ids = BTreeSet::new();
        let mut parsed = Vec::with_capacity(branches.len());
        for branch in branches {
            let Value::Object(mut object) = branch else {
                return Err(invalid("directory_branches", "expected branch objects"));
            };
            let entries = match projection {
                Projection::Summary => Vec::new(),
                Projection::Selected => {
                    let Some(Value::Array(entries)) = object.remove("entries") else {
                        return Err(invalid("entries", "expected selected facts"));
                    };
                    selected_facts(entries)?
                }
            };
            let summary = DirectoryBranch::parse(Value::Object(object))?;
            if summary.repository_id() != repository || !ids.insert(summary.id().to_owned()) {
                return Err(invalid(
                    "directory_branches",
                    "repository or unique identity mismatch",
                ));
            }
            parsed.push(ScopeBranch { summary, entries });
        }
        Ok(Self {
            repository,
            branches: parsed,
        })
    }
}
fn selected_facts(values: Vec<Value>) -> Result<Vec<SelectedFact>, api::Error> {
    if values.len() > 500 {
        return Err(invalid("entries", "selection exceeds 500 entries"));
    }
    let mut identities = BTreeSet::new();
    values
        .into_iter()
        .map(|value| {
            let Value::Object(mut object) = value else {
                return Err(invalid("entries", "expected selected fact objects"));
            };
            let entry_id = string(object.remove("entry_id"), "entry_id")?;
            let path = string(object.remove("path"), "path")?;
            let selected_path = string(object.remove("selected_path"), "selected_path")?;
            if !object.is_empty() || entry_id.len() > 256 {
                return Err(invalid("entries", "unknown field or overlong identity"));
            }
            let parse_path = |path: &str| {
                name_policy::LookupPath::parse(path).map_err(|source| {
                    crate::files::DirectoryPageError::Path {
                        field: "selected fact path",
                        source,
                    }
                })
            };
            let path = parse_path(&path)?;
            let selected_path = parse_path(&selected_path)?;
            if !identities.insert((entry_id.clone(), selected_path.key().to_owned())) {
                return Err(invalid("entries", "duplicate selected identity"));
            }
            Ok(SelectedFact {
                entry_id,
                path,
                selected_path,
            })
        })
        .collect()
}
fn string(value: Option<Value>, field: &'static str) -> Result<String, api::Error> {
    match value {
        Some(Value::String(value)) if !value.is_empty() => Ok(value),
        _ => Err(invalid(field, "expected a nonempty string")),
    }
}
fn invalid(field: &'static str, reason: &'static str) -> api::Error {
    crate::files::DirectoryPageError::Field {
        field,
        expected: reason,
    }
    .into()
}
