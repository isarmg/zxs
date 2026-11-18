//! Read-only lifecycle observations are separate from content-node ancestry.
use crate::{
    api,
    files::{history::DecodeError, page::Entry},
    ui::{State, accept_read_response},
};
use std::sync::Arc;
use storage_contract::{
    OperationId,
    file_lifecycle::{Detail, Event, Page, PathLocation, Request, SourceId},
    history_window::{HistoryFileId, HistoryResolvedContext, HistoryRoot},
};
use web_interface::{i18n::t, prelude::*, tasks::spawn_local};

mod interval;
mod navigation;
mod rail;

const PAGE_SIZE: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq)]
struct Target {
    epoch: u64,
    navigation: u64,
    entry: Entry,
    file: HistoryFileId,
    root: HistoryRoot,
    context: HistoryResolvedContext,
}
impl Target {
    fn capture(state: &State) -> Option<Self> {
        if !state.session_active() || !state.files.file_history.ready() {
            return None;
        }
        let entry = state.files.selected.as_ref()?;
        let identity = state.files.file_history.identity()?;
        if entry.kind() != storage_contract::NamespaceEntryKind::File
            || entry.file_id() != identity.file_id.as_str()
        {
            return None;
        }
        Some(Self {
            epoch: state.session_epoch,
            // Every disk/directory navigation cancels the listing generation,
            // even when a different location exposes the same retained file.
            navigation: state.files.generation,
            entry: entry.clone(),
            file: identity.file_id.clone(),
            root: identity.history_root,
            context: identity.context.clone(),
        })
    }

    fn request(&self, cursor: Option<&str>) -> Result<Request, api::Error> {
        let location = (!self.entry.preview())
            .then(|| self.entry.context())
            .flatten();
        Request::parse(
            self.file.as_str(),
            &self.root.to_string(),
            location.map(|(repository, _)| repository),
            location.map(|(_, path)| path),
            cursor,
            PAGE_SIZE,
        )
        .map_err(|source| DecodeError::Name(source).into())
    }
}

#[derive(Clone, Default)]
struct Model {
    target: Option<Target>,
    generation: u64,
    loading: bool,
    cancellation: Option<api::ReadCancellation>,
    page: Option<Arc<Page>>,
    page_request: Option<Request>,
    located: Option<crate::files::history::focus::Anchor>,
    error: Option<api::Error>,
}
impl Model {
    fn cancel(&mut self) {
        if let Some(cancellation) = self.cancellation.take() {
            cancellation.abort();
        }
        self.generation += 1;
        self.loading = false;
    }

    fn set_target(&mut self, target: Option<Target>) {
        if self.target == target {
            return;
        }
        self.cancel();
        self.target = target;
        self.page = None;
        self.page_request = None;
        self.error = None;
    }

    fn request(&self, next: bool) -> Result<Option<Request>, api::Error> {
        let Some(target) = self.target.as_ref().filter(|_| !self.loading) else {
            return Ok(None);
        };
        let cursor = if next {
            let Some(cursor) = self.page.as_ref().and_then(|page| page.next_cursor.as_ref()) else {
                return Ok(None);
            };
            Some(cursor.to_string())
        } else {
            None
        };
        target.request(cursor.as_deref()).map(Some)
    }

    fn begin(&mut self) -> u64 {
        self.cancel();
        self.loading = true;
        self.error = None;
        self.generation
    }

    fn current(&self, target: &Target, generation: u64) -> bool {
        self.target.as_ref() == Some(target) && self.generation == generation
    }

    fn complete(&mut self, target: &Target, generation: u64, request: &Request, result: Result<Page, api::Error>) {
        if !self.current(target, generation) {
            return;
        }
        self.loading = false;
        self.cancellation = None;
        match result {
            Ok(page) => {
                // One bounded page replaces another. Pages never accumulate
                // into an apparently complete operation timeline.
                // An identical refresh may keep its DOM owner; after dismiss,
                // owner/route change or an error the same values get a new owner.
                if self.page_request.as_ref() != Some(request) || self.page.as_deref() != Some(&page) {
                    self.page = Some(Arc::new(page));
                }
                self.page_request = Some(request.clone());
                self.error = None;
            }
            Err(error) => {
                self.page = None;
                self.page_request = None;
                self.error = Some(error);
            }
        }
    }

    fn close(&mut self) {
        self.cancel();
        self.page = None;
        self.page_request = None;
        self.error = None;
    }
}

