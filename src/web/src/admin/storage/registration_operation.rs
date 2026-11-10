//! Membership operations share request ownership, not authorization or receipts.
use super::*;
use storage_contract::OperationId;

pub(super) trait Status: Clone + DeserializeOwned + Sized + 'static {
    type Preview: Clone + PartialEq + 'static;
    type Context: Clone + Default;
    const PATH: &'static str;

    fn model(state: &State) -> &Model<Self>;
    fn model_mut(state: &mut State) -> &mut Model<Self>;
    fn operation(&self) -> OperationId;
    fn preview(&self) -> &Self::Preview;
    fn completed(&self) -> bool;
    fn running(&self) -> bool;
    fn validate(&self, expected: Option<OperationId>) -> Result<(), api::Error>;
    fn response(value: Value) -> Result<Option<Self>, api::Error>;
    fn load_context(_state: RwSignal<State>) {}
    fn leave_context(_context: &mut Self::Context) {}
    fn accept(model: &mut Model<Self>, status: Self) -> bool {
        model.accept_completed(status)
    }
    fn discard_rejected(model: &mut Model<Self>) -> bool {
        model.submitted = false;
        model.preview = None;
        model.operation = None;
        model.status = None;
        model.acknowledged = false;
        true
    }
    fn refresh(state: RwSignal<State>) {
        refresh(state);
    }
}

#[derive(Clone)]
pub(super) struct Model<S: Status> {
    pub(super) context: S::Context,
    pub(super) preview: Option<S::Preview>,
    pub(super) status: Option<S>,
    pub(super) operation: Option<OperationId>,
    pub(super) submitted: bool,
    pub(super) rejected: bool,
    pub(super) sending: bool,
    pub(super) reading: Option<u64>,
    pub(super) generation: u64,
    pub(super) acknowledged: bool,
    pub(super) error: Option<api::Error>,
}
impl<S: Status> Default for Model<S> {
    fn default() -> Self {
        Self {
            context: S::Context::default(),
            preview: None,
            status: None,
            operation: None,
            submitted: false,
            rejected: false,
            sending: false,
            reading: None,
            generation: 0,
            acknowledged: false,
            error: None,
        }
    }
}
impl<S: Status> Model<S> {
    pub(super) fn blocks_edit(&self) -> bool {
        self.sending
            || self.reading.is_some()
            || self.submitted
            || self
                .status
                .as_ref()
                .is_some_and(|status| !status.completed())
    }

    /// A newer mutation supersedes data, not the outstanding read's ownership.
    pub(super) fn finish_read(&mut self, generation: u64) -> bool {
        if self.reading != Some(generation) {
            return false;
        }
        self.reading = None;
        self.generation == generation
    }

    pub(super) fn leave_preview(&mut self) {
        if !self.submitted {
            self.generation += 1;
            self.preview = None;
            self.acknowledged = false;
            self.sending = false;
            self.reading = None;
            self.operation = self.status.as_ref().map(Status::operation);
        }
        S::leave_context(&mut self.context);
    }

    pub(super) fn accept(&mut self, status: S) -> bool {
        // A read started during POST can arrive after its permanent receipt.
        // It may release its own slot, but cannot reopen that settled operation.
        if self
            .status
            .as_ref()
            .is_some_and(|known| known.operation() == status.operation() && known.completed())
        {
            return false;
        }
        S::accept(self, status)
    }

    fn accept_completed(&mut self, status: S) -> bool {
        let completed = status.completed();
        self.operation = Some(status.operation());
        self.submitted = !completed;
        self.status = Some(status);
        if completed {
            self.preview = None;
            self.acknowledged = false;
            self.error = None;
        }
        completed
    }

    fn completed_operation(&self) -> bool {
        self.status
            .as_ref()
            .is_some_and(|status| self.operation == Some(status.operation()) && status.completed())
    }

