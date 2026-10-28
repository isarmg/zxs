use super::*;
#[test]
fn new_repository_defaults_to_one_actual_copy_and_preserves_explicit_choices() {
    let mut fields = initial_fields(Kind::Repository);
    fields.insert("name".into(), "one disk".into());
    fields.insert("slot".into(), "P01".into());
    let request = build(Kind::Repository, &fields, "operation", "admin").unwrap();
    let body = request.body.unwrap();
    assert_eq!(request.path, "/api/repositories");
    assert_eq!(body["target_copies"], 1);
    fields.insert("copies".into(), "3".into());
    assert_eq!(
        build(Kind::Repository, &fields, "operation", "admin")
            .unwrap()
            .body
            .unwrap()["target_copies"],
        3
    );
    fields.insert("copies".into(), "0".into());
    assert!(build(Kind::Repository, &fields, "operation", "admin").is_err());
    assert_eq!(initial_fields(Kind::Policy)["copies"], "1");
    assert!(
        initial_fields(Kind::Save).is_empty(),
        "historical saves retain inherited policy unless explicitly overridden"
    );
}

#[test]
fn inspection_cannot_build_a_mutation_request() {
    assert!(build(Kind::Inspect, &Fields::new(), "operation", "admin").is_err());
}

#[test]
fn invalid_numeric_fields_retain_their_parser_source() {
    use std::error::Error as _;
    let fields = [
        ("name".into(), "repo".into()),
        ("slot".into(), "P01".into()),
        ("copies".into(), "NaN".into()),
    ]
    .into_iter()
    .collect();
    assert!(
        build(Kind::Repository, &fields, "operation", "admin")
            .unwrap_err()
            .source()
            .is_some()
    );
    let fields = [
        ("repository".into(), "repo".into()),
        ("branch".into(), "b".into()),
        ("generation".into(), "18446744073709551616".into()),
    ]
    .into_iter()
    .collect();
    assert!(
        build(Kind::Smb, &fields, "operation", "admin")
            .unwrap_err()
            .source()
            .is_some()
    );
}

fn all_fields() -> Fields {
    [
        ("name", "Name"),
        ("slot", "P01"),
        ("copies", "2"),
        ("repository", "repo/!"),
        ("branch", "branch"),
        ("enabled", "true"),
        ("generation", "7"),
        ("id", "id/!"),
        ("prefix", "folder"),
        ("history", "history/!"),
        ("commit", "source"),
        ("source_branch", "sb"),
        ("source_commit", "sc"),
        ("target_branch", "tb"),
        ("target_commit", "tc"),
        ("versions", "a, b\nc"),
        ("workspace", "workspace"),
    ]
    .into_iter()
    .map(|(key, value)| (key.into(), value.into()))
    .collect()
}

