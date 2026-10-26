use super::{Client, Error};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
};

async fn request_headers(stream: &mut TcpStream) {
    let mut request = Vec::new();
    while !request.ends_with(b"\r\n\r\n") {
        let mut byte = [0];
        assert_eq!(stream.read(&mut byte).await.unwrap(), 1);
        request.push(byte[0]);
        assert!(request.len() < 8192);
    }
    let length = std::str::from_utf8(&request)
        .unwrap()
        .lines()
        .filter_map(|line| line.split_once(':'))
        .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
        .map(|(_, value)| value.trim().parse::<usize>().unwrap())
        .unwrap_or(0);
    assert!(length < 8192);
    stream.read_exact(&mut vec![0; length]).await.unwrap();
}

async fn reply(response: Vec<u8>) -> (Client, tokio::task::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = Client::connect(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        request_headers(&mut stream).await;
        stream.write_all(&response).await.unwrap();
    });
    (client, task)
}

#[tokio::test]
async fn logout_requires_an_exact_http_receipt_before_discarding_the_bearer() {
    for (status, body, confirmed) in [
        (200, r#"{"logged_out":true}"#, true),
        (200, "", false),
        (204, "", false),
        (200, "null", false),
        (200, "[]", false),
        (200, "true", false),
        (200, "{}", false),
        (200, r#"{"logged_out":false}"#, false),
        (200, r#"{"logged_out":"true"}"#, false),
        (200, r#"{"logged_out":true,"admin":true}"#, false),
        (200, r#"{"logged_out":true,"extra":null}"#, false),
        (200, r#"{"logged_out":true,"logged_out":true}"#, false),
        // Preserve the native client's existing error/credential policy for 401.
        (401, r#"{"code":"UNAUTHORIZED"}"#, false),
    ] {
        let response = format!(
            "HTTP/1.1 {status} Test\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let (mut client, server) = reply(response.into_bytes()).await;
        client.set_token(Some("original-bearer".into()));
        let result = client.logout().await;
        assert_eq!(result.is_ok(), confirmed, "status={status}, body={body}");
        assert_eq!(
            client.token.as_deref(),
            (!confirmed).then_some("original-bearer"),
            "status={status}, body={body}"
        );
        server.await.unwrap();
    }
}

fn content_headers(content: &[u8]) -> Vec<u8> {
    format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nETag: \"{}\"\r\nConnection: close\r\n\r\n",
        content.len(),
        blake3::hash(content)
    )
    .into_bytes()
}

#[tokio::test]
async fn verified_download_uses_private_tempfile_and_supports_long_destination_names() {
    let content = b"the complete immutable version";
    let mut response = content_headers(content);
    response.extend(content);
    let (client, server) = reply(response).await;
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("a".repeat(240));
    client.download_file("version_test", &target).await.unwrap();
    assert_eq!(std::fs::read(&target).unwrap(), content);
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(target).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    server.await.unwrap();
}

#[tokio::test]
async fn checksum_failure_removes_temporary_data_without_publishing_a_destination() {
    let mut response = content_headers(b"good");
    response.extend(b"bad!");
    let (client, server) = reply(response).await;
    let directory = tempfile::tempdir().unwrap();
    let result = client
        .download_file("version_test", &directory.path().join("file.txt"))
        .await;
    assert!(matches!(result, Err(Error::ChecksumMismatch)));
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    server.await.unwrap();
}

#[tokio::test]
async fn a_concurrent_creator_keeps_its_file_and_download_cleans_up() {
    let content = b"the complete immutable version";
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = Client::connect(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let (sent, received) = oneshot::channel();
    let (release, wait) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        request_headers(&mut stream).await;
        stream.write_all(&content_headers(content)).await.unwrap();
        stream.write_all(&content[..4]).await.unwrap();
        sent.send(()).unwrap();
        wait.await.unwrap();
        stream.write_all(&content[4..]).await.unwrap();
    });
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("file.txt");
    let download_target = target.clone();
    let download =
        tokio::spawn(async move { client.download_file("version_test", &download_target).await });
    received.await.unwrap();
    std::fs::write(&target, b"concurrent creator").unwrap();
    release.send(()).unwrap();
    assert!(
        matches!(download.await.unwrap(), Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::AlreadyExists)
    );
    assert_eq!(std::fs::read(target).unwrap(), b"concurrent creator");
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    server.await.unwrap();
}

#[tokio::test]
async fn empty_success_envelopes_and_non_json_http_errors_keep_their_meaning() {
    let (client, server) =
        reply(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n".to_vec()).await;
    assert!(
        client
            .request("POST", "/api/auth/logout", None)
            .await
            .unwrap()
            .is_null()
    );
    server.await.unwrap();
    let (client, server) = reply(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 7\r\nConnection: close\r\n\r\nblocked".to_vec()).await;
    assert!(matches!(
        client.request("GET", "/api/system/status", None).await,
        Err(Error::Api {
            status: 503,
            retryable: true,
            ..
        })
    ));
    server.await.unwrap();
    let (client, server) =
        reply(b"HTTP/1.1 200 OK\r\nContent-Length: 42\r\nConnection: close\r\n\r\n".to_vec()).await;
    assert!(
        client
            .request("HEAD", "/api/system/status", None)
            .await
            .unwrap()
            .is_null()
    );
    server.await.unwrap();
}

#[tokio::test]
async fn a_malformed_seal_response_is_an_error_instead_of_a_panic_or_cross_workspace_commit() {
    for body in ["[]", "{\"workspace_id\":\"other_workspace\"}", "{}"] {
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        let (client, server) = reply(response.into_bytes()).await;
        assert!(matches!(
            client
                .prepare_commit("workspace_test", "op_test", "save", None)
                .await,
            Err(Error::Contract(_))
        ));
        server.await.unwrap();
    }
}

#[tokio::test]
async fn truncated_download_removes_partial_file() {
    let mut response = content_headers(b"a complete file");
    response.extend(b"a partial");
    let (client, server) = reply(response).await;
    let directory = tempfile::tempdir().unwrap();
    assert!(matches!(
        client
            .download_file("version_test", &directory.path().join("file"))
            .await,
        Err(Error::Transport(_))
    ));
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    server.await.unwrap();
}

#[tokio::test]
async fn cancelling_an_in_progress_download_removes_its_private_tempfile() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = Client::connect(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let (release, wait) = oneshot::channel::<()>();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        request_headers(&mut stream).await;
        stream
            .write_all(&content_headers(b"unfinished download"))
            .await
            .unwrap();
        stream.write_all(b"unfin").await.unwrap();
        let _ = wait.await;
    });
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("file");
    let download = tokio::spawn(async move { client.download_file("version_test", &target).await });
    // Wait for bytes on disk, rather than guessing when the transfer started.
    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if std::fs::read_dir(directory.path()).unwrap().any(|entry| {
                match std::fs::read(entry.unwrap().path().join("payload")) {
                    Ok(bytes) => bytes == b"unfin",
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => false,
                    Err(error) => panic!("could not read download payload: {error}"),
                }
            }) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("download never wrote its partial file");
    download.abort();
    assert!(download.await.unwrap_err().is_cancelled());
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
    drop(release);
    server.await.unwrap();
}

#[tokio::test]
async fn existing_destination_is_preserved_before_any_network_request() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = Client::connect(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let directory = tempfile::tempdir().unwrap();
    let target = directory.path().join("file");
    std::fs::write(&target, b"keep me").unwrap();
    assert!(
        matches!(client.download_file("version_test", &target).await,
        Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::AlreadyExists)
    );
    assert_eq!(std::fs::read(&target).unwrap(), b"keep me");
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(50), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn missing_or_weak_integrity_headers_never_publish_downloads() {
    for headers in [
        "Content-Length: 4\r\n".to_owned(),
        format!(
            "Content-Length: 4\r\nETag: W/\"{}\"\r\n",
            blake3::hash(b"file")
        ),
        "Content-Length: 4\r\nETag: \"not-a-hash\"\r\n".to_owned(),
        format!("ETag: \"{}\"\r\n", blake3::hash(b"file")),
    ] {
        let (client, server) = reply(
            format!("HTTP/1.1 200 OK\r\n{headers}Connection: close\r\n\r\nfile").into_bytes(),
        )
        .await;
        let directory = tempfile::tempdir().unwrap();
        assert!(matches!(
            client
                .download_file("version_test", &directory.path().join("file"))
                .await,
            Err(Error::Response(_))
        ));
        assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 0);
        server.await.unwrap();
    }
}

#[tokio::test]
async fn upload_sends_raw_bytes_with_matching_length_hash_and_bearer() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let mut client =
        Client::connect(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    client.set_token(Some("synthetic-token".into()));
    let content = b"\0binary\xff\r\nbytes";
    let source = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(source.path(), content).unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut headers = Vec::new();
        while !headers.ends_with(b"\r\n\r\n") {
            headers.push(stream.read_u8().await.unwrap());
            assert!(headers.len() < 8192);
        }
        let headers = String::from_utf8(headers).unwrap().to_lowercase();
        assert!(headers.starts_with("put /api/workspaces/workspace_test/files/item_test/content?path=folder%2fa+b.bin http/1.1\r\n"));
        for header in [
            "authorization: bearer synthetic-token".to_owned(),
            "content-type: application/octet-stream".to_owned(),
            format!("content-length: {}", content.len()),
            format!("x-zxs-logical-size: {}", content.len()),
            format!("x-zxs-content-hash: {}", blake3::hash(content)),
            "x-zxs-upload-generation: 1".to_owned(),
        ] {
            assert!(
                headers.contains(&format!("{header}\r\n")),
                "missing {header}"
            );
        }
        let mut body = vec![0; content.len()];
        stream.read_exact(&mut body).await.unwrap();
        assert_eq!(body, content);
        stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 15\r\nConnection: close\r\n\r\n{\"staged\":true}").await.unwrap();
    });
    // The envelope is checked as well as the request wire format.
    assert_eq!(
        client
            .upload_file(
                "workspace_test",
                "item_test",
                "folder/a b.bin",
                source.path()
            )
            .await
            .unwrap(),
        json_codec::json!({"staged":true})
    );
    server.await.unwrap();
}

#[tokio::test]
async fn upload_integrity_rejections_preserve_server_error_details() {
    for code in [
        storage_contract::ErrorCode::UploadIncomplete,
        storage_contract::ErrorCode::ChecksumMismatch,
    ] {
        let body = json_codec::json!({"error":{"code":code,"message":"upload rejected","retryable":false,"request_id":"req_synthetic"}}).to_string();
        let (client, server) = reply(format!("HTTP/1.1 422 Unprocessable Entity\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).into_bytes()).await;
        let source = tempfile::NamedTempFile::new().unwrap();
        std::fs::write(source.path(), b"source is preserved").unwrap();
        assert!(
            matches!(client.upload_file("workspace_test", "item_test", "file", source.path()).await,
            Err(Error::Api {status:422, code:actual, message, retryable:false, ..})
            if actual == code && message == "upload rejected (request req_synthetic)")
        );
        assert_eq!(
            std::fs::read(source.path()).unwrap(),
            b"source is preserved"
        );
        server.await.unwrap();
    }
}

#[tokio::test]
async fn interrupted_upload_is_a_transport_error_and_preserves_source() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = Client::connect(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let source = tempfile::NamedTempFile::new().unwrap();
    let content = vec![0x5a; 1024 * 1024];
    std::fs::write(source.path(), &content).unwrap();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut headers = Vec::new();
        while !headers.ends_with(b"\r\n\r\n") {
            headers.push(stream.read_u8().await.unwrap());
            assert!(headers.len() < 8192);
        }
        let mut prefix = [0; 16];
        stream.read_exact(&mut prefix).await.unwrap();
        assert_eq!(prefix, [0x5a; 16]);
        // Close before consuming the body or producing any response.
    });
    assert!(matches!(
        client
            .upload_file("workspace_test", "item_test", "file", source.path())
            .await,
        Err(Error::Transport(_))
    ));
    assert_eq!(std::fs::read(source.path()).unwrap(), content);
    server.await.unwrap();
}

#[tokio::test]
async fn response_adapter_enforces_shared_duplicate_key_and_status_rules() {
    for status in [200, 409, 503] {
        let body = br#"{"error":{"code":"first","co\u0064e":"second"}}"#;
        let mut response = format!(
            "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .into_bytes();
        response.extend_from_slice(body);
        let (client, server) = reply(response).await;
        let expected =
            storage_contract::client_protocol::decode_response(status, body).unwrap_err();
        match client
            .request("GET", "/api/system/status", None)
            .await
            .unwrap_err()
        {
            Error::Api {
                status,
                code,
                message,
                retryable,
                ..
            } => {
                assert_eq!(status, expected.status);
                assert_eq!(code, expected.code);
                assert_eq!(message, expected.message);
                assert_eq!(retryable, expected.retryable);
            }
            error => panic!("lost protocol status: {error}"),
        }
        server.await.unwrap();
    }
}

#[cfg(unix)]
#[test]
fn fifo_upload_child_exits_without_a_writer() {
    let Some(source) = std::env::var_os("ZXS_TEST_UPLOAD_FIFO") else {
        return;
    };
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let client = Client::connect("http://127.0.0.1:1").unwrap();
        let result = client
            .upload_file("workspace", "item", "file", std::path::Path::new(&source))
            .await;
        assert!(
            matches!(result, Err(Error::Response(message)) if message.contains("regular file"))
        );
    });
    // Returning must also shut down the runtime: a cancelled FIFO open can
    // otherwise keep Tokio's blocking pool alive forever.
    drop(runtime);
}

#[cfg(unix)]
#[test]
fn fifo_upload_is_rejected_without_blocking_runtime_shutdown() {
    use std::{
        process::Command,
        time::{Duration, Instant},
    };
    let directory = tempfile::tempdir().unwrap();
    let fifo = directory.path().join("source.fifo");
    assert!(
        Command::new("mkfifo")
            .arg(&fifo)
            .status()
            .unwrap()
            .success()
    );
    let mut child = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "transfer_tests::fifo_upload_child_exits_without_a_writer",
            "--nocapture",
        ])
        .env("ZXS_TEST_UPLOAD_FIFO", &fifo)
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "FIFO upload subprocess failed: {status}");
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            child.wait().unwrap();
            panic!("FIFO upload or runtime shutdown blocked without a FIFO writer");
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[tokio::test]
async fn upload_accepts_regular_files_and_rejects_directories() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    std::fs::write(&source, b"ordinary upload content").unwrap();
    let (client, server) =
        reply(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n".to_vec()).await;
    client
        .upload_file("workspace", "item", "file", &source)
        .await
        .unwrap();
    server.await.unwrap();
    assert!(matches!(
        client
            .upload_file("workspace", "item", "file", directory.path())
            .await,
        Err(Error::Response(_)) | Err(Error::Io(_))
    ));
}

#[cfg(unix)]
#[tokio::test]
async fn upload_preserves_symlinks_to_regular_files_and_rejects_devices() {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source");
    let alias = directory.path().join("alias");
    std::fs::write(&source, b"content through a local symlink").unwrap();
    std::os::unix::fs::symlink(&source, &alias).unwrap();
    let (client, server) =
        reply(b"HTTP/1.1 204 No Content\r\nConnection: close\r\n\r\n".to_vec()).await;
    client
        .upload_file("workspace", "item", "file", &alias)
        .await
        .unwrap();
    server.await.unwrap();
    assert!(
        matches!(client.upload_file("workspace", "item", "file", std::path::Path::new("/dev/null")).await,
        Err(Error::Response(message)) if message.contains("regular file"))
    );
}

#[cfg(unix)]
#[tokio::test]
async fn download_publication_and_cleanup_stay_with_the_selected_parent() {
    let content = b"the complete immutable version";
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = Client::connect(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let (release, wait) = oneshot::channel();
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        request_headers(&mut stream).await;
        stream.write_all(&content_headers(content)).await.unwrap();
        stream.write_all(&content[..4]).await.unwrap();
        wait.await.unwrap();
        stream.write_all(&content[4..]).await.unwrap();
    });
    let directory = tempfile::tempdir().unwrap();
    let parent = directory.path().join("selected");
    let moved = directory.path().join("original");
    std::fs::create_dir(&parent).unwrap();
    let target = parent.join("file");
    let download = tokio::spawn(async move { client.download_file("version_test", &target).await });
    let staging = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            if let Some(entry) = std::fs::read_dir(&parent).unwrap().next() {
                break entry.unwrap().file_name();
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("download never created its staging entry");
    std::fs::rename(&parent, &moved).unwrap();
    std::fs::create_dir(&parent).unwrap();
    std::fs::write(parent.join(staging), b"unverified replacement").unwrap();
    release.send(()).unwrap();
    download.await.unwrap().unwrap();
    assert!(
        !parent.join("file").exists(),
        "publication followed a replaced parent"
    );
    assert_eq!(std::fs::read(moved.join("file")).unwrap(), content);
    assert_eq!(std::fs::read_dir(moved).unwrap().count(), 1);
    server.await.unwrap();
}
