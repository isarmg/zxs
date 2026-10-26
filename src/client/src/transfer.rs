//! Streaming transfer and atomic publication of verified immutable downloads.
use super::{Client, Error, response::decode, segment};
#[cfg(target_os = "linux")]
use futures::StreamExt;
use json_codec::Value;
use std::path::Path;
use tokio::adapters::io::ReaderStream;
#[cfg(target_os = "linux")]
use tokio::io::AsyncWriteExt;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt};

// Match the hashing pass's maximum read size without reserving a large body
// buffer for tiny files. Clamp before converting to usize (including on wasm).
fn upload_chunk_capacity(length: u64) -> usize {
    length.clamp(4 * 1024, 256 * 1024) as usize
}

fn upload_stream(
    file: tokio::fs::File,
    length: u64,
) -> ReaderStream<tokio::io::Take<tokio::fs::File>> {
    // Remain demand-driven: no producer task, read-ahead queue, or whole-file
    // buffering. Tokio performs at most one blocking read per file at a time.
    ReaderStream::with_capacity(file.take(length), upload_chunk_capacity(length))
}

// Hash only the selected length, then probe once for growth. Reading to EOF
// without a budget can follow a concurrently appended file indefinitely.
// This is not a filesystem snapshot: the server still verifies the final body
// hash, and the body stream cannot send bytes beyond the selected length.
async fn hash_upload_source(
    source: &mut (impl AsyncRead + Unpin),
    length: u64,
) -> Result<String, Error> {
    let mut remaining = length;
    let mut hasher = blake3::Hasher::new();
    let mut buffer = vec![0_u8; upload_chunk_capacity(length)];
    while remaining > 0 {
        let limit = remaining.min(buffer.len() as u64) as usize;
        let count = source.read(&mut buffer[..limit]).await?;
        if count == 0 {
            return Err(Error::Response(
                "upload source length changed during preparation".into(),
            ));
        }
        hasher.update(&buffer[..count]);
        remaining -= count as u64;
    }
    if source.read(&mut buffer[..1]).await? != 0 {
        return Err(Error::Response(
            "upload source length changed during preparation".into(),
        ));
    }
    Ok(hasher.finalize().to_hex().to_string())
}

/// A hashed, rewound upload ready for dispatch. Preparing does not send bytes.
pub struct PreparedUpload {
    request: reqwest::RequestBuilder,
}
impl PreparedUpload {
    pub async fn send(self) -> Result<Value, Error> {
        decode(self.request.send().await?).await
    }
}

impl Client {
    pub async fn upload_file(
        &self,
        workspace: &str,
        item: &str,
        path: &str,
        source: &Path,
    ) -> Result<Value, Error> {
        self.upload_open_file(
            workspace,
            item,
            path,
            Self::open_upload_source(source).await?,
        )
        .await
    }

    /// Open and validate a regular-file descriptor for the retained Rust client library.
    pub async fn open_upload_source(source: &Path) -> Result<tokio::fs::File, Error> {
        let mut options = tokio::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        // Opening a FIFO normally blocks before metadata can reject it. Keep
        // the open nonblocking, then validate the opened descriptor (not a
        // pre-open path check that could race a replacement). O_NONBLOCK has
        // no effect on regular-file reads and local symlinks remain supported.
        options.custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32);
        let file = options.open(source).await?;
        let metadata = file.metadata().await?;
        if !metadata.is_file() {
            return Err(Error::Response(
                "upload source must be a regular file".into(),
            ));
        }
        Ok(file)
    }

    /// Stream an already selected descriptor without reopening its original path.
    /// The descriptor is retained, but callers must not mutate the file or use
    /// duplicated descriptors that share its seek offset during the transfer.
    pub async fn upload_open_file(
        &self,
        workspace: &str,
        item: &str,
        path: &str,
        file: tokio::fs::File,
    ) -> Result<Value, Error> {
        self.prepare_file_upload(workspace, item, path, file)
            .await?
            .send()
            .await
    }
    pub async fn prepare_file_upload(
        &self,
        workspace: &str,
        item: &str,
        path: &str,
        mut file: tokio::fs::File,
    ) -> Result<PreparedUpload, Error> {
        let metadata = file.metadata().await?;
        if !metadata.is_file() {
            return Err(Error::Response(
                "upload source must be a regular file".into(),
            ));
        }
        file.seek(std::io::SeekFrom::Start(0)).await?;
        let content_hash = hash_upload_source(&mut file, metadata.len()).await?;
        file.seek(std::io::SeekFrom::Start(0)).await?;
        let mut url = self.url(&format!(
            "/api/workspaces/{}/files/{}/content",
            segment(workspace),
            segment(item)
        ))?;
        url.query_pairs_mut().append_pair("path", path);
        let mut request = self
            .client
            .put(url)
            .header(reqwest::header::CONTENT_TYPE, "application/octet-stream")
            .header(reqwest::header::CONTENT_LENGTH, metadata.len())
            .header("x-zxs-logical-size", metadata.len().to_string())
            .header("x-zxs-upload-generation", "1")
            .header("x-zxs-content-hash", content_hash)
            .body(reqwest::Body::wrap_stream(upload_stream(
                file,
                metadata.len(),
            )));
        if let Some(token) = &self.token {
            request = request.bearer_auth(token);
        }
        Ok(PreparedUpload { request })
    }
    /// Publish only a complete verified file, without replacing any existing
    /// destination. Staging owns cleanup on errors and cancellation.
    #[cfg(target_os = "linux")]
    pub async fn download_file(&self, version: &str, target: &Path) -> Result<(), Error> {
        let selected_target = super::download_target::Target::open(target)?;
        let response = self
            .builder(
                reqwest::Method::GET,
                &format!("/api/versions/{}/content", segment(version)),
            )?
            .send()
            .await?;
        if !response.status().is_success() {
            decode(response).await?;
            return Err(Error::Response("unexpected success envelope".into()));
        }
        let expected = response
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| {
                value
                    .strip_prefix('"')
                    .and_then(|value| value.strip_suffix('"'))
            })
            .filter(|value| {
                value.len() == 64
                    && value
                        .bytes()
                        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
            })
            .ok_or_else(|| Error::Response("missing or invalid immutable content hash".into()))?
            .to_owned();
        let expected_length = response
            .content_length()
            .ok_or_else(|| Error::Response("missing file length".into()))?;
        let (temporary, cleanup) = selected_target.stage()?;
        let mut file = tokio::fs::File::from_std(temporary);
        let mut length = 0u64;
        let mut hash = blake3::Hasher::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            length = length
                .checked_add(chunk.len() as u64)
                .ok_or_else(|| Error::Response("length overflow".into()))?;
            if length > expected_length {
                return Err(Error::Response("content exceeded advertised length".into()));
            }
            hash.update(&chunk);
            file.write_all(&chunk).await?;
        }
        if length != expected_length || hash.finalize().to_hex().as_str() != expected {
            return Err(Error::ChecksumMismatch);
        }
        file.sync_all().await?;
        drop(file);
        // No empty reservation file is exposed. Publication is an indivisible
        // no-replace operation, so a concurrent creator always keeps its file.
        tokio::task::spawn_blocking(move || cleanup.publish()).await??;
        Ok(())
    }
    /// Verified publication has no weaker pathname-based compatibility mode.
    #[cfg(not(target_os = "linux"))]
    pub async fn download_file(&self, _version: &str, _target: &Path) -> Result<(), Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "verified native downloads require Linux descriptor-backed publication",
        )
        .into())
    }
}

#[cfg(test)]
#[path = "../../../test/client/unit/upload_stream_tests.rs"]
mod upload_stream_tests;
