//! The two current HTTP operation projections emitted by operation_wire.
//! This is not the persisted OperationResult or a local transport-error marker.
use crate::{CommitState, ProtectionAtPublish};
use json_codec::Value;
use std::collections::BTreeSet;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OperationStatus {
    Unknown,
    Published,
    Degraded,
    Rejected,
}

/// Validate the current receipt before interpreting publication evidence. None
/// means malformed, not rejected: callers must retain their original operation.
pub fn operation_status(value: &Value) -> Option<OperationStatus> {
    parse_operation(value, Projection::Outcome)
}

/// Diagnostic responses explicitly carry either a completed health observation
/// or null when no published content scope was inspected.
pub fn operation_observation_status(value: &Value) -> Option<OperationStatus> {
    parse_operation(value, Projection::Observation)
}

#[derive(Clone, Copy)]
enum Projection {
    Outcome,
    Observation,
}

fn parse_operation(value: &Value, projection: Projection) -> Option<OperationStatus> {
    let content = value["operation_kind"] == "content_save";
    let fields = if content {
        &[
            "operation_id",
            "operation_kind",
            "request_digest",
            "state",
            "commit_state",
            "result",
            "error",
        ][..]
    } else {
        &[
            "operation_id",
            "operation_kind",
            "request_digest",
            "state",
            "commit_state",
            "result",
            "error",
            "protection_at_publish",
            "publication_sequence",
        ][..]
    };
    let shape = match projection {
        Projection::Outcome => keys(value, fields),
        Projection::Observation => value.as_object().is_some_and(|object| {
            object.len() == fields.len() + 1
                && fields.iter().all(|field| object.contains_key(*field))
                && object.get("current_health").is_some_and(|health| {
                    health.is_null()
                        || (keys(health, &["state", "checked_at"])
                            && strings(health, &["checked_at"])
                            && matches!(health["state"].as_str(),
                                Some("HEALTHY" | "DEGRADED" | "UNAVAILABLE" | "UNKNOWN")))
                })
        }),
    };
    if !shape
        || !strings(value, &["operation_id", "operation_kind"])
        || !value["request_digest"].as_str().is_some_and(|digest| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        })

    {
        return None;
    }
    let commit = json_codec::from_value::<CommitState>(value["commit_state"].clone()).ok()?;
    if content {
        content_status(value, commit)
    } else {
        let protection =
            json_codec::from_value::<ProtectionAtPublish>(value["protection_at_publish"].clone())
                .ok()?;
        let expected_state = if matches!(commit, CommitState::Published | CommitState::Acked) {
            "completed"
        } else {
            "in_doubt"
        };
        if value["state"] != expected_state
            || !decimals(value, &["publication_sequence"])
            || !value["result"].is_object()
            || !value["error"].is_null()
        {
            return None;
        }
        Some(match commit {
            CommitState::Published | CommitState::Acked => match protection {
                ProtectionAtPublish::NotApplicable | ProtectionAtPublish::TargetMet => {
                    OperationStatus::Published
                }
                ProtectionAtPublish::BelowTarget => OperationStatus::Degraded,
                ProtectionAtPublish::Unconfirmed => return None,
            },
            CommitState::PublishedDegraded if protection == ProtectionAtPublish::BelowTarget => {
                OperationStatus::Degraded
            }
            CommitState::PublishedDegraded => return None,
            CommitState::Aborted => OperationStatus::Rejected,
            _ => OperationStatus::Unknown,
        })
    }
}

fn content_status(value: &Value, commit: CommitState) -> Option<OperationStatus> {
    match commit {
        CommitState::Published | CommitState::Acked | CommitState::PublishedDegraded => {
            if value["state"] != "completed"
                || !value["error"].is_null()
                || !save_result(&value["result"])
            {
                return None;
            }
            let degraded = value["result"]["protection_at_publish"] == "BELOW_TARGET";
            if commit == CommitState::PublishedDegraded && !degraded {
                return None;
            }
            Some(if degraded {
                OperationStatus::Degraded
            } else {
                OperationStatus::Published
            })
        }
        _ => {
            if !value["result"].is_null() {
                return None;
            }
            if commit == CommitState::Aborted {
                return (value["state"] == "failed" && error(&value["error"]))
                    .then_some(OperationStatus::Rejected);
            }
            let expected = if matches!(commit, CommitState::InDoubt | CommitState::RecoveryRequired)
            {
                "in_doubt"
            } else {
                "processing"
            };
            if value["state"] != expected || !value["error"].is_null() {
                return None;
            }
            Some(OperationStatus::Unknown)
        }
    }
}