fn endpoint(request: &Request) -> String {
    let query = request
        .query_fields()
        .into_iter()
        .map(|(key, value)| format!("{}={}", api::segment(&key), api::segment(&value)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{}?{query}", request.route())
}

fn decode(
    value: json_codec::Value,
    request: &Request,
    context: &HistoryResolvedContext,
) -> Result<Page, api::Error> {
    let page: Page = json_codec::from_value(value).map_err(DecodeError::Wire)?;
    page.validate(request).map_err(DecodeError::Name)?;
    if &page.context != context {
        return Err(DecodeError::Relation("lifecycle page changed the history context").into());
    }
    Ok(page)
}

fn load(state: RwSignal<State>, model: RwSignal<Model>, next: bool) {
    let target = state.with_untracked(Target::capture);
    let request = model.with_untracked(|model| {
        if model.target != target {
            return Ok(None);
        }
        model.request(next)
    });
    let request = match request {
        Ok(Some(request)) => request,
        Ok(None) => return,
        Err(error) => {
            model.update(|model| model.error = Some(error));
            return;
        }
    };
    let Some(target) = target else {
        return;
    };
    let cancellation = match api::ReadCancellation::new() {
        Ok(cancellation) => cancellation,
        Err(error) => {
            model.update(|model| model.error = Some(error));
            return;
        }
    };
    navigation::Controls { state, model }.relinquish();
    let generation = model
        .try_update(|model| {
            let generation = model.begin();
            model.cancellation = Some(cancellation.clone());
            generation
        })
        .expect("invariant: mounted lifecycle panel owns its read state");
    spawn_local(async move {
        let result = api::cancellable_get(&endpoint(&request), cancellation)
            .await
            .and_then(|value| decode(value, &request, &target.context));
        // Even a dismissed response can reveal expiry of its current session.
        if !accept_read_response(state, target.epoch, &result)
            || state.try_with_untracked(Target::capture).flatten().as_ref() != Some(&target)
        {
            return;
        }
        let _ = model.try_update(|model| model.complete(&target, generation, &request, result));
    });
}

fn action_label(detail: &Detail) -> String {
    match detail {
        Detail::Delete { .. } => t("删除", "Deleted"),
        Detail::Undo { .. } => t("恢复", "Restored"),
        Detail::Import { .. } => t("导入已删除状态", "Imported an already-deleted state"),
        Detail::PathChanged { .. } => t("路径变更", "Path changed"),
        Detail::Merged { .. } => t("合并", "Merged"),
    }
}

fn event_identity(event: &Event) -> (OperationId, SourceId) {
    (event.operation_id, event.source_id())
}

fn event_anchor(file: &HistoryFileId, event: &Event) -> String {
    format!("lifecycle-{file}-{}-{}", event.operation_id, event.source_id())
}

fn event_link(anchor: &str) -> String {
    format!("#{anchor}")
}

fn location_label(location: &PathLocation) -> String {
    format!("{} / {}", location.repository_id, location.path)
}

fn detail_view(detail: &Detail) -> AnyView {
    match detail {
        Detail::Delete { deletion_id }
        | Detail::Undo { deletion_id }
        | Detail::Import { deletion_id } => {
            let label = format!(
                "{}: {deletion_id}",
                t("关联删除记录", "Related deletion record"),
            );
            view! {
                <p class="zxs-row">
                    {label}
                </p>
            }
            .into_any()
        }
        Detail::Merged { merge_id, survivor } => {
            view! { <div class="zxs-stack zxs-stack--flush">
                <p class="zxs-row">{format!("{}: {merge_id}", t("原合并记录", "Original merge record"))}</p>
                <p class="zxs-row">{format!("{}: {survivor}", t("保留的分支", "Surviving branch"))}</p>
            </div> }.into_any()
        }
        Detail::PathChanged {
            event_id,
            before,
            after,
        } => {
            let source = format!("{}: {event_id}", t("路径记录", "Path record"));
            let before = format!("{}: {}", t("变更前", "Before"), location_label(before));
            let after = format!("{}: {}", t("变更后", "After"), location_label(after));
            view! {
                <div class="zxs-stack zxs-stack--flush">
                    <p class="zxs-row">
                        {source}
                    </p>
                    <p class="zxs-row">
                        {before}
                    </p>
                    <p class="zxs-row">
                        {after}
                    </p>
                </div>
            }
            .into_any()
        }
    }
}

fn event_time(event: &Event) -> String {
    let Some(timestamp) = event.occurred_at else {
        return t("时间未记录", "Time not recorded");
    };
    let nanos = i128::from(timestamp.unix_seconds()) * 1_000_000_000;
    match date_time::OffsetDateTime::from_unix_timestamp_nanos(nanos) {
        Ok(time) => format!(
            "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
            time.year(),
            u8::from(time.month()),
            time.day(),
            time.hour(),
            time.minute(),
            time.second(),
        ),
        // Calendar formatting has a smaller range than the stored timestamp.
        // Keep the exact recorded seconds rather than hiding a valid event.
        Err(_) => format!(
            "{}: {}",
            t("记录时间（Unix 秒）", "Recorded time (Unix seconds)"),
            timestamp.unix_seconds(),
        ),
    }
}

fn page_status(page: &Page) -> String {
    if page.events.is_empty() {
        t(
            "此页没有已验证的操作记录。",
            "This page has no verified lifecycle records.",
        )
    } else if page.next_cursor.is_some() {
        t("后面还有保留记录。", "More retained records are available.")
    } else {
        t(
            "已到本次观察中保留记录的末页。",
            "End of the retained records in this observation.",
        )
    }
}

pub(super) fn view(state: RwSignal<State>) -> impl IntoView {
    let model = RwSignal::new(Model::default());
    let target = Memo::new(move |_| state.with(Target::capture));
    let controls = navigation::Controls { state, model };
    Effect::new(move |_| {
        let selected = target.get();
        if model.with_untracked(|m| m.target != selected) {
            controls.relinquish();
            model.update(|m| m.set_target(selected));
        }
    });
    on_cleanup(move || {
        controls.relinquish();
        let _ = model.try_update(Model::cancel);
    });
    let disabled =
        Signal::derive(move || target.get().is_none() || model.with(|model| model.loading));
    let busy = move || model.with(|model| model.loading).to_string();
    let disabled = move || disabled.get();
    let next_disabled = move || {
        disabled() || model.with(|model| {
            model.page.as_ref().is_none_or(|page| page.next_cursor.is_none())
        })
    };
    let dismiss_disabled = move || model.with(|model| {
        !model.loading && model.page.is_none() && model.error.is_none()
    });
    let load_first = move |_| load(state, model, false);
    let load_next = move |_| load(state, model, true);
    let dismiss = move |_| { controls.relinquish(); model.update(Model::close); };
    let error_visible = move || model.with(|model| model.error.is_some());
    let error_message = move || model.with(|model| {
        model.error.as_ref().map(ToString::to_string).unwrap_or_default()
    });
    let origin = Memo::new(move |_| {
        let current = target.get();
        model.with(|m| {
            (m.target == current).then(|| navigation::Origin::capture(m)).flatten()
        })
    });
    let visible_page = rail::page_view(navigation::Controls { state, model }, origin);
    view! {
        <section
            class="zxs-entry-section zxs-file-lifecycle zxs-stack"
            aria-busy=busy
        >
            <h3 class="zxs-row">
                {t("文件操作与内容时间线", "File operations and content timeline")}
            </h3>
            <p class="zxs-row">
                {t(
                    concat!(
                        "只显示已验证的删除、恢复、已删除状态导入、合并及本容器路径变更。",
                        "复制保留的原路径来源不在此展示，也不冒充本容器新操作。",
                        "操作有独立身份，不是内容版本或可写节点。",
                    ),
                    concat!(
                        "Shows verified deletion, restoration, already-deleted import ",
                        "merges and this container's path changes. ",
                        "Copied path sources remain audit evidence, not records displayed here ",
                        "or new operations on this container. ",
                        "Record identities are not content versions or writable nodes.",
                    ),
                )}
            </p>
            <p class="zxs-row">
                {t(
                    concat!(
                        "按保留记录序号分页，不代表跨来源的时钟顺序。",
                        "未记录的时间保持未知；",
                        "路径记录只展示真实前后位置，不猜测改名或移动类型，",
                        "也不代表完整操作时间线。",
                    ),
                    concat!(
                        "Pages use retained record sequence, not a clock order across sources. ",
                        "Unrecorded times stay unknown. Path records show actual before/after ",
                        "locations without guessing rename or move. This is not a complete timeline.",
                    ),
                )}
            </p>
            <div class="zxs-file-toolbar zxs-flow">
                <button
                    class="zxs-button"
                    disabled=disabled
                    on:click=load_first
                >
                    {t("查看／刷新记录", "View / refresh records")}
                </button>
                <button
                    class="zxs-button"
                    disabled=next_disabled
                    on:click=load_next
                >
                    {t("下一页记录", "Next records page")}
                </button>
                <button
                    class="zxs-button"
                    disabled=dismiss_disabled
                    on:click=dismiss
                >
                    {t("收起记录", "Dismiss records")}
                </button>
            </div>
            <Show when=error_visible>
                <p class="zxs-row zxs-inline-error" role="status">
                    {error_message}
                </p>
            </Show>
            {visible_page}
        </section>
    }
}

#[cfg(test)]
#[path = "../../../../test/web/unit/files/lifecycle_tests.rs"]
mod tests;
