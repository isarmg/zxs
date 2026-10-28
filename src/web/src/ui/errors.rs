//! Bounded notices keyed by the work that failed, never by a request attempt.
use super::{State, feedback};
use crate::api;
use web_interface::{components::Feedback, prelude::*};

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ErrorContext {
    Operation(String),
    Read(String),
}

#[derive(Clone, PartialEq, Eq)]
struct ErrorKey {
    session_epoch: u64,
    context: ErrorContext,
    code: storage_contract::ErrorCode,
    status: u16,
}

#[derive(Clone)]
pub(crate) struct ErrorNotice {
    pub(super) id: NoticeId,
    key: Option<ErrorKey>,
    pub(crate) feedback: Feedback,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct NoticeId {
    session_epoch: u64,
    sequence: u64,
}

impl State {
    fn report_error(&mut self, error: api::Error, context: Option<(u64, ErrorContext)>) {
        if context
            .as_ref()
            .is_some_and(|(epoch, _)| *epoch != self.session_epoch)
        {
            return;
        }
        if error.status == 401 {
            self.authenticated = false;
        }
        let key = context.map(|(session_epoch, context)| ErrorKey {
            session_epoch,
            context,
            code: error.code,
            status: error.status,
        });
        let existing = key.as_ref().and_then(|key| {
            self.errors
                .iter()
                .position(|notice| notice.key.as_ref() == Some(key))
        });
        let id = if let Some(index) = existing {
            self.errors.remove(index).id
        } else {
            self.next_error_id += 1;
            NoticeId {
                session_epoch: self.session_epoch,
                sequence: self.next_error_id,
            }
        };
        self.errors.insert(
            0,
            ErrorNotice {
                id,
                key,
                feedback: feedback(&error),
            },
        );
        self.errors.truncate(5);
    }

    pub(super) fn dismiss_error(&mut self, id: NoticeId) {
        self.errors.retain(|notice| notice.id != id);
    }
}

/// Without an explicit work identity, distinct user actions must stay distinct.
pub(crate) fn fail(state: RwSignal<State>, error: api::Error) {
    state.update(|state| state.report_error(error, None));
}

pub(crate) fn fail_in(
    state: RwSignal<State>,
    epoch: u64,
    context: ErrorContext,
    error: api::Error,
) {
    state.update(|state| state.report_error(error, Some((epoch, context))));
}

#[cfg(test)]
#[path = "../../../../test/web/unit/ui/errors/tests.rs"]
mod tests;
