//! Explicit final selection. A successful whole SHA is only a witness for
//! the same still-held and named staged object, never a second content copy.
use super::{
    Authorize, Failure, Reader, Result, Uploads, change, journal, material, selected_offset,
    unavailable,
};
use crate::{
    container_store::{
        Conflict, Error, Id, Mutation, OperationReceipt, Receipt, Store, invalid, routes::Routes,
        stage::Publication, volume::DataVolume,
    },
    fs::HeldDir,
};
use std::{ffi::CStr, io, sync::Arc};

const VERIFIERS: usize = 2;

pub(super) struct Committed {
    pub(super) receipt: Receipt,
    /// The original receipt remains definitive if its auxiliary UR1 removal
    /// was skipped (source still exists) or remains uncertain.
    pub(super) retired: io::Result<bool>,
}

impl Uploads<'_> {
    pub(super) fn commit(
        &self,
        mutation: Mutation,
        upload: [u8; 32],
        authorize: Authorize<'_>,
    ) -> Result<Committed> {
        let gate = self.gate(mutation, authorize)?;
        let _serial = gate.lock().map_err(|_| unavailable())?;
        // Keep this exact original Store slot referenced, but never keep its
        // object lock across the full raw-body read.
        let object = self.store.object(mutation.container)?;
        let (directory, slot, mut staged) = {
            let mut memory = object.lock().map_err(|_| unavailable())?;
            authorize()?;
            self.require_authority()?;
            let receipt = self.store.receipt_under_gate(mutation, &mut memory)?;
            let (directory, slot) =
                self.finalize_claim(mutation, upload, receipt.is_some(), authorize)?;
            if let Some(receipt) = receipt {
                return self.existing(&directory, &slot, receipt, authorize);
            }
            if selected_offset(&slot.record)? != slot.record.intent.body.length {
                return Err(Failure::Conflict);
            }
            let volume = self.original_volume(&slot.record)?;
            let staged = material::Stage::open(volume, &slot.record)?;
            (directory, slot, staged)
        };

        let verifier = Reader::take(&self.store.uploads.verifiers, VERIFIERS)?;
        staged
            .verify_whole(&slot.record.intent.body, authorize)
            .map_err(store_failure)?;
        drop(verifier);

        let mut memory = object.lock().map_err(|_| unavailable())?;
        self.require_control(&directory)?;
        slot.require(&directory)?;
        authorize()?;
        let receipt = self.store.receipt_under_gate(mutation, &mut memory)?;
        if let Some(receipt) = receipt {
            return self.existing(&directory, &slot, receipt, authorize);
        }
        let volume = self.original_volume(&slot.record)?;
        staged.require()?;
        let prove = || {
            self.require_control(&directory)?;
            slot.require(&directory)
        };
        let mut adoption = Adoption {
            stage: &mut staged,
            store: self.store,
            prove,
        };
        let receipt = self
            .store
            .publish_under_gate(
                mutation,
                change(&slot.record.intent),
                (volume, &mut memory),
                &mut adoption,
                authorize,
            )
            .map_err(store_failure)?;
        Ok(self.confirmed(&directory, &slot, receipt, authorize))
    }

    fn finalize_claim(
        &self,
        mutation: Mutation,
        upload: [u8; 32],
        official: bool,
        authorize: Authorize<'_>,
    ) -> Result<(Arc<HeldDir>, journal::Slot)> {
        let absent = || {
            if official {
                Failure::Conflict
            } else {
                Failure::Absent
            }
        };
        let directory = self.directory(false, authorize)?.ok_or_else(absent)?;
        let slot = journal::read(&directory, mutation.container)?.ok_or_else(absent)?;
        self.require_scope(&slot, mutation, Some(upload), official)?;
        if official && selected_offset(&slot.record).is_err() {
            return Err(Failure::Conflict);
        }
        authorize()?;
        slot.settle(&directory)?;
        self.require_control(&directory)?;
        slot.require(&directory)?;
        authorize()?;
        Ok((directory, slot))
    }

    fn existing(
        &self,
        directory: &HeldDir,
        slot: &journal::Slot,
        receipt: OperationReceipt,
        authorize: Authorize<'_>,
    ) -> Result<Committed> {
        let OperationReceipt::Mutation(receipt) = receipt else {
            return Err(Failure::Conflict);
        };
        if receipt.mutation != slot.record.intent.mutation
            || receipt.change != change(&slot.record.intent)
        {
            return Err(Failure::Conflict);
        }
        self.require_control(directory)?;
        slot.require(directory)?;
        authorize()?;
        Ok(self.confirmed(directory, slot, receipt, authorize))
    }

    fn confirmed(
        &self,
        directory: &HeldDir,
        slot: &journal::Slot,
        receipt: Receipt,
        authorize: Authorize<'_>,
    ) -> Committed {
        let retired = (|| {
            authorize()?;
            self.require_control(directory)?;
            slot.require(directory)?;
            let volume = self
                .enrolled(slot.record.data_token)
                .map_err(|_| unavailable())?;
            if material::source_exists(volume, &slot.record)? {
                return Ok(false);
            }
            slot.retire(
                directory,
                || {
                    self.require_control(directory)?;
                    material::require_unused(volume, &slot.record)
                },
                authorize,
            )?;
            Ok(true)
        })();
        Committed { receipt, retired }
    }
}

