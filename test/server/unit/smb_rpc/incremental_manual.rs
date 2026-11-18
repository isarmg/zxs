//! Production native dispatcher plus authenticated TCP HTTP. No AF_UNIX, FD
//! transfer, Samba daemon or network SMB execution is claimed by this test.
use super::{AuthStore, Backend, Config, Direct, Engine, Reply};
use json_codec::json;
use smb_protocol::protocol::{DirectoryEntry, OpenIo, Request, WorkingPosition};
use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::os::unix::fs::{FileExt, MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use storage_contract::OperationId;
use storage_contract::file_draft::manifest::{WORKING_IO_BYTES, packed::{PackId, CheckedIndex, VERIFY_CHUNK, physical_bytes_for}};
use storage_contract::file_draft::{DraftLocation, FileWorkingState};

#[path = "incremental_http.rs"]
mod http;
use http::Http;

fn bytes(client: &mut Direct, handle: u64, length: usize) -> Vec<u8> {
    let mut output = vec![0; length];
    let mut offset = 0;
    while offset < output.len() {
        let end = (offset + WORKING_IO_BYTES as usize).min(output.len());
        let Reply::Read(bytes) = client.call(Request::Read {
            handle,
            offset: offset as u64,
            length: (end - offset) as u32,
        }) else {
            panic!("Read must return held content bytes");
        };
        assert!(!bytes.is_empty());
        output[offset..offset + bytes.len()].copy_from_slice(&bytes);
        offset += bytes.len();
    }
    let Reply::Read(end) = client.call(Request::Read {
        handle,
        offset: length as u64,
        length: 1,
    }) else {
        panic!("Read must preserve EOF");
    };
    assert!(end.is_empty());
    output
}

fn extents(root: &Path) -> BTreeMap<PackId, PathBuf> {
    let mut result = BTreeMap::new();
    let mut pending = vec![root.to_owned()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(directory).unwrap() {
            let entry = entry.unwrap();
            let kind = entry.file_type().unwrap();
            if kind.is_dir() {
                pending.push(entry.path());
            }
            if kind.is_file()
                && let Some(name) = entry.file_name().to_str()
                && let Ok(id) = name.parse::<PackId>()
            {
                assert!(result.insert(id, entry.path()).is_none());
            }
        }
    }
    result
}

#[test]
fn ordinary_manual_ranges_and_tcp_commit_retain_exact_base_draft_and_version_bytes() {
    let root = tempfile::tempdir().unwrap();
    let system = root.path().join("state");
    let mut entropy = [0; 32];
    system_entropy::fill(&mut entropy).unwrap();
    let password: String = entropy.iter().map(|byte| format!("{byte:02x}")).collect();
    let engine = Engine::initialize(
        &system,
        Config::development([(
            name_policy::Name::parse("p").unwrap(),
            root.path().join("mounted-primary"),
        )])
        .unwrap(),
    )
    .unwrap();
    let auth = AuthStore::initialize(&system, "alice", &password).unwrap();
    let spool = root.path().join("spool");
    fs::create_dir(&spool).unwrap();
    fs::set_permissions(&spool, fs::Permissions::from_mode(0o700)).unwrap();
    fs::create_dir(spool.join("empty-directory")).unwrap();
    let mut client = Direct::start_with_limit(root.path(), engine, auth, 1024 * 1024);
    let original: Vec<_> = (0..8 * WORKING_IO_BYTES)
        .map(|index| (index % 251) as u8)
        .collect();
    let (new, placeholder) = client.open("p/incremental.bin", true, true);
    assert_eq!(new.data_io, OpenIo::NativeWorking);
    for (index, block) in original.chunks(WORKING_IO_BYTES as usize).enumerate() {
        client.write(
            new.handle,
            WorkingPosition::At {
                offset: index as u64 * WORKING_IO_BYTES,
            },
            block,
        );
    }
    client.call(Request::Close { handle: new.handle });
    drop(placeholder);
    let (directory, _) = client.open("p", false, false);
    let Reply::Directory(page) = client.call(Request::ReadDirectory {
        handle: directory.handle,
        restart: true,
    }) else {
        panic!("directory dispatch must retain the current occurrence");
    };
    let DirectoryEntry::Unborn { target, .. } = &page.entries[0] else {
        panic!("ordinary first save must remain Manual and unborn");
    };
    let target: DraftLocation = target.clone();
    client.call(Request::Close {
        handle: directory.handle,
    });
    drop(client);
    let engine = Engine::open(&system).unwrap();
    assert!(engine.state().versions.is_empty());
    let web = Http::start(&system, engine, &password);
    let query = url_encoding::form::Serializer::new(String::new())
        .append_pair("repository_id", &target.repository.to_string())
        .append_pair("path", target.path.as_str())
        .append_pair("entry_id", &target.entry.to_string())
        .finish();
    let route = format!("/api/file-manager/files/{}/working", target.file);
    let unborn = web.json(
        "GET",
        &format!(
            "/api/file-manager/files/{}/unborn-working?{query}",
            target.file,
        ),
        None,
    );
    assert_eq!(unborn["mode"], "manual");
    web.json(
        "POST",
        &format!("/api/file-manager/files/{}/first-commit", target.file),
        Some(json!({
            "operation_id": OperationId::generate().unwrap(), "target": target,
            "expected": unborn["expected"], "message": "real baseline for range editing",
        })),
    );
    let engine = web.cold(&system);
    let base = engine.file_draft_base(&target).unwrap();
    let original_metadata = engine
        .open_content(&base.version_id().to_string())
        .unwrap()
        .file
        .metadata()
        .unwrap();
    let original_inode = (original_metadata.dev(), original_metadata.ino());
    assert_eq!(original_metadata.len(), original.len() as u64);
    assert_eq!(original_metadata.nlink(), 1);
    assert!(matches!(
        engine.file_working_state(&target).unwrap(),
        FileWorkingState::ManualClean { .. }
    ));
    assert_eq!(engine.state().versions.len(), 1);
    let prior_extents = extents(root.path());
    let auth = AuthStore::open(&system).unwrap();
    let mut client = Direct::start_with_limit(root.path(), engine, auth, 1024 * 1024);
    let (writer, placeholder) = client.open("p/incremental.bin", true, false);
    assert_eq!(writer.data_io, OpenIo::NativeWorking);
    assert_eq!(writer.version_id, Some(base.version_id().to_string()));
    assert!(placeholder.metadata().unwrap().is_dir());
    assert!(placeholder.write_all_at(b"not a mutable base", 0).is_err());
    let busy = client
        .share
        .dispatch(Request::Open {
            path: "p/incremental.bin".into(),
            writable: true,
            create: false,
            truncate: false,
            exclusive: false,
            write_through: false,
        })
        .unwrap_err();
    assert_eq!(
        busy.code,
        storage_contract::ErrorCode::FileWriteBusy,
        "a second writer cannot bypass the exact path reservation"
    );
    assert_eq!(bytes(&mut client, writer.handle, original.len()), original);
    assert_eq!(
        extents(root.path()),
        prior_extents,
        "open must not copy any working packs"
    );
    let mut first = original.clone();
    let offset = WORKING_IO_BYTES as usize - 3;
    let patch = b"explicit cross-block range";
    first[offset..offset + patch.len()].copy_from_slice(patch);
    client.write(
        writer.handle,
        WorkingPosition::At {
            offset: offset as u64,
        },
        patch,
    );
    let saved = client.flush(writer.handle);
    assert_eq!(client.flush(writer.handle), saved);
    let snapshot = client
        .backend
        .file_working(&client._auth.sole_identity().unwrap(), target.clone())
        .unwrap();
    assert_eq!(snapshot.base, base);
    let payload = *snapshot.working.draft().unwrap().1;
    assert_eq!(payload.content_hash(), None, "ordinary save never records a full body hash");
    assert_eq!(payload.manifest_commitment(),
        Some(snapshot.working.incremental().unwrap().material().commitment()));
    let persisted = json_codec::to_value(&snapshot.working).unwrap();
    assert!(persisted["payload"].get("content_hash").is_none());
    assert_eq!(snapshot.working.material_bytes(), physical_bytes_for(patch.len() as u64).unwrap());
    assert_eq!(
        snapshot
            .working
            .incremental()
            .unwrap()
            .material()
            .runs()
            .len(),
        1
    );
    let (pinned, _) = client.open("p/incremental.bin", false, false);
    let mut latest = first.clone();
    let offset = 5 * WORKING_IO_BYTES as usize + 11;
    latest[offset..offset + 4].fill(0);
    client.write(
        writer.handle,
        WorkingPosition::At {
            offset: offset as u64,
        },
        &[0; 4],
    );
    let second = client.flush(writer.handle);
    assert_ne!(second, saved);
    assert_eq!(client.flush(writer.handle), second);
    assert_eq!(
        bytes(&mut client, pinned.handle, first.len()),
        first,
        "old read retains its original save"
    );
    assert_eq!(bytes(&mut client, writer.handle, latest.len()), latest);
    let retained = WORKING_IO_BYTES as usize + 10;
    client.call(Request::WorkingResize {
        handle: writer.handle,
        length: retained as u64,
    });
    client.call(Request::WorkingResize {
        handle: writer.handle,
        length: original.len() as u64,
    });
    latest[retained..].fill(0);
    let offset = 5 * WORKING_IO_BYTES as usize + 3;
    latest[offset..offset + 3].copy_from_slice(b"end");
    client.write(
        writer.handle,
        WorkingPosition::At {
            offset: offset as u64,
        },
        b"end",
    );
    client.flush(writer.handle);
    assert_eq!(bytes(&mut client, writer.handle, latest.len()), latest);
    let snapshot = client
        .backend
        .file_working(&client._auth.sole_identity().unwrap(), target.clone())
        .unwrap();
    assert_eq!(snapshot.base, base);
    let payload = *snapshot.working.draft().unwrap().1;
    assert_eq!(payload.content_hash(), None, "ordinary save never records a full body hash");
    assert_eq!(payload.manifest_commitment(),
        Some(snapshot.working.incremental().unwrap().material().commitment()));
    let persisted = json_codec::to_value(&snapshot.working).unwrap();
    assert!(persisted["payload"].get("content_hash").is_none());
    assert_eq!(snapshot.working.material_bytes(), physical_bytes_for(patch.len() as u64).unwrap()
        + physical_bytes_for(3).unwrap());
    let material = snapshot.working.incremental().unwrap().material();
    let after_extents = extents(root.path());
    assert_eq!(after_extents.len() - prior_extents.len(), 2);
    for &extent in material.packs() {
        let path = &after_extents[&extent.id()];
        let metadata = fs::metadata(path).unwrap();
        assert_eq!(
            (metadata.dev(), metadata.ino()),
            (extent.entity().device(), extent.entity().inode())
        );
        assert_eq!(metadata.nlink(), 1);
        assert_eq!(metadata.len(), extent.physical_bytes());
        let mut data = fs::read(path).unwrap();
        let trailer = data.split_off(extent.data_bytes() as usize);
        let index = CheckedIndex::from_trailer(extent, trailer).unwrap();
        for (n, part) in data.chunks(VERIFY_CHUNK as usize).enumerate() {
            assert_eq!(index.expected_chunk(n as u64), Some(*blake3::hash(part).as_bytes()));
        }
    }
    // Disconnect a successfully flushed writer without manufacturing a second save.
    client.call(Request::Close {
        handle: pinned.handle,
    });
    drop(placeholder);
    drop(client);
    let engine = Engine::open(&system).unwrap();
    assert_eq!(engine.state().versions.len(), 1);
    assert_eq!(engine.file_draft_base(&target).unwrap(), base);
    assert_eq!(
        engine.file_working_state(&target).unwrap(),
        snapshot.working
    );
    let mut original_read = Vec::new();
    engine
        .open_content(&base.version_id().to_string())
        .unwrap()
        .file
        .read_to_end(&mut original_read)
        .unwrap();
    assert_eq!(original_read, original);
    let mut draft = Vec::new();
    engine
        .open_file_draft(&target)
        .unwrap()
        .read_to_end(&mut draft)
        .unwrap();
    assert_eq!(draft, latest);
    let web = Http::start(&system, engine, &password);
    assert_eq!(
        web.bytes(&format!(
            "/api/file-manager/files/{}/draft?{query}",
            target.file
        )),
        latest
    );
    let draft_route = format!("/api/file-manager/files/{}/draft?{query}", target.file);
    for (start, end) in [(3, 21), (WORKING_IO_BYTES as usize - 8,
        WORKING_IO_BYTES as usize + 30), (3 * WORKING_IO_BYTES as usize,
        3 * WORKING_IO_BYTES as usize + 27)]
    {
        let (etag, body) = web.range(&draft_route, start, end, latest.len());
        assert_eq!(body, latest[start..=end]);
        assert_eq!(etag, format!("\"working-manifest-{}\"", material.commitment()));
        assert_ne!(etag, format!("\"{}\"", blake3::hash(&latest).to_hex()));
    }
    let working = web.json("GET", &format!("{route}?{query}"), None);
    let command = json!({
        "operation_id": OperationId::generate().unwrap(),
        "command": { "action": "commit", "target": target,
            "expected": working["working"]["revision"], "base": working["base"],
            "message": "explicit immutable merged version" },
    });
    let committed = web.json("POST", &route, Some(command.clone()));
    assert_eq!(web.json("POST", &route, Some(command)), committed);
    let engine = web.cold(&system);
    assert!(matches!(
        engine.file_working_state(&target).unwrap(),
        FileWorkingState::ManualClean { .. }
    ));
    assert_eq!(engine.state().versions.len(), 2);
    let new_base = engine.file_draft_base(&target).unwrap();
    assert_eq!(engine.state().versions[&new_base.version_id().to_string()].content_hash,
        storage_contract::ContentHash::from_digest(blake3::hash(&latest)),
        "only the explicit independent FULL carries the actual body hash");
    assert_eq!(new_base.branch_id(), base.branch_id());
    assert_ne!(new_base.version_id(), base.version_id());
    let mut committed_bytes = Vec::new();
    let mut committed_file = engine
        .open_content(&new_base.version_id().to_string())
        .unwrap()
        .file;
    let metadata = committed_file.metadata().unwrap();
    assert_eq!(metadata.len(), latest.len() as u64);
    assert_eq!(metadata.nlink(), 1);
    assert_ne!((metadata.dev(), metadata.ino()), original_inode);
    for pack in material.packs() {
        let entity = pack.entity();
        assert_ne!(
            (metadata.dev(), metadata.ino()),
            (entity.device(), entity.inode())
        );
    }
    committed_file.read_to_end(&mut committed_bytes).unwrap();
    drop(committed_file);
    assert_eq!(committed_bytes, latest);
    original_read.clear();
    let metadata = engine
        .open_content(&base.version_id().to_string())
        .unwrap()
        .file
        .metadata()
        .unwrap();
    assert_eq!((metadata.dev(), metadata.ino()), original_inode);
    engine
        .open_content(&base.version_id().to_string())
        .unwrap()
        .file
        .read_to_end(&mut original_read)
        .unwrap();
    assert_eq!(original_read, original);
    // A normal identical-byte save still gets a new range identity. The one
    // explicit commit comparison clears it without allocating a third Version.
    let auth = AuthStore::open(&system).unwrap();
    let mut client = Direct::start_with_limit(root.path(), engine, auth, 1024 * 1024);
    let (writer, placeholder) = client.open("p/incremental.bin", true, false);
    client.write(writer.handle, WorkingPosition::At { offset: 4 }, &latest[4..17]);
    client.flush(writer.handle);
    let same = client.backend.file_working(&client._auth.sole_identity().unwrap(),
        target.clone()).unwrap();
    assert_eq!(same.working.draft().unwrap().1.content_hash(), None);
    client.call(Request::Close { handle: writer.handle });
    drop(placeholder);
    drop(client);
    let engine = Engine::open(&system).unwrap();
    assert_eq!(engine.state().versions.len(), 2);
    let web = Http::start(&system, engine, &password);
    let before = web.json("GET", &format!("{route}?{query}"), None);
    let same_commit = json!({
        "operation_id": OperationId::generate().unwrap(),
        "command": { "action": "commit", "target": target,
            "expected": before["working"]["revision"], "base": before["base"],
            "message": "explicit equal-base comparison" },
    });
    let retained = web.json("POST", &route, Some(same_commit.clone()));
    assert_eq!(web.json("POST", &route, Some(same_commit)), retained);
    let engine = web.cold(&system);
    assert_eq!(engine.state().versions.len(), 2);
    assert_eq!(engine.file_draft_base(&target).unwrap(), new_base);
    assert!(matches!(engine.file_working_state(&target).unwrap(),
        FileWorkingState::ManualClean { .. }));
    drop(engine);
    drop(root);
}
