use http_routing::response::IntoResponse;

pub(super) async fn script() -> impl IntoResponse {
    (
        [("content-type", "text/javascript; charset=utf-8")],
        include_str!("../../../web/packages.js"),
    )
}
