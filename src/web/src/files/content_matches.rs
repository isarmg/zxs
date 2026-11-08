//! An explicitly requested, independent candidate page. Never a shared-tree claim.
use super::history::DecodeError;
use crate::{
    api,
    ui::{State, accept_read_response},
};
use storage_contract::content_matches::{ContentMatchPage, ContentMatchRequest};
use storage_contract::history_window::HistoryNodeId;
use web_interface::{
    components::{Select, SelectOption},
    i18n::t,
    prelude::*,
    tasks::spawn_local,
};

#[derive(Clone, Debug, PartialEq, Eq)]
struct Target {
    epoch: u64,
    file: String,
    entry: String,
    path: String,
    root: String,
}
impl Target {
    fn capture(state: &State) -> Option<Self> {
        if !state.session_active() || !state.files.file_history.ready() {
            return None;
        }
        let entry = state.files.selected.as_ref()?;
        Some(Self {
            epoch: state.session_epoch,
            file: entry.file_id().into(),
            entry: entry.entry_id().into(),
            path: entry.path().into(),
            root: state.files.file_history.root()?.into(),
        })
    }
}
#[derive(Clone, Default)]
struct Model {
    target: Option<Target>,
    node: Option<HistoryNodeId>,
    generation: u64,
    loading: bool,
    cancellation: Option<api::ReadCancellation>,
    page: Option<ContentMatchPage>,
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
    fn set_target(&mut self, target: Option<Target>, node: Option<HistoryNodeId>) {
        if self.target == target {
            return;
        }
        self.cancel();
        self.target = target;
        self.node = node;
        self.page = None;
        self.error = None;
    }
    fn select_node(&mut self, node: HistoryNodeId) {
        if self.node.as_ref() == Some(&node) {
            return;
        }
        self.cancel();
        self.node = Some(node);
        self.page = None;
        self.error = None;
    }
    fn current(&self, target: &Target, node: &str, generation: u64) -> bool {
        self.target.as_ref() == Some(target)
            && self
                .node
                .as_ref()
                .is_some_and(|selected| selected.as_str() == node)
            && self.generation == generation
    }
    fn request(&self, next: bool) -> Option<ContentMatchRequest> {
        let target = self.target.as_ref()?;
        let node = self.node.as_ref()?;
        let after = if next {
            Some(self.page.as_ref()?.next_after_version_id.as_ref()?.as_str())
        } else {
            None
        };
        Some(
            ContentMatchRequest::parse(&target.file, node.as_str(), &target.root, after, 16)
                .expect("invariant: selected admitted node and root form a bounded match request"),
        )
    }
    fn close(&mut self) {
        self.cancel();
        self.page = None;
        self.error = None;
    }
}
fn endpoint(request: &ContentMatchRequest) -> String {
    let query = request
        .query_fields()
        .into_iter()
        .map(|(k, v)| format!("{}={}", api::segment(&k), api::segment(&v)))
        .collect::<Vec<_>>()
        .join("&");
    format!("{}?{query}", request.route())
}
fn decode(
    value: json_codec::Value,
    request: &ContentMatchRequest,
) -> Result<ContentMatchPage, api::Error> {
    let page: ContentMatchPage = json_codec::from_value(value).map_err(DecodeError::Wire)?;
    page.validate(request).map_err(DecodeError::Name)?;
    Ok(page)
}
fn load(state: RwSignal<State>, model: RwSignal<Model>, next: bool) {
    let Some((target, request)) = state.with_untracked(|state| {
        let target = Target::capture(state)?;
        model.with_untracked(|model| {
            (model.target.as_ref() == Some(&target) && model.node.is_some())
                .then(|| model.request(next).map(|request| (target, request)))
                .flatten()
        })
    }) else {
        return;
    };
    let cancellation = match api::ReadCancellation::new() {
        Ok(value) => value,
        Err(error) => {
            model.update(|model| model.error = Some(error));
            return;
        }
    };
    let generation = model
        .try_update(|model| {
            model.cancel();
            model.loading = true;
            model.error = None;
            model.cancellation = Some(cancellation.clone());
            model.generation
        })
        .expect("invariant: mounted match panel owns its state");
    spawn_local(async move {
        let result = api::cancellable_get(&endpoint(&request), cancellation)
            .await
            .and_then(|value| decode(value, &request));
        // A closed or superseded panel cannot hide expiry of its current session.
        if !accept_read_response(state, target.epoch, &result)
            || !state
                .try_with_untracked(|state| Target::capture(state).as_ref() == Some(&target))
                .unwrap_or(false)
            || !model
                .try_with_untracked(|model| {
                    model.current(&target, request.node_id.as_str(), generation)
                })
                .unwrap_or(false)
        {
            return;
        }
        model.update(|model| {
            model.loading = false;
            model.cancellation = None;
            match result {
                Ok(page) => model.page = Some(page),
                Err(error) => {
                    model.page = None;
                    model.error = Some(error);
                }
            }
        });
    });
}
fn content_state(state: Option<storage_contract::ContentState>) -> String {
    match state {
        Some(storage_contract::ContentState::Available) => t(
            "记录保留正文，未检查当前健康",
            "Body recorded, current health not checked",
        ),
        Some(storage_contract::ContentState::Purged) => t(
            "正文已清除，匹配元数据保留",
            "Body purged, matching metadata retained",
        ),
        Some(storage_contract::ContentState::Migrated) => t("正文已迁出", "Body migrated"),
        None => t("正文状态未知", "Body state unknown"),
    }
}
pub(super) fn view(state: RwSignal<State>) -> impl IntoView {
    let model = RwSignal::new(Model::default());
    let target = Memo::new(move |_| state.with(Target::capture));
    Effect::new(move |_| {
        let target = target.get();
        let node = state.with_untracked(|state| {
            state.files.file_history.current_node_id().map(|node| {
                HistoryNodeId::parse(node).expect("invariant: selected history node was admitted")
            })
        });
        model.update(|model| model.set_target(target, node));
    });
    on_cleanup(move || {
        let _ = model.try_update(Model::cancel);
    });
    let options = Signal::derive(move || {
        state.with(|state| {
            let mut options = state
                .files
                .file_history
                .nodes()
                .iter()
                .map(|node| SelectOption::new(node.id(), super::node_card::file_title(node)))
                .collect::<Vec<_>>();
            model.with(|model| {
                if let Some(node) = &model.node
                    && !options.iter().any(|option| option.value == node.as_str())
                {
                    options.push(SelectOption::new(
                        node.as_str(),
                        t(
                            "选中节点（不在此历史页）",
                            "Selected node (outside this history page)",
                        ),
                    ));
                }
            });
            options
        })
    });
    let value = Signal::derive(move || {
        model.with(|model| {
            model
                .node
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_default()
        })
    });
    let disabled = Signal::derive(move || {
        target.get().is_none() || model.with(|model| model.loading || model.node.is_none())
    });
    view! {
        <section class="zxs-entry-section zxs-content-matches zxs-stack" aria-busy=move ||model.with(|model|model.loading).to_string()>
            <h3 class="zxs-row">{t("相同内容候选","Matching content candidates")}</h3>
            <p class="zxs-row">{t("仅查找本历史域中其他文件的非空内容版本。大小和摘要相同不表示已经关联，也不证明当前正文可读；不会合并文件、分支或历史。","Find nonempty versions of other files in this history. Equal size and hash do not mean files are linked or their bodies are currently readable. This does not merge files, branches or history.")}</p>
            <div class="zxs-file-toolbar zxs-flow">
                <Select label=t("比较节点","Compare node") options=options value=value disabled=Signal::derive(move ||target.get().is_none()) on_change=Callback::new(move |node:String|{if state.with_untracked(|state|state.files.file_history.node(&node).is_some()){model.update(|model|model.select_node(HistoryNodeId::parse(&node).expect("invariant: option is an admitted node")));}})/>
                <button class="zxs-button" disabled=move ||disabled.get() on:click=move |_|load(state,model,false)>{t("查找相同内容","Find matching content")}</button>
                <button class="zxs-button" disabled=move ||disabled.get()||model.with(|model|model.page.as_ref().is_none_or(|page|page.next_after_version_id.is_none())) on:click=move |_|load(state,model,true)>{t("下一页匹配","Next match page")}</button>
                <button class="zxs-button" disabled=move ||model.with(|model|!model.loading&&model.page.is_none()&&model.error.is_none()) on:click=move |_|model.update(Model::close)>{t("收起结果","Dismiss results")}</button>
            </div>
            <Show when=move ||model.with(|model|model.error.is_some())><p class="zxs-row zxs-inline-error" role="status">{move ||model.with(|model|model.error.as_ref().map(ToString::to_string).unwrap_or_default())}</p></Show>
            {move ||model.with(|model|model.page.clone()).map(|page|{
                let empty=page.matches.is_empty();let zero=page.logical_size==0;
                view!{<div class="zxs-stack" data-zxs-content-match-source=page.source_node_id.to_string()>
                    <p class="zxs-row">{format!("{} · {}",page.source_version_id,content_state(page.source_content_state))}</p>
                    {empty.then(||view!{<p class="zxs-row" role="status">{if zero{t("空文件不参与内容匹配。","Empty files do not participate in content matching.")}else{t("本页没有其他文件的匹配版本。","No matching versions of other files on this page.")}}</p>})}
                    <For each=move ||page.matches.clone() key=|found|found.version_id.clone() children=move |found|view!{
                        <article class="zxs-frame zxs-stack" data-zxs-content-match-file=found.file_id.to_string()>
                            <p class="zxs-row" role="heading" {web_interface::attr::custom::custom_attribute("aria-level", "4")}>{found.creation_name}</p>
                            <p class="zxs-row">{t("显示版本创建时的名称，不代表当前路径。","The name is from version creation, not its current path.")}</p>
                            <p class="zxs-row">{found.file_id.to_string()}</p><p class="zxs-row">{found.version_id.to_string()}</p>
                            <p class="zxs-row">{content_state(found.content_state)}</p>
                        </article>
                    }/>
                </div>}
            })}
        </section>
    }
}
#[cfg(test)]
#[path = "../../../../test/web/unit/files/content_matches_tests.rs"]
mod tests;
