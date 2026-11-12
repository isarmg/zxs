use crate::Client;
use json_codec::{Value, json};
use storage_contract::file_container_tags::{Context, ReadRequest, ReplaceCommand, Snapshot, TagSet};
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
async fn container_tag_reads_encode_context_and_reject_other_container_responses() {
    let request = request();
    for changed in [false, true] {
        let response = Snapshot {
            file_id: if changed {
                FileId::from_random([3; 16])
            } else {
                request.file_id
            },
            context: request.context.clone(),
            tags: TagSet::empty(),
            generation: Generation::new(0),
        };
        let (client, task) = capture(json!(response)).await;
        assert_eq!(client.file_container_tags(&request).await.is_err(), changed);
        let (headers, body) = task.await.unwrap();
        let query = url_encoding::form::Serializer::new(String::new())
            .append_pair("repository_id", &request.context.repository_id.to_string())
            .append_pair("path", request.context.path.as_str())
            .finish();
        assert!(headers.starts_with(&format!(
            "GET /api/file-manager/files/{}/tags?{query} HTTP/1.1\r\n",
            request.file_id,
        )));
        assert!(body.is_empty());
    }
}

#[tokio::test]
async fn container_tag_retries_keep_the_complete_cas_command_and_operation_identity() {
    let read = request();
    let command = ReplaceCommand {
        file_id: read.file_id,
        context: read.context,
        expected_generation: Generation::new(9),
        tags: TagSet::empty(),
    };
    let operation = OperationId::from_random([4; 16]);
    let expected = json!({
        "repository_id": command.context.repository_id,
        "path": command.context.path,
        "expected_generation": "9",
        "tags": [],
        "operation_id": operation,
    });
    for _ in 0..2 {
        let (client, task) = capture(json!({"state":"in_doubt"})).await;
        client
            .replace_container_tags(&command, operation)
            .await
            .unwrap();
        let (headers, body) = task.await.unwrap();
        assert!(headers.starts_with(&format!(
            "POST /api/file-manager/files/{}/tags HTTP/1.1\r\n",
            command.file_id,
        )));
        assert_eq!(json_codec::from_slice::<Value>(&body).unwrap(), expected);
    }
}