#[test]
fn each_form_constructs_the_exact_request_and_response_context_together() {
    let fields = all_fields();
    let operation = "original";
    for (kind, method, path, body, contract) in [
        (
            Kind::Repository,
            "POST",
            "/api/repositories",
            json!({"name":"Name","primary_slot_id":"P01","target_copies":2,"operation_id":operation}),
            Contract::Repository {
                operation: operation.into(),
                name: "Name".into(),
                primary_slot_id: "P01".into(),
                target_copies: 2,
            },
        ),
        (
            Kind::Smb,
            "PUT",
            "/api/repositories/repo%2F%21/smb-binding",
            json!({"branch_id":"branch","enabled":true,"expected_history_generation":"7","operation_id":operation}),
            Contract::MutationReceipt {
                operation: operation.into(),
            },
        ),
        (
            Kind::Policy,
            "PUT",
            "/api/policies/id%2F%21",
            json!({"repository_id":"repo/!","path_prefix":"folder","target_copies":2,"expected_generation":"7","operation_id":operation}),
            Contract::MutationReceipt {
                operation: operation.into(),
            },
        ),
        (
            Kind::Rule,
            "PUT",
            "/api/primary-placement-rules/id%2F%21",
            json!({"repository_id":"repo/!","path_prefix":"folder","primary_slot_id":"P01","expected_generation":"7","operation_id":operation}),
            Contract::MutationReceipt {
                operation: operation.into(),
            },
        ),
        (
            Kind::Branch,
            "POST",
            "/api/repositories/repo%2F%21/branches",
            json!({"source_commit_id":"source","name":"Name","operation_id":operation}),
            Contract::Branch {
                operation: operation.into(),
                name: "Name".into(),
                source_commit_id: "source".into(),
            },
        ),
        (
            Kind::Link,
            "POST",
            "/api/history-trees/history%2F%21/merge-links",
            json!({"source_branch_id":"sb","source_commit_id":"sc","target_branch_id":"tb","target_commit_id":"tc","operation_id":operation}),
            Contract::MergeLink {
                operation: operation.into(),
                history_tree_id: "history/!".into(),
                source_branch_id: "sb".into(),
                source_commit_id: "sc".into(),
                target_branch_id: "tb".into(),
                target_commit_id: "tc".into(),
            },
        ),
        (
            Kind::Purge,
            "POST",
            "/api/history-trees/history%2F%21/purge-plans",
            json!({"version_ids":["a","b","c"]}),
            Contract::Preview {
                kind: PreviewKind::Purge,
                history_tree_id: "history/!".into(),
                principal: "admin".into(),
            },
        ),
        (
            Kind::Split,
            "POST",
            "/api/history-trees/history%2F%21/split-plans",
            json!({"cut_commit_id":"source"}),
            Contract::Preview {
                kind: PreviewKind::Split,
                history_tree_id: "history/!".into(),
                principal: "admin".into(),
            },
        ),
        (
            Kind::Save,
            "POST",
            "/api/history-trees/history%2F%21/historical-save-plans",
            json!({"source_commit_id":"source","name":"Name","workspace_id":"workspace","primary_slot_id":"P01","target_copies":2}),
            Contract::Preview {
                kind: PreviewKind::HistoricalSave,
                history_tree_id: "history/!".into(),
                principal: "admin".into(),
            },
        ),
    ] {
        let request = build(kind, &fields, operation, "admin").unwrap();
        assert_eq!(request.method, method, "{kind:?}");
        assert_eq!(request.path, path, "{kind:?}");
        assert_eq!(request.body, Some(body), "{kind:?}");
        assert_eq!(request.contract, contract, "{kind:?}");
        assert!(request.contract.is_write());
        assert_eq!(
            request.contract.mutation_operation(),
            match kind {
                Kind::Purge | Kind::Split | Kind::Save => None,
                Kind::Repository
                | Kind::Smb
                | Kind::Policy
                | Kind::Rule
                | Kind::Branch
                | Kind::Link => Some(operation),
                Kind::Inspect => unreachable!(),
            }
        );
    }
}

#[test]
fn optional_historical_save_overrides_remain_explicit_nulls() {
    let mut fields = all_fields();
    for key in ["workspace", "slot", "copies"] {
        fields.remove(key);
    }
    let request = build(Kind::Save, &fields, "original", "admin").unwrap();
    assert_eq!(
        request.body,
        Some(
            json!({"source_commit_id":"source","name":"Name","workspace_id":null,"primary_slot_id":null,"target_copies":null})
        )
    );
}

#[test]
fn both_plan_actions_preserve_body_and_use_the_raw_receipt_contract() {
    let plan = json!({"plan_id":"p/!","state":"ready","digest":"reviewed"});
    for commit in [true, false] {
        let request = plan_action(&plan, commit, true, "original").unwrap();
        assert_eq!(request.method, "POST");
        assert_eq!(
            request.path,
            if commit {
                "/api/plans/p%2F%21/commit"
            } else {
                "/api/plans/p%2F%21/cancel"
            }
        );
        assert_eq!(
            request.body,
            Some(if commit {
                json!({"plan_digest":"reviewed","operation_id":"original"})
            } else {
                json!({"operation_id":"original"})
            })
        );
        assert_eq!(
            request.contract,
            Contract::PlanReceipt {
                operation: "original".into()
            }
        );
        assert!(request.contract.is_write());
        assert_eq!(request.contract.mutation_operation(), Some("original"));
    }
    assert!(matches!(
        plan_action(&plan, true, false, "original"),
        Err(InputError::ReviewRequired)
    ));
    assert!(plan_action(&plan, false, false, "original").is_ok());
    assert!(matches!(
        plan_action(&Value::Null, true, true, "original"),
        Err(InputError::MissingPlan)
    ));
    for state in ["committed", "cancelled", "stale"] {
        let mut plan = plan.clone();
        plan["state"] = state.into();
        for commit in [true, false] {
            assert!(matches!(
                plan_action(&plan, commit, true, "original"),
                Err(InputError::PlanNotReady)
            ));
        }
    }
    let mut plan = plan;
    plan["digest"] = "".into();
    assert!(matches!(
        plan_action(&plan, true, true, "original"),
        Err(InputError::MissingDigest)
    ));
}

