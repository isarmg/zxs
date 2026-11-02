#[path = "foundation-redirect-headers.rs"]
mod redirect_headers;
#[path = "foundation-redirect-controls.rs"]
mod redirect_controls;

#[test]
fn ranges_reject_empty_resources_without_fabricating_byte_zero() {
    for value in ["bytes=0-", "bytes=0-0", "bytes=-1", "bytes=1-2"] {
        assert!(
            http_range_header::parse_range_header(value)
                .unwrap()
                .validate(0)
                .is_err(),
            "{value}"
        );
    }
}
#[test]
fn ranges_clip_to_size_but_reject_overlap_and_out_of_bounds() {
    assert_eq!(
        http_range_header::parse_range_header("bytes=2-99")
            .unwrap()
            .validate(5)
            .unwrap(),
        std::iter::once(2..=4).collect::<Vec<_>>()
    );
    assert_eq!(
        http_range_header::parse_range_header("bytes=-2")
            .unwrap()
            .validate(5)
            .unwrap(),
        std::iter::once(3..=4).collect::<Vec<_>>()
    );
    for value in ["bytes=5-", "bytes=0-2, 2-4", "bytes=4-2"] {
        assert!(
            http_range_header::parse_range_header(value)
                .unwrap()
                .validate(5)
                .is_err()
        );
    }
    assert_eq!(
        http_range_header::parse_range_header("bytes=4-4, 0-0, 2-2")
            .unwrap()
            .validate(5)
            .unwrap(),
        vec![4..=4, 0..=0, 2..=2]
    );
    assert_eq!(
        http_range_header::parse_range_header("bytes=0-2, 1-1, 4-3")
            .unwrap()
            .validate(5),
        Err(http_range_header::RangeUnsatisfiableError::RangeReversed)
    );
    assert_eq!(
        http_range_header::parse_range_header(
            "bytes=18446744073709551614-18446744073709551615, 0-0"
        )
        .unwrap()
        .validate(u64::MAX)
        .unwrap(),
        vec![(u64::MAX - 1)..=(u64::MAX - 1), 0..=0]
    );
    assert_range_overlap_detection_matches_pairwise_oracle();
}

fn assert_range_overlap_detection_matches_pairwise_oracle() {
    use http_range_header::{
        EndPosition, ParsedRanges, RangeUnsatisfiableError, StartPosition,
        SyntacticallyCorrectRange,
    };
    let candidates: Vec<_> = (0..4)
        .flat_map(|start| (0..4).map(move |end| start..=end))
        .collect();
    for first in &candidates {
        for second in &candidates {
            for third in &candidates {
                let bounds = vec![first.clone(), second.clone(), third.clone()];
                let expected = if bounds.iter().any(|range| range.start() > range.end()) {
                    Err(RangeUnsatisfiableError::RangeReversed)
                } else if bounds.iter().enumerate().any(|(index, left)| {
                    bounds[index + 1..].iter().any(|right| {
                        left.start() <= right.end() && right.start() <= left.end()
                    })
                }) {
                    Err(RangeUnsatisfiableError::OverlappingRanges)
                } else {
                    Ok(bounds.clone())
                };
                let parsed = ParsedRanges {
                    ranges: bounds
                        .iter()
                        .map(|range| SyntacticallyCorrectRange {
                            start: StartPosition::Index(*range.start()),
                            end: EndPosition::Index(*range.end()),
                        })
                        .collect(),
                };
                assert_eq!(parsed.validate(4), expected, "{bounds:?}");
            }
        }
    }
}
#[test]
fn mime_guess_handles_extensions_case_and_unknown() {
    assert_eq!(
        mime_guess::from_path("README.HTML").first_raw(),
        Some("text/html")
    );
    assert_eq!(
        mime_guess::from_path("photo.png").first_raw(),
        Some("image/png")
    );
    assert!(
        mime_guess::from_path("data.zxs_unknown_extension")
            .first()
            .is_none()
    );
}
#[test]
fn serve_dir_empty_range_is_416_and_head_is_empty_200() {
    use bytes::Bytes;
    use http::{Method, Request, StatusCode};
    use http_body::{BodyExt, Empty};
    use service_stack::ServiceExt;
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("empty.txt"), []).unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let service = tower_http::services::ServeDir::new(root.path());
        let request = Request::builder()
            .uri("/empty.txt")
            .header("range", "bytes=0-")
            .body(Empty::<Bytes>::new())
            .unwrap();
        let response = service.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::RANGE_NOT_SATISFIABLE);
        assert_eq!(response.headers()["content-range"], "bytes */0");
        let request = Request::builder()
            .method(Method::HEAD)
            .uri("/empty.txt")
            .body(Empty::<Bytes>::new())
            .unwrap();
        let response = service.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-length"], "0");
        assert!(
            response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .is_empty()
        );
    });
}
#[test]
fn serve_dir_range_delivers_exact_bytes_and_preserves_head_length() {
    use bytes::Bytes;
    use http::{Method, Request, StatusCode};
    use http_body::{BodyExt, Empty};
    use service_stack::ServiceExt;
    let root = tempfile::tempdir().unwrap();
    std::fs::write(root.path().join("data.txt"), b"abcdef").unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let service = tower_http::services::ServeDir::new(root.path());
        let request = Request::builder()
            .uri("/data.txt")
            .header("range", "bytes=1-3")
            .body(Empty::<Bytes>::new())
            .unwrap();
        let response = service.clone().oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::PARTIAL_CONTENT);
        assert_eq!(response.headers()["content-range"], "bytes 1-3/6");
        assert_eq!(
            response
                .into_body()
                .collect()
                .await
                .unwrap()
                .to_bytes()
                .as_ref(),
            b"bcd"
        );
        let request = Request::builder()
            .method(Method::HEAD)
            .uri("/data.txt")
            .header("range", "bytes=99-")
            .body(Empty::<Bytes>::new())
            .unwrap();
        let response = service.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["content-length"], "6");
    });
}
