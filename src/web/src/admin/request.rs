//! Exact HTTP contracts for daily administration. Never builds content transfers.
use super::response::{Contract, PreviewKind};
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

#[derive(Debug)]
pub(super) struct Request {
    pub(super) method: &'static str,
    pub(super) path: String,
    pub(super) body: Option<Value>,
    pub(super) contract: Contract,
}

#[derive(Clone, Copy, Debug)]
pub(super) enum QueryKind {
    Inspect {
        prefix: &'static str,
        key: Option<&'static str>,
        suffix: &'static str,
    },
    Operation,
    Plan,
}

pub(super) const QUERIES: [(&str, QueryKind); 14] = [
    (
        "Repositories",
        QueryKind::Inspect {
            prefix: "/api/repositories",
            key: None,
            suffix: "",
        },
    ),
    (
        "Disks",
        QueryKind::Inspect {
            prefix: "/api/admin/disks",
            key: None,
            suffix: "",
        },
    ),
    (
        "Integrity check",
        QueryKind::Inspect {
            prefix: "/api/admin/check",
            key: None,
            suffix: "",
        },
    ),
    (
        "Repository",
        QueryKind::Inspect {
            prefix: "/api/repositories",
            key: Some("repository"),
            suffix: "",
        },
    ),
    (
        "SMB binding",
        QueryKind::Inspect {
            prefix: "/api/repositories",
            key: Some("repository"),
            suffix: "/smb-binding",
        },
    ),
    ("Operation", QueryKind::Operation),
    ("Plan", QueryKind::Plan),
    (
        "Branch tree",
        QueryKind::Inspect {
            prefix: "/api/branches",
            key: Some("id"),
            suffix: "/tree",
        },
    ),
    (
        "Branch details",
        QueryKind::Inspect {
            prefix: "/api/branches",
            key: Some("id"),
            suffix: "",
        },
    ),
    (
        "File history",
        QueryKind::Inspect {
            prefix: "/api/files",
            key: Some("id"),
            suffix: "/history",
        },
    ),
    (
        "History graph",
        QueryKind::Inspect {
            prefix: "/api/history-trees",
            key: Some("id"),
            suffix: "/graph",
        },
    ),
    (
        "Policy",
        QueryKind::Inspect {
            prefix: "/api/policies",
            key: Some("id"),
            suffix: "",
        },
    ),
    (
        "Placement rule",
        QueryKind::Inspect {
            prefix: "/api/primary-placement-rules",
            key: Some("id"),
            suffix: "",
        },
    ),
    (
        "Topology",
        QueryKind::Inspect {
            prefix: "/api/backup-topologies/default",
            key: None,
            suffix: "",
        },
    ),
];

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
    principal: &str,
) -> Result<Request, InputError> {
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
        Kind::Repository => {
            let name = required("name")?;
            let primary_slot_id = required("slot")?;
            let target_copies = copies()?;
            Request {
                method: "POST",
                path: "/api/repositories".into(),
                body: Some(
                    json!({"name":name,"primary_slot_id":primary_slot_id,"target_copies":target_copies,"operation_id":operation}),
                ),
                contract: Contract::Repository {
                    operation: operation.into(),
                    name: name.into(),
                    primary_slot_id: primary_slot_id.into(),
                    target_copies,
                },
            }
        }
        Kind::Smb => Request {
            method: "PUT",
            path: path("/api/repositories", "repository", "smb-binding")?,
            body: Some(
                json!({"branch_id":required("branch")?,"enabled":value("enabled")=="true","expected_history_generation":generation()?,"operation_id":operation}),
            ),
            contract: Contract::MutationReceipt {
                operation: operation.into(),
            },
        },
        Kind::Policy => Request {
            method: "PUT",
            path: path("/api/policies", "id", "")?,
            body: Some(
                json!({"repository_id":required("repository")?,"path_prefix":value("prefix"),"target_copies":copies()?,"expected_generation":generation()?,"operation_id":operation}),
            ),
            contract: Contract::MutationReceipt {
                operation: operation.into(),
            },
        },
        Kind::Rule => Request {
            method: "PUT",
            path: path("/api/primary-placement-rules", "id", "")?,
            body: Some(
                json!({"repository_id":required("repository")?,"path_prefix":value("prefix"),"primary_slot_id":required("slot")?,"expected_generation":generation()?,"operation_id":operation}),
            ),
            contract: Contract::MutationReceipt {
                operation: operation.into(),
            },
        },
        Kind::Branch => {
            let path = path("/api/repositories", "repository", "branches")?;
            let source_commit_id = required("commit")?;
            let name = required("name")?;
            Request {
                method: "POST",
                path,
                body: Some(
                    json!({"source_commit_id":source_commit_id,"name":name,"operation_id":operation}),
                ),
                contract: Contract::Branch {
                    operation: operation.into(),
                    name: name.into(),
                    source_commit_id: source_commit_id.into(),
                },
            }
        }
        Kind::Link => {
            let history_tree_id = required("history")?;
            let source_branch_id = required("source_branch")?;
            let source_commit_id = required("source_commit")?;
            let target_branch_id = required("target_branch")?;
            let target_commit_id = required("target_commit")?;
            Request {
                method: "POST",
                path: format!(
                    "/api/history-trees/{}/merge-links",
                    segment(history_tree_id)
                ),
                body: Some(
                    json!({"source_branch_id":source_branch_id,"source_commit_id":source_commit_id,"target_branch_id":target_branch_id,"target_commit_id":target_commit_id,"operation_id":operation}),
                ),
                contract: Contract::MergeLink {
                    operation: operation.into(),
                    history_tree_id: history_tree_id.into(),
                    source_branch_id: source_branch_id.into(),
                    source_commit_id: source_commit_id.into(),
                    target_branch_id: target_branch_id.into(),
                    target_commit_id: target_commit_id.into(),
                },
            }
        }
        Kind::Purge => {
            let versions = items(required("versions")?);
            let history_tree_id = required("history")?;
            Request {
                method: "POST",
                path: format!(
                    "/api/history-trees/{}/purge-plans",
                    segment(history_tree_id)
                ),
                body: Some(json!({"version_ids":versions})),
                contract: Contract::Preview {
                    kind: PreviewKind::Purge,
                    history_tree_id: history_tree_id.into(),
                    principal: principal.into(),
                },
            }
        }
        Kind::Split => {
            let history_tree_id = required("history")?;
            Request {
                method: "POST",
                path: format!(
                    "/api/history-trees/{}/split-plans",
                    segment(history_tree_id)
                ),
                body: Some(json!({"cut_commit_id":required("commit")?})),
                contract: Contract::Preview {
                    kind: PreviewKind::Split,
                    history_tree_id: history_tree_id.into(),
                    principal: principal.into(),
                },
            }
        }
        Kind::Save => {
            let history_tree_id = required("history")?;
            Request {
                method: "POST",
                path: format!(
                    "/api/history-trees/{}/historical-save-plans",
                    segment(history_tree_id)
                ),
                body: Some(
                    json!({"source_commit_id":required("commit")?,"name":required("name")?,"workspace_id":if value("workspace").is_empty(){None}else{Some(value("workspace"))},"primary_slot_id":if value("slot").is_empty(){None}else{Some(value("slot"))},"target_copies":if value("copies").is_empty(){None}else{Some(copies()?)}}),
                ),
                contract: Contract::Preview {
                    kind: PreviewKind::HistoricalSave,
                    history_tree_id: history_tree_id.into(),
                    principal: principal.into(),
                },
            }
        }
        Kind::Inspect => return Err(InputError::Inspect),
    })
}