#[test]
fn fixed_inspections_and_identity_queries_keep_get_encoding_and_contracts() {
    let fields = all_fields();
    let expected = [
        ("Repositories", "/api/repositories", Contract::Inspect),
        ("Disks", "/api/admin/disks", Contract::Inspect),
        ("Integrity check", "/api/admin/check", Contract::Inspect),
        (
            "Repository",
            "/api/repositories/repo%2F!",
            Contract::Inspect,
        ),
        (
            "SMB binding",
            "/api/repositories/repo%2F!/smb-binding",
            Contract::Inspect,
        ),
        (
            "Operation",
            "/api/operations/id%2F!",
            Contract::Operation { id: "id/!".into() },
        ),
        (
            "Plan",
            "/api/plans/id%2F!",
            Contract::Plan { id: "id/!".into() },
        ),
        (
            "Branch tree",
            "/api/branches/id%2F!/tree",
            Contract::Inspect,
        ),
        ("Branch details", "/api/branches/id%2F!", Contract::Inspect),
        (
            "File history",
            "/api/files/id%2F!/history",
            Contract::Inspect,
        ),
        (
            "History graph",
            "/api/history-trees/id%2F!/graph",
            Contract::Inspect,
        ),
        ("Policy", "/api/policies/id%2F!", Contract::Inspect),
        (
            "Placement rule",
            "/api/primary-placement-rules/id%2F!",
            Contract::Inspect,
        ),
        (
            "Topology",
            "/api/backup-topologies/default",
            Contract::Inspect,
        ),
    ];
    assert_eq!(QUERIES.len(), expected.len());
    for ((label, kind), (expected_label, path, contract)) in QUERIES.into_iter().zip(expected) {
        let request = query(kind, &fields).unwrap();
        assert_eq!(label, expected_label);
        assert_eq!(request.method, "GET");
        assert_eq!(request.path, path);
        assert_eq!(request.body, None);
        assert_eq!(request.contract, contract);
        assert!(!request.contract.is_write());
        assert_eq!(request.contract.mutation_operation(), None);
    }
    for (_, kind) in QUERIES {
        let key = match kind {
            QueryKind::Inspect {
                prefix, key: None, ..
            } => {
                for fields in [
                    Fields::new(),
                    [
                        ("id".into(), "ignored/!".into()),
                        ("".into(), "also ignored".into()),
                    ]
                    .into_iter()
                    .collect(),
                ] {
                    let request = query(kind, &fields).unwrap();
                    assert_eq!(request.method, "GET");
                    assert_eq!(request.path, prefix);
                    assert_eq!(request.body, None);
                    assert_eq!(request.contract, Contract::Inspect);
                }
                continue;
            }
            QueryKind::Inspect { key: Some(key), .. } => key,
            QueryKind::Operation | QueryKind::Plan => "id",
        };
        for fields in [
            Fields::new(),
            [(key.into(), String::new())].into_iter().collect(),
            [(key.into(), " \t".into())].into_iter().collect(),
        ] {
            assert!(
                matches!(query(kind, &fields), Err(InputError::Required(required)) if required == key)
            );
        }
    }
    let original = operation("id/!");
    let queried = query(QueryKind::Operation, &fields).unwrap();
    assert_eq!(original.path, queried.path);
    assert_eq!(original.contract, queried.contract);
}
