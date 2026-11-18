//! A content identity is useful only in the original read's observation and
//! location. Neither a raw node ID nor a newer authorized page can substitute.
use super::*;
use crate::files::{actions::UninstalledAnchor, inline};
use storage_contract::{
    FileNodeId,
    file_lifecycle::{EventKey, IntervalBody, LinkRole, References},
};

#[derive(Clone, PartialEq, Eq)]
pub(super) enum Proof {
    Event { event: EventKey, role: LinkRole, node: FileNodeId },
    Section { older: EventKey, newer: EventKey, node: FileNodeId },
}
impl Proof {
    fn node(&self) -> FileNodeId {
        match self { Self::Event { node, .. } | Self::Section { node, .. } => *node }
    }
    pub(super) fn present(&self, page: &Page) -> bool {
        match self {
            Self::Event { event, role, node } => page.events.iter().any(|row| {
                row.event_key() == *event && matches!(&row.references, References::Complete { links }
                    if links.iter().any(|link| &link.role == role && link.node == Some(*node)))
            }),
            Self::Section { older, newer, node } => {
                page.events.windows(2).any(|pair| pair[0].event_key() == *older && pair[1].event_key() == *newer)
                    && page.intervals.iter().any(|section|
                        section.older == *older && section.newer == *newer
                        && matches!(&section.body, IntervalBody::Complete { nodes, .. }
                            if nodes.iter().any(|row| row.node == *node)))
            }
        }
    }
}

#[derive(Clone)]
pub(super) struct Origin {
    pub(super) target: Target,
    pub(super) request: Request,
    pub(super) page: Arc<Page>,
}
impl PartialEq for Origin {
    fn eq(&self, other: &Self) -> bool {
        self.target == other.target && self.request == other.request && Arc::ptr_eq(&self.page, &other.page)
    }
}
impl Eq for Origin {}
impl Origin {
    pub(super) fn capture(model: &Model) -> Option<Self> {
        Some(Self {
            target: model.target.clone()?,
            request: model.page_request.clone()?,
            page: Arc::clone(model.page.as_ref()?),
        })
    }
    fn current(&self, model: &Model) -> bool {
        model.target.as_ref() == Some(&self.target)
            && model.page_request.as_ref() == Some(&self.request)
            && model.page.as_ref().is_some_and(|page| Arc::ptr_eq(page, &self.page))
    }
    fn readback(&self, response: &Page, proof: &Proof) -> Result<(), api::Error> {
        if self.page.as_ref() == response && proof.present(response) { return Ok(()); }
        Err(api::Error {
            status: 409, code: storage_contract::ErrorCode::HistoryChanged,
            message: t("原操作或观察已改变，请刷新记录后重选。", "The original operation or observation changed. Refresh records and select it again."),
            retryable: false, request_id: None,
            source: Some(Arc::new(api::ErrorSource::FileHistory(DecodeError::Name(
                storage_contract::file_lifecycle::LifecycleError::Changed.into_error(),
            )))),
        })
    }
}

#[derive(Clone, Copy)]
pub(super) struct Controls {
    pub(super) state: RwSignal<State>,
    pub(super) model: RwSignal<Model>,
}

impl Controls {
    pub(super) fn relinquish(self) {
        if let Some(old) = self.model.try_update(|m| m.located.take()).flatten() {
            let _ = self.state.try_update(|s| {
                if s.files.history_focus.as_ref() == Some(&old) { s.files.history_focus = None; }
            });
        }
    }
    fn same(self, origin: &Origin, generation: u64, anchor: &UninstalledAnchor) -> bool {
        self.state.try_with_untracked(|s|
            Target::capture(s).as_ref() == Some(&origin.target) && anchor.current(s)).unwrap_or(false)
            && self.model.try_with_untracked(|m| m.current(&origin.target, generation) && origin.current(m))
                .unwrap_or(false)
    }

