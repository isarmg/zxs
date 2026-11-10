use super::*;
use storage_contract::storage_foreign_intent::*;

mod actions;
mod response;
mod view;
pub(super) use view::view;

#[derive(Clone, Default)]
pub(super) struct Model {
    preview: Option<Arc<ForeignIntentPreview>>,
    stored: Option<ForeignIntentStatus>,
    save: SaveState,
    generation: u64,
    previewing: bool,
    reading: bool,
    acknowledged: bool,
    cancellation: Option<api::ReadCancellation>,
    error: Option<api::Error>,
}
struct Submission {
    generation: u64,
    request: ForeignIntentSave,
    previous: SaveState,
}
impl Model {
    pub(super) fn blocks_edit(&self) -> bool {
        self.previewing
            || self.reading
            || matches!(self.save, SaveState::Sending | SaveState::Unknown)
    }
    pub(super) fn leave(&mut self) {
        if let Some(cancellation) = self.cancellation.take() {
            cancellation.abort();
        }
        if self.save != SaveState::Sending {
            self.generation += 1;
            self.previewing = false;
            self.reading = false;
        }
    }
    pub(super) fn reset(&mut self) {
        let generation = self.generation + 1;
        self.leave();
        *self = Self {
            generation,
            ..Self::default()
        };
    }
    fn can_preview(&self) -> bool {
        !self.blocks_edit() && matches!(self.save, SaveState::Unsubmitted | SaveState::Rejected)
    }
    fn begin_preview(&mut self) -> Option<u64> {
        if !self.can_preview() {
            return None;
        }
        self.generation += 1;
        self.previewing = true;
        self.preview = None;
        self.stored = None;
        self.acknowledged = false;
        self.error = None;
        self.save = SaveState::Unsubmitted;
        Some(self.generation)
    }
    fn finish_preview(
        &mut self,
        generation: u64,
        result: Result<ForeignIntentPreview, api::Error>,
    ) {
        if self.generation != generation || !self.previewing {
            return;
        }
        self.previewing = false;
        self.cancellation = None;
        match result {
            Ok(preview) => self.preview = Some(Arc::new(preview)),
            Err(error) => self.error = Some(error),
        }
    }
    fn can_save(&self) -> bool {
        self.acknowledged
            && self.preview.is_some()
            && !self.reading
            && !self.previewing
            && matches!(self.save, SaveState::Unsubmitted | SaveState::Unknown)
    }
    fn begin_save(&mut self) -> Option<Submission> {
        if !self.can_save() {
            return None;
        }
        let preview = self
            .preview
            .as_deref()
            .expect("invariant: confirmed intent has a review")
            .clone();
        let previous = self.save;
        self.save = SaveState::Sending;
        self.generation += 1;
        self.error = None;
        Some(Submission {
            generation: self.generation,
            request: ForeignIntentSave {
                preview,
                confirm_save_intent: true,
            },
            previous,
        })
    }
    fn finish_save(
        &mut self,
        submission: &Submission,
        result: Result<ForeignIntentStatus, api::Error>,
    ) {
        if self.generation != submission.generation || self.save != SaveState::Sending {
            return;
        }
        match result {
            Ok(status) => {
                self.save = SaveState::Stored;
                self.stored = Some(status);
            }
            Err(error) => {
                self.save = if submission.previous == SaveState::Unknown
                    || error.mutation_outcome_unknown()
                {
                    SaveState::Unknown
                } else {
                    self.acknowledged = false;
                    SaveState::Rejected
                };
                self.error = Some(error);
            }
        }
    }
    fn begin_status(&mut self) -> Option<(u64, Arc<ForeignIntentPreview>)> {
        if self.reading
            || self.previewing
            || !matches!(self.save, SaveState::Stored | SaveState::Unknown)
        {
            return None;
        }
        let expected = self.preview.clone()?;
        self.generation += 1;
        self.reading = true;
        self.error = None;
        Some((self.generation, expected))
    }
    fn finish_status(&mut self, generation: u64, result: Result<ForeignIntentStatus, api::Error>) {
        if self.generation != generation || !self.reading {
            return;
        }
        self.reading = false;
        self.cancellation = None;
        match result {
            Ok(status) => {
                self.save = SaveState::Stored;
                self.stored = Some(status);
            }
            Err(error) => self.error = Some(error),
        }
    }
}
fn query(preview: &ForeignIntentPreview) -> ForeignIntentQuery {
    ForeignIntentQuery {
        operation_id: preview.request.operation_id,
        target: preview.plan.target.identity.clone(),
        intent_digest: preview.intent_digest.clone(),
    }
}

#[cfg(test)]
#[path = "../../../../../../../test/web/unit/admin/storage_foreign_intent_tests.rs"]
mod tests;
