use super::*;

enum Kind {
    Archive,
    Manifest,
}

pub(super) async fn archive(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    download(state, request, Kind::Archive).await
}
pub(super) async fn manifest(
    ExtractState(state): ExtractState<State>,
    request: Request,
) -> Result<Response, Failure> {
    download(state, request, Kind::Manifest).await
}

async fn download(state: State, request: Request, kind: Kind) -> Result<Response, Failure> {
    let permit = admit_job(&state, &request)?;
    let _: InstallPreviewRequest = input(request).await?;
    let verified = tokio::task::spawn_blocking(move || {
        let _owner = permit;
        crate::installation::materials::verified().map_err(install_failure)
    })
    .await
    .map_err(|error| {
        Failure::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "legal_materials_worker",
            error,
        )
    })??;
    let (bytes, checksum, content_type, disposition) = match kind {
        Kind::Archive => (
            verified.archive,
            verified.metadata.archive_sha256,
            "application/gzip",
            "attachment; filename=\"zxs-license-materials.tar.gz\"",
        ),
        Kind::Manifest => (
            verified.manifest,
            verified.metadata.manifest_sha256,
            "application/json",
            "attachment; filename=\"zxs-license-materials.json\"",
        ),
    };
    let mut response = Response::new(Body::from(bytes));
    response
        .headers_mut()
        .insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
    response.headers_mut().insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static(disposition),
    );
    response
        .headers_mut()
        .insert("x-zxs-material-sha256", HeaderValue::from_static(checksum));
    Ok(response)
}
