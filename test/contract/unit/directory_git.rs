use crate::directory_git::*;
use data_serialization::{Serialize, de::DeserializeOwned};
use json_codec::Value;

fn roundtrip<T: DeserializeOwned + Serialize>(value: Value) -> Result<Value, json_codec::Error> {
    let decoded: T = json_codec::from_value(value)?;
    json_codec::to_value(decoded)
}

fn decode(schema: &str, value: Value) -> Option<Result<Value, json_codec::Error>> {
    Some(match schema {
        "DirectoryGitChildReference" => roundtrip::<DirectoryGitChildReference>(value),
        "DirectoryGitBranch" => roundtrip::<DirectoryGitBranch>(value),
        "DirectoryGitRepository" => roundtrip::<DirectoryGitRepository>(value),
        "DirectoryGitOwner" => roundtrip::<DirectoryGitOwner>(value),
        "DirectoryGitChildStatus" => roundtrip::<DirectoryGitChildStatus>(value),
        "DirectoryGitStatus" => roundtrip::<DirectoryGitStatus>(value),
        "DirectoryGitInitializeBody" => roundtrip::<DirectoryGitInitializeBody>(value),
        "DirectoryGitStageChildBody" => roundtrip::<DirectoryGitStageChildBody>(value),
        "DirectoryGitStageOwnedBody" => roundtrip::<DirectoryGitStageOwnedBody>(value),
        "DirectoryCommitBody" => roundtrip::<crate::directory_staging::DirectoryCommitBody>(value),
        _ => return None,
    })
}

#[test]
fn directory_branch_create_source_vectors_match_wire_and_identity_validation() {
    let vectors: Value =
        json_codec::from_str(include_str!("../../../docs/contracts/vectors.json")).unwrap();
    let mut counts = [0, 0];
    for (index, group) in ["positive", "negative"].into_iter().enumerate() {
        for vector in vectors[group].as_array().unwrap() {
            let name = vector["name"].as_str().unwrap();
            if !name.starts_with("directory-branch-source-") {
                continue;
            }
            assert_eq!(vector["schema"], "DirectoryBranchCreateSource", "{name}");
            let decoded: Result<DirectoryBranchCreateSource, _> =
                json_codec::from_value(vector["value"].clone());
            if index == 0 {
                let source = decoded.unwrap();
                source.validate().unwrap();
                assert_eq!(
                    json_codec::to_value(source).unwrap(),
                    vector["value"],
                    "{name}"
                );
            } else {
                assert!(
                    match decoded {
                        Ok(source) => source.validate().is_err(),
                        Err(_) => true,
                    },
                    "accepted {name}"
                );
            }
            counts[index] += 1;
        }
    }
    assert_eq!(counts, [2, 21]);
}

#[test]
fn directory_git_fixed_vectors_match_strict_rust_wire_types() {
    let vectors: Value =
        json_codec::from_str(include_str!("../../../docs/contracts/vectors.json")).unwrap();
    let mut positive = 0;
    let mut negative = 0;
    for group in ["positive", "negative"] {
        for vector in vectors[group].as_array().unwrap() {
            let name = vector["name"].as_str().unwrap();
            if !name.starts_with("directory-git-") {
                continue;
            }
            let Some(result) = decode(vector["schema"].as_str().unwrap(), vector["value"].clone())
            else {
                continue;
            };
            if group == "positive" {
                assert_eq!(result.unwrap(), vector["value"], "{name}");
                positive += 1;
            } else {
                assert!(result.is_err(), "accepted {name}");
                negative += 1;
            }
        }
    }
    assert_eq!(positive, 10);
    assert_eq!(negative, 40);
}

#[test]
fn directory_layout_wire_is_strict_and_not_a_business_receipt() {
    let request =
        json_codec::json!({"expected_namespace_head":"commit_00000000000000000000000000000001"});
    assert_eq!(
        roundtrip::<DirectoryLayoutReconcileBody>(request.clone()).unwrap(),
        request
    );
    let mut unknown = request;
    unknown["operation_id"] = "op_00000000000000000000000000000001".into();
    assert!(roundtrip::<DirectoryLayoutReconcileBody>(unknown).is_err());
    let result = json_codec::json!({"repository_id":"repo_00000000000000000000000000000001","state_sequence":"18446744073709551615","directories":"0","containers":"2","unavailable_containers":"1","unavailable_histories":"0"});
    assert_eq!(
        roundtrip::<DirectoryLayoutResult>(result.clone()).unwrap(),
        result
    );
    for key in [
        "state_sequence",
        "directories",
        "containers",
        "unavailable_containers",
        "unavailable_histories",
    ] {
        let mut invalid = result.clone();
        invalid[key] = 1.into();
        assert!(roundtrip::<DirectoryLayoutResult>(invalid).is_err());
        let mut missing = result.clone();
        missing.as_object_mut().unwrap().remove(key);
        assert!(roundtrip::<DirectoryLayoutResult>(missing).is_err());
    }
}

#[test]
fn directory_layout_fixed_vectors_match_wire_types() {
    let vectors: Value =
        json_codec::from_str(include_str!("../../../docs/contracts/vectors.json")).unwrap();
    let mut counts = [0, 0];
    for (index, group) in ["positive", "negative"].into_iter().enumerate() {
        for vector in vectors[group].as_array().unwrap() {
            let name = vector["name"].as_str().unwrap();
            if !name.starts_with("directory-layout-") {
                continue;
            }
            let value = vector["value"].clone();
            let decoded = match vector["schema"].as_str().unwrap() {
                "DirectoryLayoutReconcileBody" => roundtrip::<DirectoryLayoutReconcileBody>(value),
                "DirectoryLayoutResult" => roundtrip::<DirectoryLayoutResult>(value),
                other => panic!("uncovered layout schema: {other}"),
            };
            if index == 0 {
                assert_eq!(decoded.unwrap(), vector["value"], "{name}");
            } else {
                assert!(decoded.is_err(), "{name}");
            }
            counts[index] += 1;
        }
    }
    assert_eq!(counts, [2, 3]);
}
