use super::*;
use json_codec::json;
use storage_contract::new_id;

#[test]
fn protection_commands_validate_wire_types_and_unknown_fields() {
    let policy = json!({"policy_id":new_id("policy"),"repository_id":new_id("repo"),
            "path_prefix":"","target_copies":3,"expected_generation":"0","operation_id":new_id("op")});
    let rule = json!({"rule_id":new_id("rule"),"repository_id":new_id("repo"),
            "path_prefix":"","primary_slot_id":"P01","expected_generation":"0","operation_id":new_id("op")});
    for (is_policy, valid) in [(true, policy), (false, rule)] {
        let decode = |value: json_codec::Value| {
            if is_policy {
                parse_policy(value.as_object().unwrap().clone()).map(|_| ())
            } else {
                parse_placement(value.as_object().unwrap().clone()).map(|_| ())
            }
        };
        assert!(decode(valid.clone()).is_ok());
        for generation in [
            json!(0),
            json!(null),
            json!(""),
            json!("00"),
            json!("01"),
            json!("+1"),
            json!(" 1"),
            json!("-1"),
            json!("18446744073709551616"),
        ] {
            let mut invalid = valid.clone();
            invalid["expected_generation"] = generation;
            assert!(decode(invalid).is_err());
        }
        let mut max = valid.clone();
        max["expected_generation"] = json!(u64::MAX.to_string());
        assert!(decode(max).is_ok());
        for key in ["repository_id", "operation_id"] {
            let mut invalid = valid.clone();
            invalid[key] = json!("");
            assert!(decode(invalid).is_err());
        }
        let mut extra = valid.clone();
        extra["principal"] = json!("admin");
        assert!(decode(extra).is_err());
        let mut invalid = valid.clone();
        invalid["path_prefix"] = json!(null);
        assert!(decode(invalid).is_err());
        if is_policy {
            for copies in [json!("3"), json!(3.5), json!(-1), json!(4294967296_u64)] {
                let mut invalid = valid.clone();
                invalid["target_copies"] = copies;
                assert!(decode(invalid).is_err());
            }
        }
    }
}

#[test]
fn protection_parser_preserves_unknown_repository_and_operation_precedence() {
    for is_policy in [true, false] {
        let parse = |value: Value| {
            if is_policy {
                parse_policy(value.as_object().unwrap().clone()).map(|_| ())
            } else {
                parse_placement(value.as_object().unwrap().clone()).map(|_| ())
            }
        };
        assert_eq!(
            parse(json!({"unexpected":true})).unwrap_err().message,
            "unknown field unexpected"
        );
        assert_eq!(
            parse(json!({"expected_generation":false}))
                .unwrap_err()
                .message,
            "missing string field repository_id"
        );
        let mut value = if is_policy {
            json!({"policy_id":"policy","repository_id":"repo","path_prefix":"","target_copies":3,"expected_generation":"0","operation_id":"invalid"})
        } else {
            json!({"rule_id":"rule","repository_id":"repo","path_prefix":"","primary_slot_id":"P01","expected_generation":"0","operation_id":"invalid"})
        };
        let error = parse(value.clone()).unwrap_err();
        assert_eq!(error.code, storage_contract::ErrorCode::InvalidId);
        assert!(std::error::Error::source(&error).is_some());
        value["expected_generation"] = json!(0);
        assert_eq!(
            parse(value).unwrap_err().code,
            storage_contract::ErrorCode::InvalidRequest
        );
    }
}
