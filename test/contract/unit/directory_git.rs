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
    assert_eq!(negative, 38);
}