fn save_result(value: &Value) -> bool {
    keys(
        value,
        &[
            "outcome",
            "commit_id",
            "branch_id",
            "branch_generation",
            "publication_sequence",
            "membership_epoch",
            "protection_at_publish",
            "versions",
        ],
    ) && strings(value, &["commit_id", "branch_id"])
        && decimals(
            value,
            &[
                "branch_generation",
                "publication_sequence",
                "membership_epoch",
            ],
        )
        && matches!(
            (
                value["outcome"].as_str(),
                value["protection_at_publish"].as_str()
            ),
            (Some("COMMITTED_AND_PROTECTED"), Some("TARGET_MET"))
                | (Some("COMMITTED_DEGRADED"), Some("BELOW_TARGET"))
        )
        && value["versions"].as_array().is_some_and(|versions| {
            !versions.is_empty()
                && versions.iter().all(version_protection)
                && versions
                    .iter()
                    .filter_map(|version| version["version_id"].as_str())
                    .collect::<BTreeSet<_>>()
                    .len()
                    == versions.len()
                && versions.iter().all(|version| {
                    version["slots"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .all(|slot| slot["state"] == "COMPLETE_VERIFIED")
                }) == (value["outcome"] == "COMMITTED_AND_PROTECTED")
        })
}

fn version_protection(value: &Value) -> bool {
    keys(
        value,
        &[
            "version_id",
            "primary_slot_id",
            "target_copies",
            "required_slots",
            "topology_generation",
            "slots",
        ],
    ) && strings(value, &["version_id", "primary_slot_id"])
        && value["target_copies"]
            .as_u64()
            .is_some_and(|copies| copies > 0)
        && decimals(value, &["topology_generation"])
        && value["required_slots"].as_array().is_some_and(|slots| {
            !slots.is_empty()
                && slots.iter().all(nonempty)
                && slots
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<BTreeSet<_>>()
                    .len()
                    == slots.len()
        })
        && value["slots"].as_array().is_some_and(|slots| {
            !slots.is_empty()
                && slots.iter().all(|slot| {
                    keys(
                        slot,
                        &[
                            "slot_id",
                            "copy_level",
                            "disk_id",
                            "binding_generation",
                            "state",
                        ],
                    ) && strings(slot, &["slot_id", "disk_id"])
                        && slot["copy_level"].as_u64().is_some_and(|level| level > 0)
                        && decimals(slot, &["binding_generation"])
                        && matches!(
                            slot["state"].as_str(),
                            Some("COMPLETE_VERIFIED" | "UNAVAILABLE")
                        )
                })
        })
        && {
            let route = value["required_slots"].as_array().unwrap();
            let slots = value["slots"].as_array().unwrap();
            route.len() as u64 == value["target_copies"].as_u64().unwrap()
                && route.len() == slots.len()
                && route.first() == Some(&value["primary_slot_id"])
                && route
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_ascii_lowercase)
                    .collect::<BTreeSet<_>>()
                    .len()
                    == route.len()
                && slots
                    .iter()
                    .filter_map(|slot| slot["disk_id"].as_str())
                    .collect::<BTreeSet<_>>()
                    .len()
                    == slots.len()
                && slots.iter().enumerate().all(|(index, slot)| {
                    slot["slot_id"] == route[index]
                        && slot["copy_level"].as_u64() == Some(index as u64 + 1)
                })
        }
}

fn error(value: &Value) -> bool {
    keys(value, &["code", "message", "retryable", "request_id"])
        && strings(value, &["code", "request_id"])
        && value["message"].is_string()
        && value["retryable"].is_boolean()
}

fn keys(value: &Value, fields: &[&str]) -> bool {
    value.as_object().is_some_and(|object| {
        object.len() == fields.len() && fields.iter().all(|field| object.contains_key(*field))
    })
}
fn nonempty(value: &Value) -> bool {
    value.as_str().is_some_and(|value| !value.is_empty())
}
fn strings(value: &Value, fields: &[&str]) -> bool {
    fields.iter().all(|field| nonempty(&value[*field]))
}
fn decimals(value: &Value, fields: &[&str]) -> bool {
    fields.iter().all(|field| {
        value[*field]
            .as_str()
            .is_some_and(|value| crate::parse_u64_decimal(value).is_ok())
    })
}

#[cfg(test)]
#[path = "../../../../test/contract/unit/client_protocol/operation_tests.rs"]
mod tests;