pub(super) fn plan_action(
    plan: &Value,
    commit: bool,
    acknowledged: bool,
    operation: &str,
) -> Result<Request, InputError> {
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
        Ok(Request {
            method: "POST",
            path: format!("/api/plans/{}/commit", segment(id)),
            body: Some(json!({"plan_digest":digest,"operation_id":operation})),
            contract: Contract::PlanReceipt {
                operation: operation.into(),
            },
        })
    } else {
        Ok(Request {
            method: "POST",
            path: format!("/api/plans/{}/cancel", segment(id)),
            body: Some(json!({"operation_id":operation})),
            contract: Contract::PlanReceipt {
                operation: operation.into(),
            },
        })
    }
}

pub(super) fn query(kind: QueryKind, fields: &Fields) -> Result<Request, InputError> {
    let (prefix, key, suffix) = match kind {
        QueryKind::Inspect {
            prefix,
            key: None,
            suffix: _,
        } => {
            return Ok(Request {
                method: "GET",
                path: prefix.into(),
                body: None,
                contract: Contract::Inspect,
            });
        }
        QueryKind::Inspect {
            prefix,
            key: Some(key),
            suffix,
        } => (prefix, key, suffix),
        QueryKind::Operation => ("/api/operations", "id", ""),
        QueryKind::Plan => ("/api/plans", "id", ""),
    };
    let id = fields.get(key).map(String::as_str).unwrap_or_default();
    if id.trim().is_empty() {
        return Err(InputError::Required(key));
    }
    let contract = match kind {
        QueryKind::Inspect { .. } => Contract::Inspect,
        QueryKind::Plan => Contract::Plan { id: id.into() },
        QueryKind::Operation => return Ok(operation(id)),
    };
    Ok(Request {
        method: "GET",
        path: format!("{prefix}/{}{suffix}", crate::api::segment(id)),
        body: None,
        contract,
    })
}

pub(super) fn operation(id: &str) -> Request {
    Request {
        method: "GET",
        path: format!("/api/operations/{}", crate::api::segment(id)),
        body: None,
        contract: Contract::Operation { id: id.into() },
    }
}

#[cfg(test)]
#[path = "../../../../test/web/unit/admin/request_tests.rs"]
mod single_disk_tests;
