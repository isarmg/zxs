//! Only for a fresh, empty selection. A durable PENDING is an unrelated
//! resume path: its original operation and issued IDs must never be regenerated.
use super::FreshRootLocks;
use crate::{maintenance::{genesis::{InitialBytes, RootInput}, register_flock::UnpublishedRegisterFlock}, pinned::invalid};
use rustix::rand::{GetRandomFlags, getrandom};
use std::{io, os::unix::ffi::OsStrExt};

/// A memory proposal tied to the original locked roots and sole unlinked empty
/// registration. Not a receipt. No FD, write, link, xattr or unlock is exposed.
#[must_use]
pub(in crate::maintenance) struct UnpublishedProposal<'r> {
    roots: &'r FreshRootLocks,
    register: UnpublishedRegisterFlock<'r>,
    bytes: InitialBytes,
}

fn random_id() -> io::Result<[u8; 16]> {
    let mut id = [0; 16];
    // One nonblocking kernel draw. A short read or an unready RNG causes no
    // allocation or claim. No clock/PID/fallback source or caller-supplied seed.
    let got = getrandom(&mut id[..], GetRandomFlags::NONBLOCK)?;
    if got != id.len() || id == [0; 16] { return Err(invalid("did not obtain a complete nonzero random ID")); }
    Ok(id)
}

impl FreshRootLocks {
    pub(in crate::maintenance) fn prepare_unpublished(&mut self, op: [u8; 16]) -> io::Result<UnpublishedProposal<'_>> {
        if op == [0; 16] { return Err(invalid("a nonzero original initialization operation is required")); }
        self.check_empty_selection()?;
        let pool = random_id()?;
        let mut rows = Vec::with_capacity(self.roots.len());
        for r in &self.roots {
            let id = random_id()?;
            if id == pool || rows.iter().any(|row: &RootInput<'_>| row.id == id) {
                return Err(invalid("fresh random identities collided"));
            }
            rows.push(RootInput { id, origin: r.observed.fields(), limit: r.component_limit,
                path: r.name.path().as_os_str().as_bytes() });
        }
        let register = UnpublishedRegisterFlock::create_in_held_system(&self.roots[0].dir, self.uid)?;
        let (original, system) = register.fresh_origins()?;
        if system != self.roots[0].observed { return Err(invalid("registration was not allocated on the selected original SYSTEM")); }
        let bytes = InitialBytes::encode(pool, op, self.uid, original.fields(), &rows)
            .map_err(|_| invalid("cannot encode the proposed original registration"))?;
        let proposal = UnpublishedProposal { roots: self, register, bytes };
        proposal.recheck()?;
        Ok(proposal)
    }
}

impl<'r> UnpublishedProposal<'r> {
    pub(in crate::maintenance) fn into_checked_parts(self) -> io::Result<(&'r FreshRootLocks, UnpublishedRegisterFlock<'r>, InitialBytes)> {
        self.recheck()?;
        Ok((self.roots, self.register, self.bytes))
    }

    pub(in crate::maintenance) fn recheck(&self) -> io::Result<()> {
        let g = self.bytes.checked().map_err(|_| invalid("invalid proposed registration bytes or evidence"))?;
        self.register.check_image_origins(&g)?; // original EX still held; nlink=0, length=0
        self.roots.check_image_roots(&g)
    }
}
