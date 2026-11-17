//! Real default-share RPC against the native no-history container, then HTTP first commit.
use super::{Fixture, TestCredentials, close, open};
use super::manual_draft::Management;
use json_codec::json;
use smb_protocol::client::Client;
use smb_protocol::protocol::{DirectoryEntry, OpenIo, Request, WorkingPosition};
use std::io::Read;
use std::os::unix::fs::FileExt;
use storage_contract::OperationId;
use storage_contract::file_draft::DraftLocation;

fn listing(client: &Client, expected_size: u64) -> DraftLocation {
    let (directory, descriptor) = open(client, "p", false, false, false);
    assert_eq!(directory.data_io, OpenIo::Directory);
    assert!(descriptor.metadata().unwrap().is_dir());
    let page = client.read_directory(directory.handle, true).unwrap();
    assert!(!page.more);
    let item = page.entries.iter().find(|entry| entry.path().as_str() == "first.txt").unwrap();
    let DirectoryEntry::Unborn { target, logical_size } = item else {
        panic!("an unsaved/working file must not manufacture a historical Entry");
    };
    assert_eq!(*logical_size, expected_size);
    let wire = json_codec::to_value(item).unwrap();
    assert!(wire.get("version_id").is_none() && wire.get("entry").is_none());
    let target = target.clone();
    close(client, directory.handle);
    target
}

#[test]
fn native_default_share_ordered_io_stays_unborn_until_cold_http_first_commit() {
    let credentials = TestCredentials::generated();
    let mut fixture = Fixture::with_credentials(&credentials);
    let client = fixture.connect();
    let (opened, placeholder) = open(&client, "p/first.txt", true, true, false);
    assert_eq!(opened.data_io, OpenIo::NativeWorking);
    assert!(opened.version_id.is_none() && opened.workspace_id.is_none());
    assert_eq!(opened.size, 0);
    assert!(placeholder.metadata().unwrap().is_dir());
    assert!(placeholder.write_all_at(b"must not be an application BODY FD", 0).is_err());
    let target = listing(&client, 0);
    assert_eq!(target.entry.to_string(), opened.entry_id);
    assert_eq!(client.stat("p/first.txt".into()).unwrap().size, 0);
    assert_eq!(client.read(opened.handle, 0, &mut [0xff; 1]).unwrap(), 0);
    assert_eq!(client.write_working(opened.handle,
        WorkingPosition::At { offset: 0 }, b"abcdef").unwrap(), 6);
    assert_eq!(client.write_working(opened.handle, WorkingPosition::Append {}, b"GH").unwrap(), 2);
    client.resize_working(opened.handle, 4).unwrap();
    client.resize_working(opened.handle, 10).unwrap();
    client.write_working(opened.handle, WorkingPosition::At { offset: 7 }, b"Z").unwrap();
    let expected = b"abcd\0\0\0Z\0\0";
    let mut actual = [0xff; 12];
    assert_eq!(client.read(opened.handle, 0, &mut actual).unwrap(), expected.len());
    assert_eq!(&actual[..expected.len()], expected);
    assert_eq!(client.stat_working(opened.handle).unwrap().size, expected.len() as u64);
    // Pwrite completion is not an acknowledged Save. Current enumeration sees
    // only the admitted empty body until the real native publication succeeds.
    assert_eq!(listing(&client, 0), target);
    let receipt = client.call(Request::Flush { handle: opened.handle }).unwrap();
    assert_eq!(client.call(Request::Flush { handle: opened.handle }).unwrap(), receipt);
    client.write_working(opened.handle, WorkingPosition::At { offset: 9 }, b"Q").unwrap();
    let second = client.call(Request::Flush { handle: opened.handle }).unwrap();
    assert_ne!(second, receipt);
    assert_eq!(client.call(Request::Flush { handle: opened.handle }).unwrap(), second);
    let expected = b"abcd\0\0\0Z\0Q";
    close(&client, opened.handle);
    assert_eq!(listing(&client, expected.len() as u64), target);
    drop(placeholder);
    drop(client);
    let (root, engine) = fixture.cold_open();
    assert!(engine.state().versions.is_empty());
    assert!(engine.state().branches.values().all(|branch| branch.file_id.is_none()));
    assert!(engine.state().operations.values().all(|result| result.kind != "workspace.commit"));
    let mut fixture = Fixture::resume(root, engine);
    let client = fixture.connect();
    let (read, descriptor) = open(&client, "p/first.txt", false, false, false);
    assert_eq!(read.data_io, OpenIo::NativeWorking);
    assert!(descriptor.metadata().unwrap().is_dir());
    actual.fill(0xff);
    assert_eq!(client.read(read.handle, 0, &mut actual).unwrap(), expected.len());
    assert_eq!(&actual[..expected.len()], expected);
    assert!(client.write_working(read.handle, WorkingPosition::At { offset: 0 }, b"!").is_err());
    actual.fill(0xff);
    assert_eq!(client.read(read.handle, 0, &mut actual).unwrap(), expected.len());
    assert_eq!(&actual[..expected.len()], expected);
    close(&client, read.handle);

    // An actual second publication invalidates an editing occurrence. Neither
    // Flush nor Close may bind its uncommitted extents to a newer State Arc.
    let (stale, _) = open(&client, "p/first.txt", true, false, false);
    client.write_working(stale.handle, WorkingPosition::At { offset: 0 }, b"X").unwrap();
    let (other, _) = open(&client, "p/other.txt", true, true, false);
    close(&client, other.handle);
    assert!(client.call(Request::Flush { handle: stale.handle }).is_err());
    assert!(client.call(Request::Close { handle: stale.handle }).is_err());
    drop(descriptor);
    drop(client);
    let (root, engine) = fixture.cold_open();
    assert!(engine.state().versions.is_empty());
    let web = Management::open(root, engine, &credentials);
    let query = url_encoding::form::Serializer::new(String::new())
        .append_pair("repository_id", &target.repository.to_string())
        .append_pair("path", target.path.as_str())
        .append_pair("entry_id", &target.entry.to_string()).finish();
    let status = web.json("GET", &format!(
        "/api/file-manager/files/{}/unborn-working?{query}", target.file,
    ), None);
    assert_eq!(status["mode"], "manual");
    assert_eq!(status["logical_size"], expected.len().to_string());
    assert_eq!(status["expected"]["content"]["state"], "saved");
    let command = json!({
        "operation_id": OperationId::generate().unwrap(), "target": target,
        "expected": status["expected"], "message": "first explicit native history",
    });
    let route = format!("/api/file-manager/files/{}/first-commit", target.file);
    let committed = web.json("POST", &route, Some(command.clone()));
    let (root, engine) = web.cold_open();
    let versions = engine.state().versions.values()
        .filter(|version| version.file_id == target.file.to_string()).collect::<Vec<_>>();
    assert_eq!(versions.len(), 1);
    let mut body = Vec::new();
    engine.open_content(&versions[0].version_id).unwrap().file.read_to_end(&mut body).unwrap();
    assert_eq!(body, expected);
    let version = versions[0].version_id.clone();
    let web = Management::open(root, engine, &credentials);
    assert_eq!(web.json("POST", &route, Some(command)), committed);
    let (root, engine) = web.cold_open();
    assert_eq!(engine.state().versions.len(), 1);
    assert!(engine.state().versions.contains_key(&version));
    drop(engine);
    drop(root);
}