    pub(super) fn finish_send(&mut self, generation: u64, result: Result<S, api::Error>) -> bool {
        if self.generation != generation {
            return false;
        }
        self.sending = false;
        match result {
            Ok(status) => self.accept(status),
            Err(error) => {
                self.rejected = !error.mutation_outcome_unknown() && !self.completed_operation();
                self.error = Some(error);
                false
            }
        }
    }

    pub(super) fn finish_load(
        &mut self,
        generation: u64,
        result: Result<Option<S>, api::Error>,
    ) -> bool {
        if !self.finish_read(generation) {
            return false;
        }
        match result {
            Ok(status) => {
                if self.preview.is_some() && !self.submitted {
                    return false;
                }
                status.is_some_and(|status| self.accept(status))
            }
            Err(error) => {
                if !(error.code == storage_contract::ErrorCode::OperationNotFound
                    && self.rejected
                    && !self.completed_operation()
                    && S::discard_rejected(self))
                {
                    self.error = Some(error);
                }
                false
            }
        }
    }
}

pub(super) struct Submission<S: Status> {
    pub(super) epoch: u64,
    pub(super) generation: u64,
    pub(super) operation: OperationId,
    pub(super) preview: S::Preview,
    pub(super) path: &'static str,
    pub(super) body: Value,
}

fn validate<S: Status>(
    status: &S,
    operation: Option<OperationId>,
    preview: Option<&S::Preview>,
) -> Result<(), api::Error> {
    status.validate(operation)?;
    if preview.is_some_and(|preview| preview != status.preview()) {
        return Err(invalid(ResponseError::Identity));
    }
    Ok(())
}

pub(super) fn send<S: Status>(state: RwSignal<State>, request: Submission<S>) {
    spawn_local(async move {
        let result = api::validated_request("POST", request.path, Some(request.body), |value| {
            let status: S = decode(value)?;
            validate(&status, Some(request.operation), Some(&request.preview))?;
            Ok(status)
        })
        .await;
        if !accept_read_response(state, request.epoch, &result) {
            return;
        }
        let completed = state.try_update(|s| {
            s.session_epoch == request.epoch
                && S::model_mut(s).finish_send(request.generation, result)
        }) == Some(true);
        if completed {
            S::refresh(state);
        }
        load::<S>(state);
    });
}

fn refresh(state: RwSignal<State>) {
    state.update(|s| s.files.invalidate_disks());
    super::super::disks::load(state);
    crate::management::load(state);
    super::load(state);
}

pub(super) fn load<S: Status>(state: RwSignal<State>) {
    S::load_context(state);
    let captured = state
        .try_update(|s| {
            if !s.session_active() {
                return None;
            }
            let epoch = s.session_epoch;
            let model = S::model_mut(s);
            if model.reading.is_some() {
                return None;
            }
            model.reading = Some(model.generation);
            Some((
                epoch,
                model.generation,
                model.submitted.then_some(model.operation).flatten(),
                model.submitted.then(|| model.preview.clone()).flatten(),
            ))
        })
        .flatten();
    let Some((epoch, generation, operation, expected)) = captured else {
        return;
    };
    spawn_local(async move {
        let path = operation.map_or_else(
            || S::PATH.to_owned(),
            |operation| format!("{}?operation_id={operation}", S::PATH),
        );
        let result = api::validated_request("GET", &path, None, |value| {
            let status = S::response(value)?;
            if let Some(status) = &status {
                validate(status, operation, expected.as_ref())?;
            }
            Ok(status)
        })
        .await;
        if !accept_read_response(state, epoch, &result) {
            return;
        }
        let completed = state.try_update(|s| {
            s.session_epoch == epoch && S::model_mut(s).finish_load(generation, result)
        }) == Some(true);
        if completed {
            S::refresh(state);
        }
    });
}

pub(super) fn poll<S: Status>(state: RwSignal<State>) {
    if state.with_untracked(|s| {
        let model = S::model(s);
        s.session_active()
            && model.submitted
            && model.reading.is_none()
            && (model.sending || model.status.as_ref().is_none_or(Status::running))
    }) {
        load::<S>(state);
    }
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/admin/registration_operation_tests.rs"]
pub(super) mod tests;
