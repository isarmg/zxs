//! Workspace feedback belongs to the current visit and admitted operation.
//! Receipts and error diagnostics retain their independent recovery lifetimes.
use super::{Page, State};
use crate::files::{SaveStatus, publication_notice};
use web_interface::{i18n::t, prelude::*};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NoticeId {
    session_epoch: u64,
    sequence: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Success,
    Warning,
    OperationWarning,
}

#[derive(Clone, PartialEq, Eq)]
struct Notice {
    id: NoticeId,
    message: String,
    kind: Kind,
}

#[derive(Clone)]
struct Operation {
    id: String,
    status: Option<SaveStatus>,
    success: Option<String>,
    current_visit: bool,
}

#[derive(Clone, Default)]
pub(crate) struct Notices {
    sequence: u64,
    visible: Option<Notice>,
    operation: Option<Operation>,
}

impl Notices {
    pub(crate) fn clear(&mut self) {
        self.visible = None;
        self.operation = None;
    }

    pub(crate) fn leave(&mut self) {
        if !self
            .visible
            .as_ref()
            .is_some_and(|notice| notice.kind == Kind::OperationWarning)
        {
            self.visible = None;
        }
        // An unresolved outcome or reduced protection remains actionable even
        // elsewhere. A later ordinary success settles it without reviving the
        // previous visit's toast, including navigation away and back.
        if let Some(operation) = self.operation.as_mut() {
            operation.current_visit = false;
        }
    }

    pub(crate) fn begin_operation(&mut self, id: String) {
        self.visible = None;
        self.operation = Some(Operation {
            id,
            status: None,
            success: None,
            current_visit: true,
        });
    }

    pub(crate) fn operation_success(&mut self, message: String) {
        self.operation
            .as_mut()
            .expect("invariant: operation was admitted")
            .success = Some(message);
    }

    pub(crate) fn record_operation(&mut self, epoch: u64, id: &str, status: SaveStatus) {
        let Some(operation) = self
            .operation
            .as_mut()
            .filter(|operation| operation.id == id)
        else {
            return;
        };
        if operation.status == Some(status) {
            return;
        }
        operation.status = Some(status);
        if status == SaveStatus::Published && !operation.current_visit {
            self.clear_operation_notice(id);
            return;
        }
        let (message, kind) = if status == SaveStatus::Published {
            (
                operation
                    .success
                    .clone()
                    .unwrap_or_else(|| publication_notice(status)),
                Kind::Success,
            )
        } else {
            // Reduced protection is not an ordinary success. Never replace its
            // warning with the directory-restored success copy.
            (publication_notice(status), Kind::OperationWarning)
        };
        self.show(epoch, message, kind);
    }

    fn show(&mut self, epoch: u64, message: String, kind: Kind) {
        self.sequence += 1;
        self.visible = Some(Notice {
            id: NoticeId {
                session_epoch: epoch,
                sequence: self.sequence,
            },
            message,
            kind,
        });
    }

    pub(crate) fn clear_operation_notice(&mut self, id: &str) {
        if self
            .operation
            .as_ref()
            .is_some_and(|operation| operation.id == id)
        {
            self.visible = None;
        }
    }

    pub(crate) fn clear_success(&mut self) {
        if self
            .visible
            .as_ref()
            .is_some_and(|notice| notice.kind == Kind::Success)
        {
            self.visible = None;
        }
    }

    fn dismiss(&mut self, id: NoticeId) {
        if self.visible.as_ref().is_some_and(|notice| notice.id == id) {
            self.visible = None;
        }
    }

    #[cfg(any(target_arch = "wasm32", test))]
    fn expiry(&self) -> Option<NoticeId> {
        self.visible
            .as_ref()
            .filter(|notice| notice.kind == Kind::Success)
            .map(|notice| notice.id)
    }
}

impl State {
    pub(crate) fn change_page(&mut self, page: Page) {
        if self.page != page {
            self.notices.leave();
            self.page = page;
        }
    }

    pub(crate) fn set_file_notice(&mut self, message: String) {
        if matches!(self.page, Page::Files | Page::Details) {
            self.notices
                .show(self.session_epoch, message, Kind::Warning);
        }
    }
}

pub(super) fn view(state: RwSignal<State>) -> impl IntoView {
    install_expiry(state);
    let current = Memo::new(move |_| state.with(|s| s.notices.visible.clone()));
    view! {
        {move || current.get().map(move |notice| {
            let id = notice.id;
            view! {
                <div class="zxs-inline-feedback zxs-flow">
                    <p class="zxs-status zxs-row" role=if notice.kind == Kind::Success { "status" } else { "alert" }>{notice.message}</p>
                    <button class="zxs-button" type="button" aria-label=t("关闭操作提示", "Dismiss operation notice")
                        on:click=move |_|state.update(|s|s.notices.dismiss(id))>"×"</button>
                </div>
            }
        })}
    }
}

#[cfg(target_arch = "wasm32")]
fn install_expiry(state: RwSignal<State>) {
    use gloo_timers::callback::Timeout;
    use send_wrapper::SendWrapper;
    const SUCCESS_EXPIRY_MS: u32 = 5_000;
    let expiry = Memo::new(move |_| state.with(|s| s.notices.expiry()));
    let timer = StoredValue::new(SendWrapper::new(None::<Timeout>));
    Effect::new(move |_| {
        let id = expiry.get();
        timer.update_value(|timer| {
            // Replacing or dismissing a notice cancels the previous timer.
            **timer = id.map(|id| {
                Timeout::new(SUCCESS_EXPIRY_MS, move || {
                    let _ = state.try_update(|s| s.notices.dismiss(id));
                })
            });
        });
    });
    on_cleanup(move || {
        let _ = timer.try_update_value(|timer| {
            (**timer).take();
        });
    });
}

#[cfg(not(target_arch = "wasm32"))]
fn install_expiry(_state: RwSignal<State>) {}

#[cfg(test)]
#[path = "../../../../test/web/unit/ui/notices/tests.rs"]
mod tests;
