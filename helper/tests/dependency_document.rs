use sha2::{Digest, Sha256};

#[test]
fn dependency_document_matches_the_exact_lockfile() {
    let digest = Sha256::digest(include_bytes!("../Cargo.lock"));
    let expected = format!("Cargo.lock SHA-256: `{digest:x}`");
    assert!(
        include_str!("../DEPENDENCIES.md")
            .lines()
            .any(|line| line == expected),
        "update the dependency inventory and lock digest: {expected}"
    );
}
