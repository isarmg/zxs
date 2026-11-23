use super::*;

fn sample() -> Ticket {
    Ticket {
        seed: [1; 16],
        pool: [2; 16],
        token: [3; 32],
        target: b"/zxs/disks/data-second".to_vec(),
        source_root: b"/external".to_vec(),
        relative: b"folder/old\xff".to_vec(),
        source: Witness {
            entity: Entity { dev: 11, ino: 12 },
            uid: 1000,
            mode: 0o100_644,
            links: 2,
            modified: (13, 14),
            changed: (15, 16),
            length: 17,
        },
        sha256: [8; 32],
        name: "报告".as_bytes().to_vec(),
    }
}

#[test]
fn the_ticket_preserves_byte_exact_source_and_does_not_infer_a_name() {
    let original = sample();
    let bytes = original.encode().unwrap();
    let decoded = Ticket::decode(&bytes).unwrap();
    assert_eq!(decoded.relative.as_slice(), b"folder/old\xff");
    assert_eq!(decoded.name, "报告".as_bytes());
    assert_eq!(decoded.source, original.source);
    assert_eq!(decoded.encode().unwrap(), bytes);
    for cut in 0..bytes.len() {
        assert!(Ticket::decode(&bytes[..cut]).is_err());
    }
    let mut trailing = bytes;
    trailing.push(0);
    assert!(Ticket::decode(&trailing).is_err());
}

#[test]
fn symbolic_traversal_is_not_a_canonical_source_path() {
    let mut ticket = sample();
    for relative in [b"a/../b".as_slice(), b"/a", b"a//b", b"./b", b"a/\0"] {
        ticket.relative = relative.to_vec();
        assert!(ticket.encode().is_err());
    }
}

#[test]
fn both_absolute_paths_obey_the_ticket_limit() {
    let mut path = Vec::new();
    for _ in 0..15 {
        path.push(b'/');
        path.extend_from_slice(&[b'a'; 255]);
    }
    path.push(b'/');
    path.extend_from_slice(&[b'b'; 254]);
    let mut ticket = sample();
    ticket.target.clone_from(&path);
    ticket.source_root.clone_from(&path);
    assert_eq!(path.len(), PATH_BYTES);
    assert!(Ticket::decode(&ticket.encode().unwrap()).is_ok());
    ticket.target.push(b'c');
    assert!(ticket.encode().is_err());
    ticket.target.pop();
    ticket.source_root.push(b'c');
    assert!(ticket.encode().is_err());
}
