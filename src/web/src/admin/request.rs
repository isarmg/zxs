//! Exact HTTP contracts for daily administration. Never builds content transfers.
use json_codec::{Value, json};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Repository,
    Smb,
    Policy,
    Rule,
    Branch,
    Link,
    Purge,
    Split,
    Save,
    Inspect,
}

#[derive(Debug, error_definition::Error)]
pub(super) enum InputError {
    #[error("{0} is required")]
    Required(&'static str),
    #[error("copies must be a positive integer")]
    Copies(#[source] std::num::ParseIntError),
    #[error("copies must be a positive integer")]
    ZeroCopies,
    #[error("{0}")]
    Generation(#[source] storage_contract::Error),
    #[error("Select a management action first")]
    Inspect,
    #[error("Load a plan first")]
    MissingPlan,
    #[error("Only a ready plan may be committed or cancelled")]
    PlanNotReady,
    #[error("Review the full plan and acknowledge its consequences first")]
    ReviewRequired,
    #[error("Plan digest missing")]
    MissingDigest,
}

fn segment(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || b"-._~".contains(&b) {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
pub(super) type Fields = BTreeMap<String, String>;
pub(super) fn initial_fields(kind: Kind) -> Fields {
    if matches!(kind, Kind::Repository | Kind::Policy) {
        [("copies".to_owned(), "1".to_owned())]
            .into_iter()
            .collect()
    } else {
        Fields::new()
    }
}
pub(super) fn items(value: &str) -> Vec<String> {
    value
        .split(|c: char| c == ',' || c.is_whitespace())
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}
pub(super) fn build(
    kind: Kind,
    fields: &Fields,
    operation: &str,
) -> Result<(&'static str, String, Value), InputError> {
    let value = |key: &str| fields.get(key).map(String::as_str).unwrap_or_default();
    let required = |key: &'static str| {
        let v = value(key);
        if v.trim().is_empty() {
            Err(InputError::Required(key))
        } else {
            Ok(v)
        }
    };
    let copies = || {
        let copies = required("copies")?
            .parse::<u32>()
            .map_err(InputError::Copies)?;
        if copies == 0 {
            Err(InputError::ZeroCopies)
        } else {
            Ok(copies)
        }
    };
    let generation = || {
        let n = required("generation")?;
        storage_contract::parse_u64_decimal(n).map_err(InputError::Generation)?;
        Ok::<_, InputError>(n)
    };
    let path = |prefix: &str, key: &'static str, suffix: &str| {
        Ok::<_, InputError>(
            format!("{prefix}/{}/{suffix}", segment(required(key)?))
                .trim_end_matches('/')
                .to_string(),
        )
    };
    Ok(match kind {
        Kind::Repository => (
            "POST",
            "/api/repositories".into(),
            json!({"name":required("name")?,"primary_slot_id":required("slot")?,"target_copies":copies()?,"operation_id":operation}),
        ),
        Kind::Smb => (
            "PUT",
            path("/api/repositories", "repository", "smb-binding")?,
            json!({"branch_id":required("branch")?,"enabled":value("enabled")=="true","expected_history_generation":generation()?,"operation_id":operation}),
        ),
        Kind::Policy => (
            "PUT",
            path("/api/policies", "id", "")?,
            json!({"repository_id":required("repository")?,"path_prefix":value("prefix"),"target_copies":copies()?,"expected_generation":generation()?,"operation_id":operation}),
        ),
        Kind::Rule => (
            "PUT",
            path("/api/primary-placement-rules", "id", "")?,
            json!({"repository_id":required("repository")?,"path_prefix":value("prefix"),"primary_slot_id":required("slot")?,"expected_generation":generation()?,"operation_id":operation}),
        ),
        Kind::Branch => (
            "POST",
            path("/api/repositories", "repository", "branches")?,
            json!({"source_commit_id":required("commit")?,"name":required("name")?,"operation_id":operation}),
        ),
        Kind::Link => (
            "POST",
            path("/api/history-trees", "history", "merge-links")?,
            json!({"source_branch_id":required("source_branch")?,"source_commit_id":required("source_commit")?,"target_branch_id":required("target_branch")?,"target_commit_id":required("target_commit")?,"operation_id":operation}),
        ),
        Kind::Purge => {
            let versions = items(required("versions")?);
            (
                "POST",
                path("/api/history-trees", "history", "purge-plans")?,
                json!({"version_ids":versions}),
            )
        }
        Kind::Split => (
            "POST",
            path("/api/history-trees", "history", "split-plans")?,
            json!({"cut_commit_id":required("commit")?}),
        ),
        Kind::Save => (
            "POST",
            path("/api/history-trees", "history", "historical-save-plans")?,
            json!({"source_commit_id":required("commit")?,"name":required("name")?,"workspace_id":if value("workspace").is_empty(){None}else{Some(value("workspace"))},"primary_slot_id":if value("slot").is_empty(){None}else{Some(value("slot"))},"target_copies":if value("copies").is_empty(){None}else{Some(copies()?)}}),
        ),
        Kind::Inspect => return Err(InputError::Inspect),
    })
}
pub(super) fn plan_action(
    plan: &Value,
    commit: bool,
    acknowledged: bool,
    operation: &str,
) -> Result<(&'static str, String, Value), InputError> {
    let id = plan["plan_id"]
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or(InputError::MissingPlan)?;
    if plan["state"] != "ready" {
        return Err(InputError::PlanNotReady);
    }
    if commit {
        if !acknowledged {
            return Err(InputError::ReviewRequired);
        }
        let digest = plan["digest"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or(InputError::MissingDigest)?;
        Ok((
            "POST",
            format!("/api/plans/{}/commit", segment(id)),
            json!({"plan_digest":digest,"operation_id":operation}),
        ))
    } else {
        Ok((
            "POST",
            format!("/api/plans/{}/cancel", segment(id)),
            json!({"operation_id":operation}),
        ))
    }
}

#[cfg(test)]
#[path = "../../../../test/web/unit/admin/request_tests.rs"]
mod single_disk_tests;
