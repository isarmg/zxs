use crate::Client;
use json_codec::{Value, json};
use storage_contract::file_container_tags::{Context, ReadRequest};
use storage_contract::{FileId, Generation, OperationId, RepositoryId};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

fn request() -> ReadRequest {
    ReadRequest {
        file_id: FileId::from_random([1; 16]),
        context: Context {
            repository_id: RepositoryId::from_random([2; 16]),
            path: json_codec::from_value(json!("资料/a & b.txt")).unwrap(),
        },
    }
}

async fn capture(reply: Value) -> (Client, tokio::task::JoinHandle<(String, Vec<u8>)>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = Client::connect(&format!("http://{}", listener.local_addr().unwrap())).unwrap();
    let task = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut headers = Vec::new();
        while !headers.ends_with(b"\r\n\r\n") {
            headers.push(stream.read_u8().await.unwrap());
            assert!(headers.len() < 8192);
        }
        let headers = String::from_utf8(headers).unwrap();
        let length = headers
            .lines()
            .filter_map(|line| line.split_once(':'))
            .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .map(|(_, value)| value.trim().parse::<usize>().unwrap())
            .unwrap_or(0);
        assert!(length < 8192);
        let mut body = vec![0; length];
        stream.read_exact(&mut body).await.unwrap();
        let reply = json_codec::to_vec(&reply).unwrap();
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n",
            reply.len(),
        );
        stream.write_all(head.as_bytes()).await.unwrap();
        stream.write_all(&reply).await.unwrap();
        (headers, body)
    });
    (client, task)
}

#[tokio::test]
async fn mode_read_and_setting_preserve_exact_context_generation_and_operation() {
    let request = request();
    let (client, task) = capture(json!({"state":"automatic","generation":"0"})).await;
    assert_eq!(client.container_edit(&request).await.unwrap(),
        storage_contract::container_edit::ContainerEdit::initial());
    let (headers, body) = task.await.unwrap();
    let query = url_encoding::form::Serializer::new(String::new())
        .append_pair("repository", &request.context.repository_id.to_string())
        .append_pair("path", request.context.path.as_str()).finish();
    assert!(headers.starts_with(&format!(
        "GET /api/file-manager/files/{}/edit-mode?{query} HTTP/1.1\r\n", request.file_id)));
    assert!(body.is_empty());
    let operation = OperationId::from_random([3; 16]);
    let (client, task) = capture(json!({"operation_id":operation})).await;
    client.set_container_commit_mode(&request, Generation::new(9),
        storage_contract::container_edit::CommitMode::Manual, operation).await.unwrap();
    let (headers, body) = task.await.unwrap();
    assert!(headers.starts_with(&format!(
        "POST /api/file-manager/files/{}/edit-mode HTTP/1.1\r\n", request.file_id)));
    assert_eq!(json_codec::from_slice::<Value>(&body).unwrap(), json!({
        "repository":request.context.repository_id,"path":request.context.path,
        "expected_generation":"9","mode":"manual","operation_id":operation,
    }));
}

#[tokio::test]
async fn mode_read_does_not_treat_unknown_or_malformed_state_as_automatic() {
    for response in [json!({}), json!({"state":"automatic","generation":-1}),
        json!({"state":"manual_dirty","generation":"2"})] {
        let (client, task) = capture(response).await;
        assert!(client.container_edit(&request()).await.is_err());
        task.await.unwrap();
    }
}
