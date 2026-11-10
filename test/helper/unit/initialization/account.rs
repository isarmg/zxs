use super::*;

#[test]
fn native_account_instruction_has_one_fixed_nonlogin_profile() {
    let input = Input {
        service_user: Name::new("zxs".into()).unwrap(),
        service_group: Name::new("zxs".into()).unwrap(),
    };
    assert_eq!(
        instruction(&input),
        "u zxs - - /nonexistent /usr/sbin/nologin"
    );
    for name in ["root", "a b", "a\nuser", "--root", "user:group", "a\"b"] {
        assert!(Name::new(name.into()).is_err());
    }
}

#[test]
fn account_creation_requires_confirmation_before_host_access() {
    for (confirmation, stopped) in [("", true), ("prepare-local-service-account", false)] {
        let error = prepare(Prepare {
            input: Input {
                service_user: Name::new("zxs".into()).unwrap(),
                service_group: Name::new("zxs".into()).unwrap(),
            },
            request_digest: "a".repeat(64),
            confirmation: confirmation.into(),
            external_administration_stopped: stopped,
        })
        .err()
        .unwrap();
        assert!(
            error
                .to_string()
                .contains("confirm prepare-local-service-account")
        );
    }
}

#[test]
fn account_request_cannot_override_program_uid_home_or_shell() {
    let input = json_codec::json!({"service_user":"zxs", "service_group":"zxs"});
    assert!(json_codec::from_value::<Input>(input.clone()).is_ok());
    for field in ["program", "uid", "home", "shell"] {
        let mut modified = input.clone();
        modified[field] = "override".into();
        assert!(json_codec::from_value::<Input>(modified).is_err());
    }
}