struct Adoption<'a, 'v, F> {
    stage: &'a mut material::Stage<'v>,
    store: &'a Store,
    prove: F,
}

impl<F: Fn() -> io::Result<()>> Adoption<'_, '_, F> {
    // No Store route lookups here: the bootstrap reservation may already hold
    // the SYSTEM routes cut. Prove our existing held upload, not a new route.
    fn prove_bootstrap(&self) -> io::Result<()> {
        (self.prove)()?;
        self.stage.require_prepared()
    }
}

impl<F: Fn() -> io::Result<()>> Publication for Adoption<'_, '_, F> {
    fn stage_receipt(
        &mut self,
        receipt: &Receipt,
        authorize: &impl Fn() -> io::Result<()>,
    ) -> std::result::Result<(), Error> {
        self.stage
            .prepare_receipt(self.store, receipt, &self.prove, authorize)
    }

    fn install(
        &self,
        parent: &HeldDir,
        name: &CStr,
        authorize: impl FnOnce() -> io::Result<()>,
    ) -> io::Result<()> {
        self.stage.choose(parent, name, &self.prove, authorize)
    }

    fn reserve_first(
        &self,
        routes: &Routes,
        id: Id,
        token: [u8; 32],
        authorize: &impl Fn() -> io::Result<()>,
    ) -> io::Result<()> {
        routes.reserve_upload(id, token, &|| self.prove_bootstrap(), authorize)
    }

    fn create_first(
        &self,
        volume: &DataVolume,
        id: Id,
        authorize: &impl Fn() -> io::Result<()>,
    ) -> io::Result<HeldDir> {
        volume.create_upload_container(id, || {
            if self.store.routes.get(id)? != Some(volume.token) {
                return Err(invalid("upload lost its exact selected first route"));
            }
            self.prove_bootstrap()?;
            authorize()
        })
    }

    fn confirm_moved(&self, parent: &HeldDir, name: &CStr) -> io::Result<()> {
        (self.prove)()?;
        self.stage.confirm_moved(parent, name)
    }
}

fn store_failure(error: Error) -> Failure {
    match error {
        Error::Conflict(Conflict::Invalid(error)) => error.into(),
        Error::Conflict(_) => Failure::Conflict,
        Error::WrongLength | Error::WrongDigest => Failure::InvalidBody,
        Error::Io(error) | Error::Uncertain(error) | Error::CleanupPending { error, .. } => {
            error.into()
        }
    }
}
