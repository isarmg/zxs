use super::response::Contract;
use json_codec::{Value, json};
use std::collections::BTreeMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Kind {
    Repository,
    Policy,
    Rule,
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum QueryId {
    Repositories,
    Disks,
    IntegrityCheck,
    Repository,
    Operation,
    Plan,
    BranchTree,
    BranchDetails,
    FileHistory,
    HistoryGraph,
    Policy,
    PlacementRule,
    Topology,
}

pub(super) const QUERIES: [(QueryId, QueryKind); 13] = [
    (
        QueryId::Repositories,
        QueryKind::Inspect {
            prefix: "/api/repositories",
            key: None,
            suffix: "",
        },
    ),
    (
        QueryId::Disks,
        QueryKind::Inspect {
            prefix: "/api/admin/disks",
            key: None,
            suffix: "",
        },
    ),
    (
        QueryId::IntegrityCheck,
        QueryKind::Inspect {
            prefix: "/api/admin/check",
            key: None,
            suffix: "",
        },
    ),
    (
        QueryId::Repository,
        QueryKind::Inspect {
            prefix: "/api/repositories",
            key: Some("repository"),
            suffix: "",
        },
    ),
    (QueryId::Operation, QueryKind::Operation),
    (QueryId::Plan, QueryKind::Plan),
    (
        QueryId::BranchTree,
        QueryKind::Inspect {
            prefix: "/api/branches",
            key: Some("id"),
            suffix: "/tree",
        },
    ),
    (
        QueryId::BranchDetails,
        QueryKind::Inspect {
            prefix: "/api/branches",
            key: Some("id"),
            suffix: "",
        },
    ),
    (
        QueryId::FileHistory,
        QueryKind::Inspect {
            prefix: "/api/files",
            key: Some("id"),
            suffix: "/history",
        },
    ),
    (
        QueryId::HistoryGraph,
        QueryKind::Inspect {
            prefix: "/api/history-trees",
            key: Some("id"),
            suffix: "/graph",
        },
    ),
    (
        QueryId::Policy,
        QueryKind::Inspect {
            prefix: "/api/policies",
            key: Some("id"),
            suffix: "",
        },
    ),
    (
        QueryId::PlacementRule,
        QueryKind::Inspect {
            prefix: "/api/primary-placement-rules",
            key: Some("id"),
            suffix: "",
        },
    ),
    (
        QueryId::Topology,
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
pub(super) fn build(kind: Kind, fields: &Fields, operation: &str) -> Result<Request, InputError> {
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
        Kind::Inspect => return Err(InputError::Inspect),
    })
}

pub(super) fn plan_action(
    plan: Option<&super::response::ValidatedPlan>,
    commit: bool,
    acknowledged: bool,
    operation: &str,
) -> Result<Request, InputError> {
    let plan = plan.ok_or(InputError::MissingPlan)?.plan();
    let id = &plan.plan_id;
    if plan.state != storage_contract::PlanState::Ready {
        return Err(InputError::PlanNotReady);
    }
    if commit {
        if !acknowledged {
            return Err(InputError::ReviewRequired);
        }
        let digest = &plan.digest;
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
        path: format!(
            "{prefix}/{}{suffix}",
            storage_contract::client_protocol::uri_component_segment(id)
        ),
        body: None,
        contract,
    })
}

pub(super) fn operation(id: &str) -> Request {
    Request {
        method: "GET",
        path: format!(
            "/api/operations/{}",
            storage_contract::client_protocol::uri_component_segment(id)
        ),
        body: None,
        contract: Contract::Operation { id: id.into() },
    }
}

pub(super) fn operation_outcome(id: &str) -> Request {
    Request {
        method: "GET",
        path: format!(
            "/api/operations/{}/outcome",
            storage_contract::client_protocol::uri_component_segment(id)
        ),
        body: None,
        contract: Contract::OperationOutcome { id: id.into() },
    }
}

#[cfg(test)]
#[path = "../../../../test/web/unit/admin/request_tests.rs"]
mod single_disk_tests;
