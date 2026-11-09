//! Snapshot one already-built matching ZXS executable. Never download or run it.
use sha2::{Digest, Sha256};
use std::{env, fs, io::Read, path::PathBuf};

#[path = "build/materials.rs"]
mod materials;

const MAX_PAYLOAD: u64 = 256 * 1024 * 1024;
const TARGET: &str = "x86_64-unknown-linux-gnu";
fn required(name: &str) -> String {
    env::var(name).unwrap_or_else(|_| panic!("{name} is required when embedding the ZXS payload"))
}
fn object_id(name: &str) -> String {
    let text = required(name);
    assert!(
        (text.len() == 40 || text.len() == 64)
            && text
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)),
        "{name} must be a canonical Git object ID"
    );
    text
}
fn main() {
    for name in [
        "ZXS_HELPER_PAYLOAD",
        "ZXS_HELPER_PAYLOAD_COMMIT",
        "ZXS_HELPER_PAYLOAD_TREE",
        "ZXS_HELPER_PAYLOAD_VERSION",
        "ZXS_HELPER_PAYLOAD_SHA256",
    ] {
        println!("cargo:rerun-if-env-changed={name}");
    }
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=build/materials.rs");
    for name in materials::VARIABLES {
        println!("cargo:rerun-if-env-changed={name}");
    }
    let out = PathBuf::from(required("OUT_DIR"));
    let Some(path) = env::var_os("ZXS_HELPER_PAYLOAD") else {
        materials::absent(&out);
        for name in [
            "ZXS_HELPER_PAYLOAD_COMMIT",
            "ZXS_HELPER_PAYLOAD_TREE",
            "ZXS_HELPER_PAYLOAD_VERSION",
            "ZXS_HELPER_PAYLOAD_SHA256",
        ] {
            assert!(
                env::var_os(name).is_none(),
                "partial embedded-payload configuration: {name}"
            );
        }
        fs::write(
            out.join("install_payload.rs"),
            "pub(super) const EMBEDDED: Option<Payload> = None;\n",
        )
        .expect("write explicit development payload absence");
        return;
    };
    assert_eq!(
        required("TARGET"),
        TARGET,
        "embedded installer payload currently supports Linux amd64 GNU only"
    );
    let version = required("ZXS_HELPER_PAYLOAD_VERSION");
    assert_eq!(
        version,
        required("CARGO_PKG_VERSION"),
        "ZXS and helper package versions must match"
    );
    let commit = object_id("ZXS_HELPER_PAYLOAD_COMMIT");
    let tree = object_id("ZXS_HELPER_PAYLOAD_TREE");
    let path = PathBuf::from(path);
    assert!(path.is_absolute(), "payload path must be absolute");
    let metadata = fs::symlink_metadata(&path).expect("read payload metadata");
    assert!(
        metadata.file_type().is_file() && metadata.len() >= 64 && metadata.len() <= MAX_PAYLOAD,
        "payload must be one bounded regular ELF executable, not a bundle or symlink"
    );
    let mut bytes = Vec::new();
    fs::File::open(&path)
        .expect("open payload")
        .take(MAX_PAYLOAD + 1)
        .read_to_end(&mut bytes)
        .expect("read payload");
    assert_eq!(
        bytes.len() as u64,
        metadata.len(),
        "payload changed while being read"
    );
    assert!(
        bytes[..7] == *b"\x7fELF\x02\x01\x01"
            && u16::from_le_bytes([bytes[18], bytes[19]]) == 62
            && matches!(u16::from_le_bytes([bytes[16], bytes[17]]), 2 | 3),
        "payload must be an ELF64 little-endian x86_64 executable"
    );
    let digest = format!("{:x}", Sha256::digest(&bytes));
    assert_eq!(
        digest,
        required("ZXS_HELPER_PAYLOAD_SHA256"),
        "payload bytes differ from the exact ZXS build result"
    );
    println!("cargo:rerun-if-changed={}", path.display());
    fs::write(out.join("payload-zxs"), &bytes).expect("snapshot payload bytes");
    let generated = format!(
        "pub(super) const EMBEDDED: Option<Payload> = Some(Payload {{ bytes: include_bytes!(concat!(env!(\"OUT_DIR\"), \"/payload-zxs\")), version: {version:?}, commit: {commit:?}, tree: {tree:?}, target: {TARGET:?}, sha256: {digest:?} }});\n"
    );
    fs::write(out.join("install_payload.rs"), generated).expect("write embedded payload identity");
    materials::prepare(&out, materials::Binding {
        version: &version, commit: &commit, tree: &tree, target: TARGET, payload_sha256: &digest,
    });
}
