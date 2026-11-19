//! Pure, bounded encoder for a new uncommitted registration. Only the native
//! preparation supplies its real operands; this module neither issues IDs nor opens a file.
use super::{FreshGenesis, GENESIS, Id, Invalid, MAX_IMAGE, VOLUME};
use crate::path_rules::MAX_PATH_BYTES;
use sha2::{Digest, Sha256};

pub(in crate::maintenance) struct RootInput<'a> {
    pub(in crate::maintenance) id: Id,
    pub(in crate::maintenance) origin: (u64, u64, &'a [u8]),
    pub(in crate::maintenance) limit: u8,
    pub(in crate::maintenance) path: &'a [u8],
}

pub(in crate::maintenance) struct InitialBytes {
    pub(in crate::maintenance) image: Vec<u8>,
    pub(in crate::maintenance) labels: Vec<Vec<u8>>,
    pub(in crate::maintenance) frontier: [u8; 48],
}

fn put_origin(out: &mut Vec<u8>, (dev, ino, birth): (u64, u64, &[u8])) -> Result<(), Invalid> {
    if !(21..=148).contains(&birth.len()) { return Err(Invalid::Identity); }
    out.extend_from_slice(&dev.to_be_bytes());
    out.extend_from_slice(&ino.to_be_bytes());
    out.push(birth.len() as u8);
    out.extend_from_slice(birth);
    Ok(())
}

impl InitialBytes {
    pub(in crate::maintenance) fn encode(pool: Id, op: Id, uid: u32, register: (u64, u64, &[u8]),
                                        roots: &[RootInput<'_>]) -> Result<Self, Invalid> {
        if !(3..=64).contains(&roots.len()) { return Err(Invalid::Role); }
        let mut image = GENESIS.to_vec();
        image.extend_from_slice(&pool); image.extend_from_slice(&op); image.extend_from_slice(&uid.to_be_bytes());
        put_origin(&mut image, register)?;
        image.extend_from_slice(&(roots.len() as u16).to_be_bytes());
        for (i, r) in roots.iter().enumerate() {
            if r.path.len() > MAX_PATH_BYTES { return Err(Invalid::Path); }
            image.push(i.min(2) as u8);
            image.extend_from_slice(&r.id);
            put_origin(&mut image, r.origin)?;
            image.push(r.limit);
            image.extend_from_slice(&(r.path.len() as u16).to_be_bytes());
            image.extend_from_slice(r.path);
        }
        if image.len() > MAX_IMAGE-32 { return Err(Invalid::Length); }
        let digest: [u8; 32] = Sha256::digest(&image).into();
        image.extend_from_slice(&digest);
        let g = FreshGenesis::decode(&image)?;
        if g.pool != pool || g.operation != op || !g.has_root_count(uid, roots.len())
            || register != (g.registration.device, g.registration.inode, g.registration.birth)
            || roots.iter().enumerate().any(|(i,r)| g.volumes[i].id != r.id ||
                !g.matches_root_origin(i, r.path, r.limit, r.origin)) { return Err(Invalid::Identity); }

        let mut labels = Vec::with_capacity(roots.len());
        for row in &g.volumes {
            let mut label = VOLUME.to_vec();
            label.push(row.tag); label.extend_from_slice(&pool); label.extend_from_slice(&row.id);
            label.extend_from_slice(&op);
            if row.tag == 0 { label.extend_from_slice(&digest); }
            labels.push(label);
        }
        let mut frontier = [0; 48];
        frontier[16..].copy_from_slice(&digest);
        let bytes = Self { image, labels, frontier };
        Ok(bytes)
    }

    pub(in crate::maintenance) fn checked(&self) -> Result<FreshGenesis<'_>, Invalid> {
        let g = FreshGenesis::decode(&self.image)?;
        let labels: Vec<_> = self.labels.iter().map(Vec::as_slice).collect();
        g.check_labels(&labels)?;
        g.check_initial_frontier(Some(&self.frontier))?;
        Ok(g)
    }
}
