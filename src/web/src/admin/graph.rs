//! One replaceable history graph page and one replaceable metadata detail.
mod view;
mod wire;
use crate::{
    api,
    snapshot::Snapshot,
    ui::{State, accept_read_response},
};
use storage_contract::history_graph::*;
use web_interface::{prelude::*, tasks::spawn_local};

#[derive(Clone, Debug, PartialEq, Eq)]
enum DetailRequest {
    Text {
        target: TextTarget,
        offset: u64,
    },
    Pending {
        target: PendingTarget,
        after: Option<String>,
    },
    Deletions {
        commit: String,
        offset: u64,
    },
}
#[derive(Debug)]
enum Detail {
    Text(TextPage),
    Pending(PendingPage),
    Deletions(DeletionsPage),
}
impl Detail {
    fn has_next(&self) -> bool {
        match self {
            Self::Text(page) => page.next_offset.is_some(),
            Self::Pending(page) => page.next_after.is_some(),
            Self::Deletions(page) => page.next_offset.is_some(),
        }
    }
    fn next(&self) -> Option<DetailRequest> {
        match self {
            Self::Text(page) => page.next_offset.map(|offset| DetailRequest::Text {
                target: page.target.clone(),
                offset,
            }),
            Self::Pending(page) => page
                .next_after
                .as_ref()
                .map(|after| DetailRequest::Pending {
                    target: page.target.clone(),
                    after: Some(after.clone()),
                }),
            Self::Deletions(page) => page.next_offset.map(|offset| DetailRequest::Deletions {
                commit: page.commit_id.clone(),
                offset,
            }),
        }
    }
    fn pretty(&self) -> String {
        match self {
            Self::Text(page) => pretty(page),
            Self::Pending(page) => pretty(page),
            Self::Deletions(page) => pretty(page),
        }
    }
}
fn pretty(value: &impl data_serialization::Serialize) -> String {
    json_codec::to_string_pretty(value).expect("invariant: graph wire types serialize")
}
#[derive(Default)]
struct ReadSlot {
    ticket: u64,
    cancellation: Option<api::ReadCancellation>,
}
impl ReadSlot {
    fn cancel(&mut self) {
        if let Some(cancellation) = self.cancellation.take() {
            cancellation.abort();
        }
        self.ticket += 1;
    }
    fn begin(&mut self, cancellation: api::ReadCancellation) -> u64 {
        self.cancel();
        self.cancellation = Some(cancellation);
        self.ticket
    }
}
struct Model {
    history: String,
    collection: GraphCollection,
    page: Option<Snapshot<GraphPage>>,
    detail: Option<Snapshot<Detail>>,
    main_read: ReadSlot,
    detail_read: ReadSlot,
    error: Option<api::Error>,
    stale: bool,
}
#[derive(Clone, Copy)]
enum Ticket {
    Main(u64),
    Detail(u64),
}
fn accepts<T>(
    state: RwSignal<State>,
    model: RwSignal<Model>,
    epoch: u64,
    ticket: Ticket,
    result: &Result<T, api::Error>,
) -> bool {
    accept_read_response(state, epoch, result)
        && model
            .try_with_untracked(|m| match ticket {
                Ticket::Main(ticket) => m.main_read.ticket == ticket,
                Ticket::Detail(ticket) => m.detail_read.ticket == ticket,
            })
            .unwrap_or(false)
}
impl Model {
    fn new(history: String) -> Self {
        Self {
            history,
            collection: GraphCollection::Commits,
            page: None,
            detail: None,
            main_read: ReadSlot::default(),
            detail_read: ReadSlot::default(),
            error: None,
            stale: false,
        }
    }
    fn cancel(&mut self) {
        self.main_read.cancel();
        self.detail_read.cancel();
    }
    fn reject(&mut self, error: api::Error) {
        if error.code == storage_contract::ErrorCode::HistoryChanged {
            self.stale = true;
            self.detail_read.cancel();
            self.detail = None;
        }
        self.error = Some(error);
    }
}
fn base(history: &str) -> String {
    format!("/api/history-trees/{}/graph", api::segment(history))
}
fn main_path(history: &str, collection: GraphCollection, cursor: Option<&str>) -> String {
    let mut path = format!(
        "{}?collection={}&limit=50",
        base(history),
        collection.as_str()
    );
    if let Some(cursor) = cursor {
        path.push_str(&format!("&cursor={}", api::segment(cursor)));
    }
    path
}
fn detail_path(history: &str, root: &str, request: &DetailRequest) -> String {
    let (suffix, query) = match request {
        DetailRequest::Text { target, offset } => {
            let target = match target {
                TextTarget::History { field } => format!("history/fields/{}", field.as_str()),
                TextTarget::Commit { id, field } => {
                    format!("commits/{}/fields/{}", api::segment(id), field.as_str())
                }
                TextTarget::MergeLink { id, field } => {
                    format!("merge-links/{}/fields/{}", api::segment(id), field.as_str())
                }
                TextTarget::Boundary { id, field } => {
                    format!("boundaries/{}/fields/{}", api::segment(id), field.as_str())
                }
            };
            (
                target,
                format!("offset={offset}&limit={MAX_GRAPH_TEXT_BYTES}"),
            )
        }
        DetailRequest::Pending { target, after } => {
            let target = match target {
                PendingTarget::Commit { id } => format!("commits/{}/pending", api::segment(id)),
                PendingTarget::Branch { id } => format!("branches/{}/pending", api::segment(id)),
            };
            let mut query = format!("limit={MAX_GRAPH_RECORDS}");
            if let Some(after) = after {
                query.push_str(&format!("&after={}", api::segment(after)));
            }
            (target, query)
        }
        DetailRequest::Deletions { commit, offset } => (
            format!("commits/{}/deletions", api::segment(commit)),
            format!("offset={offset}&limit={MAX_GRAPH_RECORDS}"),
        ),
    };
    format!(
        "{}/{suffix}?history_root={}&{query}",
        base(history),
        api::segment(root)
    )
}
fn load_main(
    state: RwSignal<State>,
    model: RwSignal<Model>,
    collection: GraphCollection,
    cursor: Option<String>,
) {
    if !state.with_untracked(State::session_active) {
        return;
    }
    let cancellation = match api::ReadCancellation::new() {
        Ok(cancellation) => cancellation,
        Err(error) => {
            model.update(|m| m.reject(error));
            return;
        }
    };
    let epoch = state.with_untracked(|s| s.session_epoch);
    let (history, root, ticket) = model
        .try_update(|m| {
            let root = cursor.as_ref().map(|_| {
                m.page
                    .as_ref()
                    .expect("invariant: next graph cursor comes from current page")
                    .history_root
                    .clone()
            });
            if m.collection != collection {
                m.page = None;
            }
            m.collection = collection;
            m.detail_read.cancel();
            m.detail = None;
            m.error = None;
            (
                m.history.clone(),
                root,
                m.main_read.begin(cancellation.clone()),
            )
        })
        .expect("invariant: graph request starts in its live owner");
    let path = main_path(&history, collection, cursor.as_deref());
    spawn_local(async move {
        let result = api::cancellable_get_with(&path, cancellation, |status, bytes| {
            wire::main_page(status, bytes, &history, collection, root.as_deref())
        })
        .await;
        if !accepts(state, model, epoch, Ticket::Main(ticket), &result) {
            return;
        }
        model.update(|m| {
            m.main_read.cancellation = None;
            match result {
                Ok(page) => {
                    m.page = Some(page.into());
                    m.stale = false;
                }
                Err(error) => m.reject(error),
            }
        });
    });
}
fn load_detail(state: RwSignal<State>, model: RwSignal<Model>, request: DetailRequest) {
    if !state.with_untracked(State::session_active) {
        return;
    }
    let Some((history, root)) = model.with_untracked(|m| {
        if m.stale || m.main_read.cancellation.is_some() {
            return None;
        }
        m.page
            .as_ref()
            .map(|page| (m.history.clone(), page.history_root.clone()))
    }) else {
        return;
    };
    let cancellation = match api::ReadCancellation::new() {
        Ok(cancellation) => cancellation,
        Err(error) => {
            model.update(|m| m.reject(error));
            return;
        }
    };
    let epoch = state.with_untracked(|s| s.session_epoch);
    let ticket = model
        .try_update(|m| {
            m.error = None;
            m.detail_read.begin(cancellation.clone())
        })
        .expect("invariant: graph detail request starts in its live owner");
    let path = detail_path(&history, &root, &request);
    spawn_local(async move {
        let result = api::cancellable_get_with(&path, cancellation, |status, bytes| {
            wire::detail(status, bytes, &history, &root, &request)
        })
        .await;
        if !accepts(state, model, epoch, Ticket::Detail(ticket), &result) {
            return;
        }
        model.update(|m| {
            m.detail_read.cancellation = None;
            match result {
                Ok(detail) => m.detail = Some(detail.into()),
                Err(error) => m.reject(error),
            }
        });
    });
}
pub(super) fn view(state: RwSignal<State>, history: String) -> impl IntoView {
    let model = RwSignal::new(Model::new(history));
    on_cleanup(move || model.update_untracked(Model::cancel));
    let epoch = state.with_untracked(|s| s.session_epoch);
    Effect::new(move |_| {
        let active = state.with(|s| s.session_active() && s.session_epoch == epoch);
        if !active {
            model.update(Model::cancel);
        }
    });
    load_main(state, model, GraphCollection::Commits, None);
    view::inspector(state, model)
}
#[cfg(test)]
#[path = "../../../../test/web/unit/admin/graph_tests.rs"]
mod tests;
