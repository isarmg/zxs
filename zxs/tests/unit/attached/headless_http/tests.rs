use super::*;

const TARGET: &str =
    "/zxs/v1/containers/11111111111111111111111111111111/branches/22222222222222222222222222222222";
const OP: &str = "33333333333333333333333333333333";

#[test]
fn r3_create_branch_uses_exact_json_extent() {
    let wire = b"{\"from\":\"44444444444444444444444444444444\"}";
    let length = wire.len().to_string();
    let request = workspace_protocol::request(
        "PUT",
        TARGET,
        &[
            ("content-type", "application/json"),
            ("content-length", &length),
            ("zxs-operation", OP),
        ],
    )
    .unwrap();
    assert!(matches!(request, Request::CreateBranch { .. }));
    let mut combined = wire.to_vec();
    combined.extend_from_slice(b"unrelated pipeline bytes");
    let mut physical = combined.as_slice();
    {
        let mut inbound = body_for(&request, &mut physical).ok().unwrap();
        let Incoming::Json(body) = &mut inbound else {
            panic!("expected bounded JSON");
        };
        let mut actual = Vec::new();
        body.read_to_end(&mut actual).unwrap();
        assert_eq!(actual, wire);
        assert!(inbound.exhausted());
    }
    assert_eq!(physical, b"unrelated pipeline bytes");
}

#[test]
fn r3_create_branch_short_or_oversized_json_has_no_successful_extent() {
    let request = |len: &str| {
        workspace_protocol::request(
            "PUT",
            TARGET,
            &[
                ("content-type", "application/json"),
                ("content-length", len),
                ("zxs-operation", OP),
            ],
        )
        .unwrap()
    };
    let mut short = b"{".as_slice();
    let one = request("3");
    let mut inbound = body_for(&one, &mut short).ok().unwrap();
    let Incoming::Json(body) = &mut inbound else {
        panic!("expected bounded JSON");
    };
    let mut actual = Vec::new();
    assert_eq!(body.read_to_end(&mut actual).unwrap(), 1);
    assert!(!inbound.exhausted());

    let large = request(&(MAX_JSON + 1).to_string());
    assert!(matches!(
        body_for(&large, &mut io::empty()),
        Err(Problem::TooLarge)
    ));
}
