use super::history::DecodeError;
use crate::{
    api,
    ui::{State, accept_read_response},
};
use storage_contract::history_window::HistoryFileId;
use storage_contract::shared_history::{SharedHistoryGraph, SharedHistoryRequest};
mod projection;
use projection::project;
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
    selection: Selection,
    generation: u64,
    loading: bool,
    cancellation: Option<api::ReadCancellation>,
    graph: Option<SharedHistoryGraph>,
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
        self.selection = Selection::All;
        self.graph = None;
        self.error = None;
    }
    fn current(&self, target: &Target, generation: u64) -> bool {
        self.target.as_ref() == Some(target) && self.generation == generation
    }
    fn select(&mut self, value: &str) {
        if value == "all" {
            self.selection = Selection::All;
            return;
        }
        if let Some(file) = self.graph.as_ref().and_then(|graph| {
            graph
                .files
                .iter()
                .find(|file| file.file_id.as_str() == value)
        }) {
            self.selection = Selection::File(file.file_id.clone());
        }
    }
    fn request(&self) -> Option<SharedHistoryRequest> {
        let target = self.target.as_ref()?;
        Some(
            SharedHistoryRequest::parse(&target.file, &target.root)
                .expect("invariant: admitted file/root forms a shared-history request"),
        )
    }
    fn receive(&mut self, graph: SharedHistoryGraph) {
        if let Selection::File(file) = &self.selection
            && !graph.files.iter().any(|member| &member.file_id == file)
        {
            self.selection = Selection::All;
        }
        self.graph = Some(graph);
    }
    fn close(&mut self) {
        self.cancel();
        self.graph = None;
        self.error = None;
    }
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
enum Selection {
    #[default]
    All,
    File(HistoryFileId),
}
fn endpoint(request: &SharedHistoryRequest) -> String {
    format!(
        "{}?history_root={}",
        request.route(),
        api::segment(&request.history_root.to_string())
    )
}
fn decode(
    value: json_codec::Value,
    request: &SharedHistoryRequest,
) -> Result<SharedHistoryGraph, api::Error> {
    let page: SharedHistoryGraph = json_codec::from_value(value).map_err(DecodeError::Wire)?;
    page.validate(request).map_err(DecodeError::Name)?;
    Ok(page)
}
fn load(state: RwSignal<State>, model: RwSignal<Model>) {
    let Some((target, request)) = state.with_untracked(|state| {
        let target = Target::capture(state)?;
        model.with_untracked(|model| {
            (model.target.as_ref() == Some(&target))
                .then(|| model.request().map(|request| (target, request)))
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
        .expect("invariant: mounted shared-history panel owns its state");
    spawn_local(async move {
        let result = api::cancellable_get(&endpoint(&request), cancellation)
            .await
            .and_then(|value| decode(value, &request));
        if !accept_read_response(state, target.epoch, &result)
            || !state
                .try_with_untracked(|state| Target::capture(state).as_ref() == Some(&target))
                .unwrap_or(false)
            || !model
                .try_with_untracked(|model| model.current(&target, generation))
                .unwrap_or(false)
        {
            return;
        }
        model.update(|model| {
            model.loading = false;
            model.cancellation = None;
            match result {
                Ok(graph) => model.receive(graph),
                Err(error) => {
                    model.graph = None;
                    model.error = Some(error);
                }
            }
        });
    });
}
pub(super) fn view(state: RwSignal<State>) -> impl IntoView {
    let model = RwSignal::new(Model::default());
    let target = Memo::new(move |_| state.with(Target::capture));
    Effect::new(move |_| {
        let target = target.get();
        model.update(|model| model.set_target(target));
    });
    on_cleanup(move || {
        let _ = model.try_update(Model::cancel);
    });
    let projection = Memo::new(move |_| {
        model.with(|model| {
            model
                .graph
                .as_ref()
                .map(|graph| project(graph, &model.selection))
                .unwrap_or_default()
        })
    });
    let records = Memo::new(move |_| projection.with(|projection| projection.records.clone()));
    let options = Signal::derive(move || {
        model.with(|model| {
            let mut options = vec![SelectOption::new("all", t("联合视图", "Combined view"))];
            if let Some(graph) = &model.graph {
                options.extend(graph.files.iter().map(|file| {
                    SelectOption::new(
                        file.file_id.to_string(),
                        format!(
                            "{}{} · {}",
                            if file.file_id == graph.origin_file_id {
                                t("本文件：", "This file: ")
                            } else {
                                String::new()
                            },
                            file.creation_name,
                            file.file_id
                        ),
                    )
                }));
            }
            options
        })
    });
    let value = Signal::derive(move || {
        model.with(|model| match &model.selection {
            Selection::All => "all".into(),
            Selection::File(file) => file.to_string(),
        })
    });
    view! {
        <section class="zxs-entry-section zxs-shared-history zxs-stack" aria-busy=move ||model.with(|model|model.loading).to_string()>
            <h3 class="zxs-row">{t("关联文件联合树", "Related file history graph")}</h3>
            <p class="zxs-row">{t("按保留的非空内容自动连接同历史域的文件，保留各文件、分支和事件身份。箭头表示原文件祖先；无箭头连线表示同内容。此面板只读，不改变默认分支或编辑目标。", "Automatically connect files in the same history through retained nonempty content, preserving file, branch and event identities. Arrows show original ancestry; lines without arrows show equal content. This read-only panel changes neither defaults nor editing targets.")}</p>
            <p class="zxs-row">{t("名称来自版本创建记录，不是当前路径。正文不可达或软删除不会抹去保留关系；本片不包含跨历史域、永久解除关联或完整磁盘退出语义。", "Names come from version creation, not current paths. Inaccessible bodies or soft deletion do not erase retained relationships. Cross-history links, permanent unlinking and complete disk-exit behavior are outside this slice.")}</p>
            <div class="zxs-file-toolbar zxs-flow">
                <button class="zxs-button" disabled=move ||target.get().is_none()||model.with(|model|model.loading) on:click=move |_|load(state,model)>{t("查看／刷新联合树", "View / refresh related graph")}</button>
                <Select label=t("查看文件树", "View file history") options=options value=value disabled=Signal::derive(move ||model.with(|model|model.graph.is_none())) on_change=Callback::new(move |file:String|model.update(|model|model.select(&file)))/>
                <button class="zxs-button" disabled=move ||model.with(|model|!model.loading&&model.graph.is_none()&&model.error.is_none()) on:click=move |_|model.update(Model::close)>{t("收起联合树", "Dismiss related graph")}</button>
            </div>
            <Show when=move ||model.with(|model|model.error.is_some())><p class="zxs-row zxs-inline-error" role="status">{move ||model.with(|model|model.error.as_ref().map(ToString::to_string).unwrap_or_default())}</p></Show>
            <Show when=move ||model.with(|model|model.graph.as_ref().is_some_and(|graph|graph.files.len()==1))><p class="zxs-row" role="status">{t("此历史域没有通过非空内容关联的其他文件。", "No other files in this history are connected through nonempty content.")}</p></Show>
            {super::version_tree::shared_ancestry_graph(records,move |id|{
                let id=StoredValue::new(id);
                let card=Memo::new(move |_|projection.with(|projection|id.with_value(|id|projection.cards.get(id).cloned())));
                view!{<article class="zxs-version-node zxs-shared-history-node" data-version-id=id.get_value() data-shared-history-kind=move ||card.with(|card|if card.as_ref().is_some_and(|card|card.shared){"content-relation"}else{"file-event"})>
                    <details class="zxs-node-disclosure">
                        <summary class="zxs-row">{move ||card.with(|card|card.as_ref().map(|card|card.title.clone()).unwrap_or_default())}</summary>
                        <div class="zxs-content-panel zxs-node-detail zxs-frame zxs-stack zxs-stack--flush">
                            <For each=move ||card.with(|card|card.as_ref().map(|card|card.lines.iter().enumerate().map(|(index,line)|(index,line.clone())).collect::<Vec<_>>()).unwrap_or_default()) key=|line|line.0 children=move |(_,line)|view!{<p class="zxs-row">{line}</p>}/>
                        </div>
                    </details>
                </article>}
            })}
        </section>
    }
}
#[cfg(test)]
#[path = "../../../../test/web/unit/files/shared_history_tests.rs"]
mod tests;
