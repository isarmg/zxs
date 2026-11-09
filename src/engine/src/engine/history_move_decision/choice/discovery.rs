//! Cold discovery only adds denial. Missing replicas do not authorize a vote,
//! clear the selection, select a different operation or reconstruct lost inputs.
use super::*;
use platform_io::fs_safety::linux::FileIdentity;

fn read_at(root: &DiskRoot, config: &ValidatedConfig) -> Result<Option<Choice>> {
    let identity = root.directory_identity(root.path())?;
    let path = root.path().join(DIRECTORY);
    let found = if root.optional_metadata(&path)?.is_some() {
        let stage_identity = root.directory_identity(&path)?;
        let stage = root.private_child_matching(&path, stage_identity)?;
        let held = HeldRegular::capture(&stage, NAME, MAX_BYTES)?;
        let choice = held
            .as_ref()
            .map(|held| Choice::decode(config, held.bytes()))
            .transpose()?;
        // Hold the original inode through a fresh name/content observation.
        let named = HeldRegular::capture(&stage, NAME, MAX_BYTES)?;
        if held.as_ref().map(HeldRegular::stamp) != named.as_ref().map(HeldRegular::stamp)
            || root.directory_identity(&path)? != stage_identity
        {
            return Err(invalid(
                "retained move choice or its stage changed during discovery",
            ));
        }
        if let Some(held) = held {
            held.check_running()?;
        }
        choice
    } else {
        None
    };
    let named = DiskRoot::open(root.path())?;
    if named.directory_identity(named.path())? != identity {
        return Err(invalid("move choice owner root changed during discovery"));
    }
    Ok(found)
}

fn retain(selected: &mut Option<Choice>, observed: Option<Choice>) -> Result<()> {
    if let Some(observed) = observed {
        match selected {
            Some(original) if *original != observed => {
                return Err(Error::new(
                    ErrorCode::RecoveryRequired,
                    "retained move choices disagree; preserve every original replica",
                ));
            }
            Some(_) => {}
            None => *selected = Some(observed),
        }
    }
    Ok(())
}

impl Engine {
    /// Called with pool writer locks held, before ordinary recovery opens a
    /// journal writer. Probe only admitted I/O roots, not arbitrary config paths.
    pub(in crate::engine) fn discover_file_history_move_choice(&mut self) -> Result<()> {
        let root = DiskRoot::open(&self.root)?;
        root.private_directory(root.path())?;
        let writer = self._locks.system_identity()?;
        let check_writer = || -> Result<()> {
            let named = root.open_regular(&root.path().join("writer.lock"))?;
            if FileIdentity::from_fd(&named)? != writer {
                return Err(invalid(
                    "system writer identity changed during move choice discovery",
                ));
            }
            Ok(())
        };
        check_writer()?;
        let mut selected = read_at(&root, &self.config)?;
        check_writer()?;
        for handle in self.disk_resources.handles() {
            let disk = self
                .config
                .io_disks()
                .find(|disk| disk.disk_id == handle.seal.disk_id)
                .expect("invariant: admitted handle has an I/O configuration binding");
            let lease = handle.root.admit_io()?;
            let check_seal = || -> Result<()> {
                let seal =
                    validate_seal_observed(&self.config, disk, &mut |_| lease.check_running())?;
                validate_stored_seal(&handle.seal, &seal)?;
                lease.check_running()
            };
            check_seal()?;
            let observed = read_at(&handle.root, &self.config)?;
            if observed.as_ref().is_some_and(|choice| {
                !choice
                    .record
                    .participants
                    .iter()
                    .any(|identity| identity.disk_id == disk.disk_id)
            }) {
                return Err(invalid(
                    "move choice is stored outside its original participant set",
                ));
            }
            check_seal()?;
            retain(&mut selected, observed)?;
        }
        self.history_move_choice = selected;
        self.discover_move_intent()
    }
}
