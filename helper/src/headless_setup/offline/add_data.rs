//! Append one empty DATA disk using the exact offline descriptor lease.
use super::{
    DESCRIPTOR, FIXED_ROOTS, LOGIN, MAX_DATA, OfflineSession, read,
    stage::{finish_stage, read_stage},
    stamp,
};
use crate::{
    headless_setup::{
        Held, MARK_BYTES, MARK_MAGIC, MARKER, RootInput, SelectedRoles, random, read_role_token,
        refused, require_only, role_proof,
    },
    pinned::Entity,
};
use rustix::fs::{RenameFlags, renameat, renameat_with};
use std::{io, os::unix::ffi::OsStrExt, path::Path};
use zeroize::Zeroizing;

const ADD_STAGE: &str = ".server-stage-add-data";
const DATA: u8 = 1;
const ROLE_STAGE: &str = ".role-stage-add-data";
const TOKEN_AT: usize = MARK_MAGIC.len() + 17;

impl SelectedRoles {
    pub(crate) fn add_data(self, path: &Path) -> io::Result<()> {
        // An exact rerun after descriptor replacement can recognize the
        // same selected path as the final roster entry without writing again.
        let current_paths = self.paths().map(Path::to_path_buf).collect::<Vec<_>>();
        let session = match self.open_offline(Some(ADD_STAGE)) {
            Ok(session) => session,
            Err(error) if current_paths.len() < FIXED_ROOTS + MAX_DATA => {
                let mut already = current_paths;
                already.push(path.to_owned());
                return Self::select(&already)
                    .and_then(SelectedRoles::open_durable)
                    .map(|_| ())
                    .map_err(|_| error);
            }
            Err(error) => return Err(error),
        };
        session.append_data(path)
    }
}

impl OfflineSession {
    fn append_data(&self, path: &Path) -> io::Result<()> {
        if self.selected.roots.len() == FIXED_ROOTS + MAX_DATA {
            return Err(refused());
        }
        self.require_original()?;
        let uid = self.selected.uid;
        let pool = &self.identity.pool;
        let candidate = Held::select(path, uid, &self.selected.scope)?;
        let mut marker = [0; MARK_BYTES];
        let offset = MARK_MAGIC.len();
        marker[..offset].copy_from_slice(MARK_MAGIC);
        marker[offset] = DATA;
        marker[offset + 1..TOKEN_AT].copy_from_slice(pool);
        let (has_role_file, token) = match read_role_token(&candidate.input(), DATA, pool, uid) {
            Ok((_, token)) => (true, token),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let mut token = random::<32>()?;
                if let Some(stage) = read_stage(&candidate.directory, ROLE_STAGE, uid)? {
                    let prefix = &stage.full[..stage.full.len().min(TOKEN_AT)];
                    if stage.full.len() > MARK_BYTES || !marker[..TOKEN_AT].starts_with(prefix) {
                        return Err(refused());
                    }
                    if stage.full.len() > TOKEN_AT {
                        let partial = &stage.full[TOKEN_AT..];
                        token[..partial.len()].copy_from_slice(partial);
                    }
                }
                (false, token)
            }
            Err(error) => return Err(error),
        };
        let system = &self.selected.roots[0].directory;
        if super::retire_data::retired(system, pool, &token, uid)? {
            return Err(refused());
        }
        marker[TOKEN_AT..].copy_from_slice(&token);
        let marker_name = Some(MARKER.as_bytes());
        require_only(
            &candidate.directory,
            if has_role_file {
                marker_name
            } else {
                Some(ROLE_STAGE.as_bytes())
            },
        )?;

        let root = || RootInput {
            held: candidate.input(),
            token,
        };
        let original_and_candidate = || {
            self.require_original()?;
            let mut issue = self.selected.issue(&self.login, &self.identity)?;
            issue.roots.push(root());
            issue.require_roots(false)?;
            if super::retire_data::retired(system, pool, &token, uid)? {
                return Err(refused());
            }
            Ok(())
        };
        original_and_candidate()?;

        let updated = self.descriptor_with_disk(&candidate, &token)?;

        // A previously prepared replacement is useful only if it is the exact
        // append to the still-current descriptor for this selected disk.
        if let Some(stage) = read_stage(&self.locked, ADD_STAGE, uid)?
            && (!has_role_file || !updated.starts_with(&stage.full))
        {
            return Err(refused());
        }

        if !has_role_file {
            publish_role_marker(&candidate, &marker, uid, original_and_candidate)?;
        }
        let prove = || {
            original_and_candidate()?;
            require_only(&candidate.directory, marker_name)?;
            role_proof(&root(), DATA, pool, uid)?;
            Ok::<(), io::Error>(())
        };
        prove()?;
        // A marker left by a stopped writer may have a visible name whose
        // directory entry was not yet durable.
        candidate.directory.sync_all()?;
        prove()?;

        let stage = finish_stage(&self.locked, ADD_STAGE, &updated, uid, prove)?;
        prove()?;
        let named = read(&self.locked, ADD_STAGE, uid)?;
        if stamp(&named.meta) != stamp(&stage.meta) || named.full.as_slice() != updated.as_slice() {
            return Err(refused());
        }
        renameat(&self.locked, ADD_STAGE, &self.locked, DESCRIPTOR)?;
        self.locked.sync_all()?;
        let final_record = read(&self.locked, DESCRIPTOR, uid)?;
        if Entity::of(&final_record.meta)? != Entity::of(&stage.meta)?
            || final_record.full.as_slice() != updated.as_slice()
        {
            return Err(refused());
        }
        role_proof(&root(), DATA, pool, uid)?;
        self.selected.require_site()
    }

    fn descriptor_with_disk(
        &self,
        candidate: &Held,
        token: &[u8; 32],
    ) -> io::Result<Zeroizing<Vec<u8>>> {
        let mut updated = Zeroizing::new(self.original.full.to_vec());
        updated[LOGIN + 1 + self.login.len()] += 1;
        let path = candidate.original.path().as_os_str().as_bytes();
        updated.extend_from_slice(
            &u16::try_from(path.len())
                .map_err(|_| refused())?
                .to_be_bytes(),
        );
        updated.extend_from_slice(path);
        updated.extend_from_slice(token);
        Ok(updated)
    }
}

fn publish_role_marker(
    candidate: &Held,
    marker: &[u8; MARK_BYTES],
    uid: u32,
    prove_original: impl Fn() -> io::Result<()>,
) -> io::Result<()> {
    let directory = &candidate.directory;
    let prove_marker = || {
        prove_original()?;
        require_only(directory, Some(ROLE_STAGE.as_bytes()))
    };
    let staged = finish_stage(directory, ROLE_STAGE, marker, uid, prove_marker)?;
    prove_marker()?;
    if stamp(&read(directory, ROLE_STAGE, uid)?.meta) != stamp(&staged.meta) {
        return Err(refused());
    }
    renameat_with(
        directory,
        ROLE_STAGE,
        directory,
        MARKER,
        RenameFlags::NOREPLACE,
    )?;
    directory.sync_all()
}
