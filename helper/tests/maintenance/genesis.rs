use super::*;

// Rows and metadata are independent protocol fixtures, not observations of host storage.
fn wire(name: &str) -> Vec<u8> {
    let source = include_str!("vector.txt");
    let hex = source.lines().find_map(|line| {
        let (key, value) = line.split_once('=')?;
        (key == name).then_some(value)
    }).expect("named common wire fixture");
    assert!(hex.is_ascii() && hex.len().is_multiple_of(2));
    (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect()
}

fn sign(body: &[u8]) -> Vec<u8> {
    let mut bytes = body.to_vec();
    bytes.extend_from_slice(&Sha256::digest(body));
    bytes
}

#[test]
fn independent_four_root_vector_accepts_exact_initial_bytes_and_all_root_labels() {
    let image = wire("genesis");
    let g = FreshGenesis::decode(&image).unwrap();
    assert_eq!(g.digest.as_slice(), wire("sha256_prefix"));
    assert_eq!(g.pool, [0x11; 16]);
    assert_eq!(g.operation, [0x22; 16]);
    assert_eq!(g.storage_uid, 1001);
    assert_eq!((g.registration.device, g.registration.inode), (0x801, 100));
    assert_eq!(g.volumes.len(), 4);
    assert!(g.volumes.iter().all(|v| v.name_max == 255));
    assert_eq!(g.volumes.iter().map(|v| v.path).collect::<Vec<_>>(),
        vec![&b"/m/sys"[..], b"/m/bin", b"/m/data", b"/m/\xffvol"]);
    let labels = [wire("label_system"), wire("label_recycle"), wire("label_file_0"), wire("label_file_1")];
    let refs = labels.each_ref().map(Vec::as_slice);
    g.check_labels(&refs).unwrap();
    g.check_initial_frontier(Some(&wire("lower_bound"))).unwrap();
}

#[test]
fn missing_old_reordered_or_extended_attributes_do_not_authenticate_initial_bytes() {
    let image = wire("genesis");
    let g = FreshGenesis::decode(&image).unwrap();
    let labels = [wire("label_system"), wire("label_recycle"), wire("label_file_0"), wire("label_file_1")];
    let refs = labels.each_ref().map(Vec::as_slice);
    assert_eq!(g.check_labels(&refs[..3]), Err(Invalid::Label));
    assert_eq!(g.check_labels(&[refs[0], refs[1], refs[3], refs[2]]), Err(Invalid::Label));
    for i in 0..4 {
        // Old magic or changed role, pool, volume or operation is independently refused.
        for pos in [0, VOLUME.len(), VOLUME.len()+1, VOLUME.len()+17, VOLUME.len()+33] {
            let mut damaged = labels[i].clone(); damaged[pos] ^= 1;
            let mut chosen = refs; chosen[i] = &damaged;
            assert_eq!(g.check_labels(&chosen), Err(Invalid::Label));
        }
        let mut changed = labels[i].clone();
        if i == 0 { changed.truncate(changed.len()-32); } else { changed.extend_from_slice(&g.digest); }
        let mut chosen = refs; chosen[i] = &changed;
        assert_eq!(g.check_labels(&chosen), Err(Invalid::Label));
    }
    let old: Vec<_> = b"ZXS/VOLUME/ID-LEAF-1\0".iter().chain(&labels[0][VOLUME.len()..]).copied().collect();
    assert_eq!(g.check_labels(&[&old, refs[1], refs[2], refs[3]]), Err(Invalid::Label));
    let frontier = wire("lower_bound");
    assert_eq!(g.check_initial_frontier(None), Err(Invalid::Frontier));
    assert_eq!(g.check_initial_frontier(Some(&frontier[..47])), Err(Invalid::Frontier));
    for i in [7, 15, 16, 47] {
        let mut incorrect = frontier.clone(); incorrect[i] ^= 1;
        assert_eq!(g.check_initial_frontier(Some(&incorrect)), Err(Invalid::Frontier));
    }
}

// Offsets of the documented fixture's fields, computed from its own byte lengths
// (not production parsing code); reject a semantically wrong but correctly hashed image.
#[derive(Clone, Copy)]
struct Row { start: usize, limit: usize, path: usize, end: usize }
fn fields(body: &[u8]) -> (usize, usize, Vec<Row>) {
    let origin = GENESIS.len() + 16 + 16 + 4;
    let count = origin + 17 + usize::from(body[origin+16]);
    let mut start = count + 2;
    let mut rows = Vec::new();
    for _ in 0..4 {
        let n = usize::from(body[start + 33]);
        let limit = start + 34 + n;
        let path = limit + 3;
        let end = path + usize::from(u16::from_be_bytes([body[limit+1], body[limit+2]]));
        rows.push(Row { start, limit, path, end });
        start = end;
    }
    assert_eq!(start, body.len());
    (origin, count, rows)
}
fn rehashed_error(body: &[u8], why: Invalid, change: impl FnOnce(&mut Vec<u8>)) {
    let mut b = body.to_vec(); change(&mut b);
    assert_eq!(FreshGenesis::decode(&sign(&b)).unwrap_err(), why);
}

#[test]
fn rejects_wrong_formats_corrupt_hashes_truncation_and_prefixed_journal_bytes() {
    let image = wire("genesis");
    for n in 0..image.len() { assert!(FreshGenesis::decode(&image[..n]).is_err()); }
    let (body, _) = image.split_last_chunk::<32>().unwrap();
    rehashed_error(body, Invalid::Format, |b| { b.splice(..GENESIS.len(), b"ZXS/FILE-POOL/ID-LEAF-1\0".iter().copied()); });
    let mut altered = image.clone(); altered[GENESIS.len()+4] ^= 1;
    assert_eq!(FreshGenesis::decode(&altered).unwrap_err(), Invalid::Digest);
    let mut extra = image.clone(); extra.extend_from_slice(b"post-genesis bytes");
    assert!(FreshGenesis::decode(&extra).is_err());
    rehashed_error(body, Invalid::Length, |b| b.push(0));
    assert_eq!(FreshGenesis::decode(&vec![0; MAX_IMAGE+1]).unwrap_err(), Invalid::Length);
}

#[test]
fn rejects_rehashed_role_count_identity_and_geometry_substitutions() {
    let image = wire("genesis");
    let (b, _) = image.split_last_chunk::<32>().unwrap();
    let (origin, count, rows) = fields(b);
    for start in [GENESIS.len(), GENESIS.len()+16, rows[0].start+1] {
        rehashed_error(b, Invalid::Identity, |c| c[start..start+16].fill(0));
    }
    for n in [2_u16, 65] { rehashed_error(b, Invalid::Role, |c| c[count..count+2].copy_from_slice(&n.to_be_bytes())); }
    for r in &rows {
        rehashed_error(b, Invalid::Role, |c| c[r.start] = 0xff);
        rehashed_error(b, Invalid::Path, |c| c[r.limit] = 0);
        rehashed_error(b, Invalid::Identity, |c| c[r.start+25..r.start+33].fill(0));
        for length in [20, 149] { rehashed_error(b, Invalid::Identity, |c| c[r.start+33] = length); }
        rehashed_error(b, Invalid::Identity, |c| c[r.start+34..r.start+50].fill(0));
    }
    let s = rows[0].start; let r = rows[1].start; let f = rows[2].start;
    rehashed_error(b, Invalid::Identity, |c| c[f+1..f+17].copy_from_slice(&b[s+1..s+17]));
    rehashed_error(b, Invalid::Identity, |c| c[f+17..f+33].copy_from_slice(&b[s+17..s+33]));
    rehashed_error(b, Invalid::Identity, |c| c[origin..origin+16].copy_from_slice(&b[s+17..s+33]));
    // Keep dev/inode distinct and substitute an entire original's opaque incarnation.
    rehashed_error(b, Invalid::Identity, |c| { c.splice(f+33..rows[2].limit, b[s+33..rows[0].limit].iter().copied()); });
    rehashed_error(b, Invalid::Identity, |c| { c.splice(origin+16..count, b[s+33..rows[0].limit].iter().copied()); });
    for other in [s, f] {
        rehashed_error(b, Invalid::Identity, |c| c[r+17..r+25].copy_from_slice(&b[other+17..other+25]));
        rehashed_error(b, Invalid::Identity, |c| c[r+34..r+50].copy_from_slice(&b[other+34..other+50]));
    }
    rehashed_error(b, Invalid::Identity, |c| c[origin..origin+8].copy_from_slice(&b[r+17..r+25]));
    rehashed_error(b, Invalid::Identity, |c| c[origin+17..origin+33].copy_from_slice(&b[r+34..r+50]));
}

fn replace_path(body: &mut Vec<u8>, row: Row, path: &[u8]) {
    body[row.path-2..row.path].copy_from_slice(&u16::try_from(path.len()).unwrap().to_be_bytes());
    body.splice(row.path..row.end, path.iter().copied());
}
#[test]
fn enforces_raw_file_order_canonical_paths_and_component_ancestry() {
    let image = wire("genesis");
    let (b, _) = image.split_last_chunk::<32>().unwrap();
    let (_, _, rows) = fields(b);
    for bad in [&b"/"[..], b"not-absolute", b"/m/", b"/m//x", b"/m/./x", b"/m/../x", b"/m/n\0x", b"/m/sys", b"/m/sys/sub", b"/m"] {
        rehashed_error(b, Invalid::Path, |c| replace_path(c, rows[2], bad));
    }
    for bad in [&b"/m/data"[..], b"/m/aaa"] {
        rehashed_error(b, Invalid::Path, |c| replace_path(c, rows[3], bad));
    }
    rehashed_error(b, Invalid::Path, |c| replace_path(c, rows[2], &b"/x".repeat(257)));
    rehashed_error(b, Invalid::Path, |c| replace_path(c, rows[2], &vec![b'x'; 4097]));
    // Equal character prefixes are fine when the next byte isn't a component boundary.
    let mut permitted = b.to_vec(); replace_path(&mut permitted, rows[2], b"/m/sys2");
    assert!(FreshGenesis::decode(&sign(&permitted)).is_ok());
}

#[test]
fn decoded_and_operator_selected_storage_paths_share_exact_raw_bounds() {
    use crate::pinned::storage_status::Selection;
    use std::{ffi::OsString, os::unix::ffi::OsStringExt};
    let image = wire("genesis");
    let (body, _) = image.split_last_chunk::<32>().unwrap();
    let row = fields(body).2[0]; // no effect on the lexical ordering of the two FILE rows
    let check = |path: Vec<u8>, accepted: bool| {
        let mut b = body.to_vec(); replace_path(&mut b, row, &path);
        assert_eq!(FreshGenesis::decode(&sign(&b)).is_ok(), accepted);
        let selection = ["--system".into(), OsString::from_vec(path), "--recycle".into(),
                         "/bin".into(), "--data".into(), "/data".into()];
        assert_eq!(Selection::parse(selection.into_iter()).is_ok(), accepted);
    };
    for count in [128, 129, 256, 257] {
        let mut raw = b"/x".repeat(count);
        *raw.last_mut().unwrap() = 0xFF; // no UTF-8 normalization or loss
        assert_eq!(Kind::Directory.check_path(&raw).is_ok(), count <= 128);
        assert_eq!(Kind::Regular.check_path(&raw).is_ok(), count <= 128);
        check(raw, count <= 256);
    }
    let bound = b"/123456789abcdef".repeat(256);
    assert_eq!(bound.len(), 4096);
    check(bound.clone(), true);
    let mut too_long = bound; too_long.push(b'x'); check(too_long, false);
    for bad in [&b"/"[..], b"relative", b"/x//y", b"/x/./y", b"/x/../y", b"/x/", b"/x\0y"] {
        check(bad.to_vec(), false);
    }
}
