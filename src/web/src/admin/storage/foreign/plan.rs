use super::*;
use storage_contract::{OperationId, storage_foreign_plan::*};

mod actions;
mod draft;
mod fields;
mod intent;
mod messages;
mod response;
mod view;

use draft::{Draft, DraftError, Intent, Mode, PrimaryChoice};
use response::{read_preview, read_status};
pub(super) use view::view;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
enum SaveState {
    #[default]
    Unsubmitted,
    Sending,
    Unknown,
    Stored,
    Rejected,
}
#[derive(Clone, Copy)]
enum Read {
    Status,
    Revalidate,
}
#[derive(Clone, Default)]
pub(super) struct Model {
    draft: Draft,
    intent: intent::Model,
    review: Option<ForeignPlanPreview>,
    stored: Option<ForeignPlanStatus>,
    known_primary: Vec<PrimaryChoice>,
    save: SaveState,
    reading: bool,
    previewing: bool,
    generation: u64,
    acknowledged: bool,
    error: Option<api::Error>,
}
impl Model {
    pub(super) fn blocks_edit(&self) -> bool {
        self.intent.blocks_edit()
            || self.reading
            || self.previewing
            || matches!(self.save, SaveState::Sending | SaveState::Unknown)
    }
    pub(super) fn set_source(&mut self, source: &ForeignDiskPreview) {
        self.invalidate();
        self.draft.names = source
            .histories
            .iter()
            .map(|history| (history.group_id.clone(), String::new()))
            .collect();
    }
    pub(super) fn leave(&mut self) {
        self.intent.leave();
        if self.save != SaveState::Sending {
            self.generation += 1;
            self.reading = false;
            self.previewing = false;
        }
    }
    pub(super) fn invalidate(&mut self) {
        if self.blocks_edit() {
            return;
        }
        self.generation += 1;
        self.draft = Draft::default();
        self.intent.reset();
        self.review = None;
        self.stored = None;
        self.known_primary.clear();
        self.acknowledged = false;
        self.error = None;
        self.save = SaveState::Unsubmitted;
    }
    fn edit(&mut self, change: impl FnOnce(&mut Draft)) {
        if self.blocks_edit() {
            return;
        }
        let previous = self.draft.clone();
        change(&mut self.draft);
        if self.draft == previous {
            return;
        }
        self.generation += 1;
        self.intent.reset();
        self.review = None;
        self.stored = None;
        self.acknowledged = false;
        self.error = None;
        self.save = SaveState::Unsubmitted;
    }
    fn accept_preview(&mut self, review: ForeignPlanPreview) {
        self.intent.reset();
        let primary = PrimaryChoice {
            identity: review.target.identity.clone(),
            sequence: review.target.sequence,
        };
        self.known_primary.retain(|known| {
            !known
                .identity
                .slot_id
                .eq_ignore_ascii_case(&primary.identity.slot_id)
        });
        self.known_primary.push(primary);
        self.previewing = false;
        self.save = SaveState::Unsubmitted;
        self.review = Some(review);
        self.stored = None;
        self.acknowledged = false;
        self.error = None;
    }
    fn can_save(&self) -> bool {
        !self.intent.blocks_edit()
            && self.acknowledged
            && self.review.is_some()
            && !self.reading
            && !self.previewing
            && matches!(self.save, SaveState::Unsubmitted | SaveState::Unknown)
    }
    fn save_request(&mut self) -> Option<ForeignPlanSave> {
        if !self.can_save() {
            return None;
        }
        let preview = self
            .review
            .as_ref()
            .expect("invariant: save has an exact review")
            .clone();
        self.save = SaveState::Sending;
        self.error = None;
        Some(ForeignPlanSave {
            preview,
            acknowledge_plan_only: true,
        })
    }
    fn accept_saved(&mut self, status: ForeignPlanStatus) {
        self.save = SaveState::Stored;
        self.reading = false;
        self.error = None;
        self.stored = Some(status);
    }
    fn read_request(&mut self, action: Read) -> Option<(u64, ForeignPlanPreview)> {
        if self.intent.blocks_edit()
            || self.reading
            || self.previewing
            || !matches!(self.save, SaveState::Unknown | SaveState::Stored)
            || (matches!(action, Read::Revalidate) && self.save != SaveState::Stored)
        {
            return None;
        }
        let expected = self.review.clone()?;
        self.reading = true;
        self.generation += 1;
        self.error = None;
        Some((self.generation, expected))
    }
    fn finish_read(&mut self, generation: u64, result: Result<ForeignPlanStatus, api::Error>) {
        if self.generation != generation {
            return;
        }
        self.reading = false;
        match result {
            Ok(status) => self.accept_saved(status),
            Err(error) => self.read_failed(error),
        }
    }
    fn read_failed(&mut self, error: api::Error) {
        self.reading = false;
        if let Some(status) = &mut self.stored {
            status.validation = ForeignPlanValidation::Required;
        }
        self.error = Some(error);
    }
    fn save_failed(&mut self, error: api::Error) {
        self.save = if error.mutation_outcome_unknown() {
            SaveState::Unknown
        } else {
            SaveState::Rejected
        };
        if self.save == SaveState::Rejected {
            self.acknowledged = false;
        }
        self.error = Some(error);
    }
}
fn editable(state: &State) -> bool {
    state.session_active()
        && state.admin_console.storage.editable()
        && state.admin_console.storage.foreign.preview.is_some()
}
fn input_error(error: DraftError) -> api::Error {
    api::error(storage_contract::ErrorCode::ConfigInvalid, error.message())
}

#[cfg(test)]
#[path = "../../../../../../test/web/unit/admin/storage_foreign_plan_tests.rs"]
mod tests;
