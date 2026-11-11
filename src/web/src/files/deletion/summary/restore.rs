use crate::api;
use crate::files::actions;
use crate::files::deletion::summary::{Target, summary_endpoint, unchanged};
use crate::files::deletion::{request, restore};
use crate::ui::{State, accept_read_response, fail};
use std::sync::Arc;
use storage_contract::file_deletion_review::Summary;
use web_interface::{prelude::*, tasks::spawn_local};

#[derive(Clone)]
pub(in crate::files::deletion) struct Review {
    pub(super) target: Target,
    pub(super) generation: u64,
}
fn allowed(state: &State, target: &Target) -> bool {
    target.current(state) && state.management.storage.writable() && !actions::pending(state)
}
pub(super) fn begin(state: RwSignal<State>, target: Target) {
    let Some((target, review_generation)) = state
        .try_update(|s| {
            if !allowed(s, &target) {
                return None;
            }
            let target = target.begin(s)?;
            Some((target, s.files.deletion.review_generation))
        })
        .expect("invariant: mounted summary restore owns its state")
    else {
        return;
    };
    spawn_local(async move {
        let endpoint = summary_endpoint(&target.summary);
        let result = api::validated_request("GET", &endpoint, None, |value| {
            unchanged(value, &target.summary)
        })
        .await;
        if !accept_read_response(state, target.epoch, &result)
            || !state.with_untracked(|s| target.matches(s))
        {
            return;
        }
        state.update(|s| s.files.deletion.loading = false);
        if !state.with_untracked(|s| s.files.deletion.review_generation == review_generation) {
            return;
        }
        match result {
            Ok(_) => state.update(|s| {
                s.files.deletion.restore_review = Some(restore::Review::Summary(Review {
                    target,
                    generation: review_generation,
                }));
            }),
            Err(error) => fail(state, error),
        }
    });
}
pub(super) fn reviewed(state: &State, epoch: u64, generation: u64) -> Option<Arc<Summary>> {
    let restore::Review::Summary(review) = state.files.deletion.restore_review.as_ref()? else {
        return None;
    };
    (review.target.epoch == epoch
        && review.generation == generation
        && state.files.deletion.review_generation == generation
        && allowed(state, &review.target))
    .then(|| review.target.summary.clone())
}
pub(super) fn confirm(state: RwSignal<State>, epoch: u64, generation: u64) {
    let Some(summary) = state.with_untracked(|s| reviewed(s, epoch, generation)) else {
        return;
    };
    state.update(|s| s.files.deletion.cancel_review());
    request::submit(state, request::Intent::UndoSummary(summary));
}
pub(super) fn can_review(state: &State, target: &Target) -> bool {
    allowed(state, target)
}

pub(super) fn cancel(state: RwSignal<State>, epoch: u64, generation: u64) {
    state.update(|s| {
        if s.session_epoch == epoch
            && s.files
                .deletion
                .restore_review
                .as_ref()
                .is_some_and(|review| {
                    matches!(review, restore::Review::Summary(review)
                        if review.generation == generation)
                })
        {
            s.files.deletion.cancel_review();
        }
    });
}
