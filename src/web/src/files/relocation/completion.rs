//! A session-local shortcut after this editor's original move is published.
//! It grants no mutation authority and never repeats or polls a request.
use super::{Review, actions};
use crate::{files::SaveStatus, ui::State};
use web_interface::{i18n::t, prelude::*};

#[derive(Clone)]
pub(super) struct Destination {
    epoch: u64,
    principal: String,
    operation: String,
    slot: String,
    path: String,
}
impl Destination {
    pub(super) fn submitted(review: Review, operation: String) -> Self {
        Self {
            epoch: review.basis.epoch,
            principal: review.basis.principal,
            operation,
            slot: review.request.target_slot_id,
            path: review.destination,
        }
    }

    pub(super) fn owned_by(&self, owner: Option<&(u64, String)>) -> bool {
        owner.is_some_and(|(epoch, principal)| *epoch == self.epoch && principal == &self.principal)
    }

    fn status(&self, state: &State) -> Option<SaveStatus> {
        if !state.session_active()
            || state.session_epoch != self.epoch
            || state.principal != self.principal
        {
            return None;
        }
        state
            .files
            .operations
            .iter()
            .find(|operation| operation.id() == self.operation)
            .map(|operation| operation.status())
            .filter(|status| matches!(status, SaveStatus::Published | SaveStatus::Degraded))
    }

    fn can_open(&self, state: &State) -> bool {
        self.status(state).is_some()
            && !actions::pending(state)
            && !state.files.loading
            && state
                .files
                .disks
                .iter()
                .any(|disk| disk.slot_id == self.slot && disk.online)
    }
}

pub(super) fn view(
    state: RwSignal<State>,
    completion: RwSignal<Option<Destination>>,
) -> impl IntoView {
    view! {
        {move ||completion.get().map(|destination| {
            let label = format!("{} / {}", destination.slot, destination.path);
            let destination = StoredValue::new(destination);
            let status = Signal::derive(move ||state.with(|state| {
                destination.with_value(|destination|destination.status(state))
            }));
            view! {
                <Show when=move ||status.get().is_some()>
                    <section class="zxs-frame zxs-stack" data-zxs-move-destination="">
                        <p class="zxs-row" role="status">{move ||if status.get() == Some(SaveStatus::Degraded) {
                            t("移动已发布，但保护状态未完全满足，请查看操作提示。", "The move was published with a protection warning. Review the operation notice.")
                        } else {
                            t("移动已完成。", "Move completed.")
                        }}</p>
                        <p class="zxs-row">{label.clone()}</p>
                        <p class="zxs-row">{t("打开目标所在文件夹会读取当前位置，不会重新移动。", "Opening the destination folder reads its current contents without moving anything again.")}</p>
                        <div class="zxs-flow">
                            <button class="zxs-button" data-open-move-destination=""
                                disabled=move ||!state.with(|state|destination.with_value(|destination|destination.can_open(state)))
                                on:click=move |_| {
                                    let destination = destination.get_value();
                                    if completion.with_untracked(|current|current.as_ref().is_none_or(|current|current.operation != destination.operation))
                                        || !state.with_untracked(|state|destination.can_open(state))
                                    {
                                        return;
                                    }
                                    let parent = destination.path.rsplit_once('/').map_or("", |(parent, _)|parent).to_owned();
                                    completion.set(None);
                                    actions::open_disk_directory(state, destination.slot, parent);
                                }>{t("打开目标所在文件夹", "Open destination folder")}</button>
                            <button class="zxs-button" on:click=move |_| {
                                completion.update(|current| {
                                    if current.as_ref().is_some_and(|current|destination.with_value(|destination|current.operation == destination.operation)) {
                                        *current = None;
                                    }
                                });
                            }>{t("关闭提示", "Dismiss")}</button>
                        </div>
                    </section>
                </Show>
            }
        })}
    }
}
