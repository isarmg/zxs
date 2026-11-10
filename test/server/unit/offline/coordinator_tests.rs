use super::*;
use crate::offline::service_worker::{self, Envelope, Request};
use data_serialization::Deserialize;

#[derive(Serialize, Deserialize)]
#[serialization(deny_unknown_fields)]
struct Recorded {
    context: layout::Context,
    purpose: String,
}
impl Plan for Recorded {
    const ROOT_REQUIRED: &'static str = "root is required";
    const CONTEXT_CHANGED: &'static str = "original context changed";
    const PENDING: &'static str = "preserve the original operation";
    fn context(&self) -> &layout::Context {
        &self.context
    }
}
impl RetainedPlan for Recorded {
    const RECORD_PREFIX: &'static str = ".test-original-";
    const DIGEST_REQUIRED: &'static str = "exact lowercase digest required";
    const MISSING: &'static str = "original plan is missing";
    const DIGEST_CHANGED: &'static str = "original digest changed";
}
fn context() -> layout::Context {
    let identity = json_codec::json!({"device":"1","inode":"2"});
    json_codec::from_value(json_codec::json!({
        "host":identity,"data":identity,"state":identity,
        "management":identity,"discovery":identity,
        "account":{"name":"zxs","uid":1001,"gid":1001,
            "source_device":"1","source_inode":"3","source_sha256":"a".repeat(64)},
    }))
    .unwrap()
}

#[test]
fn shared_plan_digest_retains_the_exact_compact_encoding_and_context() {
    let original = Recorded {
        context: context(),
        purpose: "original \"plan\"\n精确".into(),
    };
    let bytes = json_codec::to_vec(&original).unwrap();
    let expected = platform_io::lower_hex(&Sha256::digest(&bytes));
    assert_eq!(digest(&original).unwrap(), expected);
    let restored: Recorded = json_codec::from_slice(&bytes).unwrap();
    assert_eq!(digest(&restored).unwrap(), expected);
    assert!(std::ptr::eq(original.context(), &original.context));
    let mut changed = restored;
    changed.context.account.gid += 1;
    assert_ne!(digest(&changed).unwrap(), expected);
}

#[test]
fn retained_digest_admission_keeps_the_exact_lowercase_boundary() {
    for value in ["0".repeat(64), "abcdef0123456789".repeat(4)] {
        request_digest::<Recorded>(&value).unwrap();
    }
    for value in [
        String::new(),
        "a".repeat(63),
        "a".repeat(65),
        "A".repeat(64),
        "g".repeat(64),
        "../plan".into(),
        format!("{}\n", "a".repeat(63)),
    ] {
        assert_eq!(
            request_digest::<Recorded>(&value).unwrap_err().to_string(),
            Recorded::DIGEST_REQUIRED,
        );
    }
}

#[test]
fn pending_operation_preserves_its_message_and_original_error_source() {
    let error = pending::<Recorded>(Box::new(std::io::Error::new(
        std::io::ErrorKind::PermissionDenied,
        "exact native failure",
    )));
    assert_eq!(
        error.to_string(),
        "preserve the original operation: exact native failure",
    );
    assert_eq!(
        error
            .source()
            .unwrap()
            .downcast_ref::<std::io::Error>()
            .unwrap()
            .kind(),
        std::io::ErrorKind::PermissionDenied,
    );
}

#[derive(Serialize, Deserialize)]
#[serialization(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
enum Echo {
    Observe { value: String },
}
impl service_worker::Action for Echo {
    type Output = String;
    const COMMAND: &'static str = "internal-test-worker";
    fn execute(request: Envelope<Self>) -> Result<String> {
        let Echo::Observe { value } = request.action;
        Ok(format!("{}:{value}", request.context.account.name.as_str()))
    }
}

#[test]
fn shared_worker_envelope_preserves_bytes_account_and_dispatch() {
    #[derive(Serialize)]
    struct Previous<'a> {
        context: &'a layout::Context,
        action: &'a Echo,
    }
    let request = Envelope {
        context: context(),
        action: Echo::Observe {
            value: "bounded original".into(),
        },
    };
    let bytes = json_codec::to_vec(&request).unwrap();
    assert_eq!(
        bytes,
        json_codec::to_vec(&Previous {
            context: &request.context,
            action: &request.action,
        })
        .unwrap(),
    );
    assert!(std::ptr::eq(request.account(), &request.context.account));
    assert_eq!(<Envelope<Echo> as Request>::COMMAND, "internal-test-worker",);
    assert_eq!(<Envelope<Echo> as Request>::MAX_RESPONSE_BYTES, 64 * 1024);
    let decoded: Envelope<Echo> = json_codec::from_slice(&bytes).unwrap();
    assert_eq!(decoded.execute().unwrap(), "zxs:bounded original");
}

#[test]
fn shared_worker_envelope_keeps_strict_context_and_action_fields() {
    let request = Envelope {
        context: context(),
        action: Echo::Observe { value: "x".into() },
    };
    let value = json_codec::to_value(request).unwrap();
    for field in ["context", "action"] {
        let mut changed = value.clone();
        changed.as_object_mut().unwrap().remove(field);
        assert!(json_codec::from_value::<Envelope<Echo>>(changed).is_err());
    }
    let mut changed = value.clone();
    changed["uid"] = 0.into();
    assert!(json_codec::from_value::<Envelope<Echo>>(changed).is_err());
    let mut changed = value.clone();
    changed["context"]["unverified_owner"] = true.into();
    assert!(json_codec::from_value::<Envelope<Echo>>(changed).is_err());
    let mut changed = value;
    changed["action"]["skip_validation"] = true.into();
    assert!(json_codec::from_value::<Envelope<Echo>>(changed).is_err());
}
