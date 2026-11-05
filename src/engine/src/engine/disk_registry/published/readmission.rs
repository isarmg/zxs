//! A new admission of the same retained registration, never a historical rebind.
use super::continuation::RegistryBoundary;
use super::*;

impl VerifiedDiskRegistry {
    pub(in crate::engine) fn prepare_readmission(
        &self,
        source: &ValidatedConfig,
        target: &ValidatedConfig,
        boundary: RegistryBoundary<'_>,
        identity: &DiskRegistrationIdentity,
    ) -> Result<PreparedDiskRegistry> {
        encode_registry(self.readmission_state(source, target, boundary, identity)?)
    }

    pub(in crate::engine) fn verify_readmission(
        &self,
        source: &ValidatedConfig,
        target: &ValidatedConfig,
        boundary: RegistryBoundary<'_>,
        identity: &DiskRegistrationIdentity,
        expected: &DiskRegistryCommitment,
        read: impl FnMut(&'static str, &str) -> Result<Vec<u8>>,
    ) -> Result<Self> {
        let loaded = load_registry(expected, read)?;
        if loaded != self.readmission_state(source, target, boundary, identity)? {
            return Err(invalid(
                "readmission changes the exact retained registration",
            ));
        }
        Ok(Self {
            commitment: expected.clone(),
            state: loaded,
        })
    }

    fn readmission_state(
        &self,
        source: &ValidatedConfig,
        target: &ValidatedConfig,
        boundary: RegistryBoundary<'_>,
        identity: &DiskRegistrationIdentity,
    ) -> Result<RegistryState> {
        self.require_identity(identity)?;
        if self.registration(identity)? != Some(RegistrationMode::Disabled)
            || self.state.detached.contains_key(&identity.disk_id)
        {
            return Err(invalid(
                "readmission requires an existing Disabled registration",
            ));
        }
        // The ordinary continuation authenticates the published source and clears
        // only its completed exit reservation. Historical definitions stay exact.
        let mut next = self.continuation_state(source, source, boundary)?;
        let historical = &next.topology[&identity.disk_id];
        let requested = target
            .disks
            .iter()
            .find(|disk| disk.disk_id == identity.disk_id)
            .ok_or_else(|| invalid("readmission target omits the retained registration"))?;
        if requested.metadata_member != historical.original_metadata_member {
            return Err(invalid(
                "readmission must restore the original registration role",
            ));
        }
        next.registrations
            .get_mut(&identity.disk_id)
            .expect("invariant: Disabled registration was admitted")
            .mode = RegistrationMode::Enabled;
        if historical.original_metadata_member {
            next.voters
                .insert(identity.disk_id.clone(), identity.clone());
        }
        let result = Self {
            commitment: self.commitment.clone(),
            state: next,
        };
        result.require_config(target)?;
        Ok(result.state)
    }
}
