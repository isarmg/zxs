//! Allocation is a plan over the caller's fenced complete parent namespace.
//! The publisher reserves the result durably and uses no-replace installation.
use name_policy::{LookupName, Name};
use storage_contract::BranchName;

const FIRST: u16 = 1;
const LAST: u16 = 999;

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct WorktreeName {
    name: Name,
    ordinal: u16,
}
impl WorktreeName {
    pub(crate) fn name(&self) -> &Name {
        &self.name
    }
    pub(crate) fn ordinal(&self) -> u16 {
        self.ordinal
    }
}

#[derive(Debug, error_definition::Error)]
pub(crate) enum NameError {
    #[error("all 999 worktree names for this branch are occupied or reserved")]
    Exhausted,
    #[error("worktree name exceeds the target filesystem component limit")]
    TooLong,
    #[error("worktree name is invalid: {0}")]
    Invalid(#[source] name_policy::Error),
}

/// Include ordinary entries, live worktrees and in-flight durable reservations.
/// Soft-deleted instances do not occupy a visible name, but keep their stable ID.
/// Their restoration must obtain a new free claim rather than reuse an occupant.
pub(crate) fn allocate(
    branch: &BranchName,
    occupied: impl IntoIterator<Item = LookupName>,
    component_limit: usize,
) -> Result<WorktreeName, NameError> {
    let prefix = format!("worktree-{}-", branch.as_str());
    if prefix.len() + 3 > component_limit {
        return Err(NameError::TooLong);
    }
    let prefix_key = name_policy::lookup_key(&prefix);
    let mut used = [false; LAST as usize];
    for name in occupied {
        let key = name.key();
        let Some(number) = key.strip_prefix(&prefix_key) else {
            continue;
        };
        if let Some(ordinal) = ordinal(number) {
            used[usize::from(ordinal - FIRST)] = true;
        }
    }
    for ordinal in FIRST..=LAST {
        if !used[usize::from(ordinal - FIRST)] {
            let name = Name::parse(&format!("{prefix}{ordinal:03}"))
                .map_err(NameError::Invalid)?;
            return Ok(WorktreeName { name, ordinal });
        }
    }
    Err(NameError::Exhausted)
}

/// Replay proves the reserved spelling belongs to its admitted source branch.
/// It cannot recompute the lowest free name from a later ordinary namespace.
pub(crate) fn is_allocated_name(branch: &BranchName, name: &Name) -> bool {
    let prefix = format!("worktree-{}-", branch.as_str());
    name.as_str().strip_prefix(&prefix).and_then(ordinal).is_some()
}

fn ordinal(number: &str) -> Option<u16> {
    if number.len() != 3 || !number.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    // Three ASCII digits are bounded below 1000 without parsing or overflow.
    let bytes = number.as_bytes();
    let ordinal = u16::from(bytes[0] - b'0') * 100
        + u16::from(bytes[1] - b'0') * 10
        + u16::from(bytes[2] - b'0');
    (ordinal >= FIRST).then_some(ordinal)
}

#[cfg(test)]
#[path = "../../../../test/engine/unit/directory_worktree/names.rs"]
mod tests;