    pub(super) fn jump(self, origin: &Origin, proof: &Proof) {
        let Self { state, model } = self;
        if !model.try_with_untracked(|m| !m.loading && origin.current(m)).unwrap_or(false)
            || state.try_with_untracked(Target::capture).flatten().as_ref() != Some(&origin.target)
            || !proof.present(&origin.page) || !inline::history_navigation_allowed(state) { return; }
        let anchor = match state.with_untracked(|s| UninstalledAnchor::capture(s, &proof.node().to_string())) {
            Ok(Some(anchor)) => anchor,
            Ok(None) => return,
            Err(error) => { model.update(|m| m.error = Some(error)); return; }
        };
        let first = match api::ReadCancellation::new() {
            Ok(first) => first,
            Err(error) => { model.update(|m| m.error = Some(error)); return; }
        };
        self.relinquish();
        let generation = model.try_update(|m| {
            let g = m.begin();
            m.cancellation = Some(first.clone());
            g
        });
        let Some(generation) = generation else { return };
        let origin = origin.clone();
        let proof = proof.clone();
        spawn_local(async move {
            let result = async {
                let located = anchor.receive(first).await;
                if !accept_read_response(state, origin.target.epoch, &located)
                    || !self.same(&origin, generation, &anchor) { return Ok(()); }
                let located = located?;
                // Never reuse the first fetch's deadline/AbortController.
                let second = api::ReadCancellation::new()?;
                model.update(|m| m.cancellation = Some(second.clone()));
                let confirmed = api::cancellable_get(&endpoint(&origin.request), second).await
                    .and_then(|wire| decode(wire, &origin.request, &origin.target.context));
                if !accept_read_response(state, origin.target.epoch, &confirmed)
                    || !self.same(&origin, generation, &anchor) { return Ok(()); }
                origin.readback(&confirmed?, &proof)?;
                // No await between proof comparison and the final editor/owner
                // fence; ordinary navigation never receives this detached page.
                if let Some(installed) = state.try_update(|s| {
                    Ok::<_,api::Error>(anchor.install(s, located)?.then(||s.files.history_focus.clone()).flatten())
                }) {
                    let located = installed?;
                    let _ = model.try_update(|m| m.located = located);
                }
                Ok::<_, api::Error>(())
            }.await;
            let _ = model.try_update(|m| {
                if m.current(&origin.target, generation) && origin.current(m) {
                    m.loading = false;
                    m.cancellation = None;
                    m.error = result.err();
                    if m.error.is_some() {
                        // Do not keep labels/principals or clickable old relations
                        // after a failed original-source authorization/readback.
                        m.page = None;
                        m.page_request = None;
                    }
                }
            });
        });
    }

    // Interval cards retain their element/scroll when a new observation has
    // identical verified topology. Only the *displayed, current* observation
    // may reauthorize a click on that still-present original relation.
    pub(super) fn section_button(self, origin: Memo<Option<Origin>>, proof: Proof) -> impl IntoView {
        let id = proof.node().to_string();
        let label = format!("{}: {id}", t("定位内容", "Locate content"));
        let disabled = move || self.model.with(|m| m.loading)
            || self.state.with(|s| !crate::files::actions::history_navigation_ready(s));
        view! { <button class="zxs-button zxs-lifecycle-node-link" disabled=disabled
            data-zxs-related-node=id on:click=move |_| {
                if let Some(shown) = origin.get_untracked() { self.jump(&shown, &proof); }
            }>{label}</button> }
    }

    pub(super) fn button(self, origin: Origin, proof: Proof, label: String) -> impl IntoView {
        let disabled = move || self.model.with(|m| m.loading)
            || self.state.with(|s| !crate::files::actions::history_navigation_ready(s));
        let id = proof.node().to_string();
        view! {
            <button class="zxs-button zxs-lifecycle-node-link" data-zxs-related-node=id
                disabled=disabled on:click=move |_| self.jump(&origin, &proof)>{label}</button>
        }
    }
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/lifecycle_navigation.rs"]
mod tests;
