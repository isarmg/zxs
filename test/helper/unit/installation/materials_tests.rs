use super::*;

fn checksum(bytes: &[u8]) -> &'static str {
    Box::leak(platform_io::lower_hex(&Sha256::digest(bytes)).into_boxed_str())
}
fn fixture() -> (Embedded, super::super::payload::Payload) {
    let payload = super::super::payload::Payload {
        bytes: b"bounded payload fixture",
        version: "fixture",
        commit: "source",
        tree: "tree",
        target: "x86_64-unknown-linux-gnu",
        sha256: "payload-digest",
    };
    let archive = b"\x1f\x8b\x08bounded byte-binding fixture, not an executable archive";
    let manifest = b"{\"scope\":\"fixture only\"}\n";
    (
        Embedded {
            archive,
            manifest,
            archive_sha256: checksum(archive),
            manifest_sha256: checksum(manifest),
            version: payload.version,
            commit: payload.commit,
            tree: payload.tree,
            target: payload.target,
            payload_sha256: payload.sha256,
        },
        payload,
    )
}

#[test]
fn verified_material_bytes_bind_payload_source_and_exact_manifest_without_extraction() {
    let (materials, payload) = fixture();
    let verified = materials.verify(payload).unwrap();
    assert_eq!(verified.archive, materials.archive);
    assert_eq!(verified.manifest, materials.manifest);
    let value = json_codec::to_value(&verified.metadata).unwrap();
    assert_eq!(value["legal_completeness_attested"], false);
    assert_eq!(value["payload_sha256"], payload.sha256);
}

#[test]
fn altered_materials_or_payload_identity_never_produce_a_verified_export() {
    let (materials, payload) = fixture();
    for altered in [
        Embedded {
            archive: b"different",
            ..materials
        },
        Embedded {
            manifest: b"different",
            ..materials
        },
        Embedded {
            commit: "another source",
            ..materials
        },
        Embedded {
            payload_sha256: "another program",
            ..materials
        },
    ] {
        assert!(altered.verify(payload).is_err());
    }
}

#[test]
fn development_absence_never_downloads_or_creates_a_material_file() {
    if MATERIALS.is_none() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("materials.tar.gz");
        assert!(export(&path).is_err());
        assert!(!path.exists());
        assert!(metadata().is_none());
    }
}
