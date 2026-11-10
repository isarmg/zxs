use super::*;
use storage_contract::{
    storage_independent_source::{IndependentIdentity, IndependentRole},
    storage_independent_topology::{IndependentDiskPreview, IndependentDiskPreviewRequest},
};

#[derive(Clone, Default)]
pub(super) struct Model {
    generation: u64,
    reading: bool,
    cancellation: Option<api::ReadCancellation>,
    preview: Option<IndependentDiskPreview>,
    error: Option<api::Error>,
}
impl Model {
    pub(super) fn invalidate(&mut self) {
        if let Some(cancellation) = self.cancellation.take() {
            cancellation.abort();
        }
        self.generation += 1;
        self.reading = false;
        self.preview = None;
        self.error = None;
    }
}

fn source(model: &super::super::Model) -> Option<&DirectorySource> {
    match &model.wizard.chosen()?.state {
        DiscoveredDiskState::Unsupported { source, .. } => source.as_ref(),
        _ => None,
    }
}
fn inspectable(state: &State) -> bool {
    state.session_active()
        && state.admin_console.storage.editable()
        && !state.admin_console.storage.foreign.independent.reading
        && source(&state.admin_console.storage).is_some()
}
fn current(model: &super::super::Model, expected: &DirectorySource, generation: u64) -> bool {
    model.foreign.independent.generation == generation && source(model) == Some(expected)
}
fn read(value: Value, expected: &DirectorySource) -> Result<IndependentDiskPreview, api::Error> {
    let preview: IndependentDiskPreview = decode(value)?;
    if preview.source != *expected {
        return Err(invalid(ResponseError::Identity));
    }
    Ok(preview)
}
fn inspect(state: RwSignal<State>) {
    let captured = state.with_untracked(|state| {
        inspectable(state).then(|| {
            (
                state.session_epoch,
                source(&state.admin_console.storage)
                    .expect("invariant: independent preview requires an observed source")
                    .clone(),
            )
        })
    });
    let Some((epoch, source)) = captured else {
        return;
    };
    let cancellation = match api::ReadCancellation::new() {
        Ok(cancellation) => cancellation,
        Err(error) => {
            state.update(|state| {
                state.admin_console.storage.foreign.independent.error = Some(error)
            });
            return;
        }
    };
    let generation = state
        .try_update(|state| {
            let foreign = &mut state.admin_console.storage.foreign;
            foreign.invalidate();
            let model = &mut foreign.independent;
            model.reading = true;
            model.cancellation = Some(cancellation.clone());
            model.generation
        })
        .expect("invariant: active independent inspection owns application state");
    spawn_local(async move {
        let result = api::cancellable_request(
            "POST",
            "/api/admin/storage/independent/preview",
            Some(json!(IndependentDiskPreviewRequest {
                source: source.clone()
            })),
            cancellation,
        )
        .await
        .and_then(|value| read(value, &source));
        if !accept_read_response(state, epoch, &result) {
            return;
        }
        let _ = state.try_update(|state| {
            if !state.session_active()
                || state.session_epoch != epoch
                || !current(&state.admin_console.storage, &source, generation)
            {
                return;
            }
            let model = &mut state.admin_console.storage.foreign.independent;
            model.reading = false;
            model.cancellation = None;
            match result {
                Ok(preview) => model.preview = Some(preview),
                Err(error) => model.error = Some(error),
            }
        });
    });
}
fn identity_label(identity: &IndependentIdentity) -> String {
    let origin = &identity.origin;
    format!(
        "{} · {} · {} · {} {}",
        origin.slot_id.as_str(),
        origin.pool_id.as_str(),
        origin.disk_id.as_str(),
        t("绑定代数", "Binding generation"),
        origin.binding_generation.get(),
    )
}
fn review(preview: IndependentDiskPreview) -> impl IntoView {
    let graph = preview.topology;
    let inventory = preview.inventory;
    let root = graph.root();
    let checkpoint = root.publication.as_ref().map_or_else(
        || t("尚无发布", "No publication yet"),
        |publication| publication.as_str().to_owned(),
    );
    view! {
        <div class="zxs-stack" data-independent-preview="">
            <p class="zxs-row">{identity_label(&root.identity)}</p>
            <p class="zxs-row">{format!("{} {} · {checkpoint}", t("本盘发布序列", "Local publication sequence"), root.sequence)}</p>
            <p class="zxs-row" role="status">{t("这是所选盘保存的来源图，不代表来源盘当前在线、候选完整或唯一，也不授予迁入、降级或写权限。本次不读取正文，不据此判断正文可用性。", "This is the source graph retained by the selected disk. It does not prove that sources are online, uniquely matched or completely discovered, and grants no import, tier-conversion or write permission. Contents are not read and their availability is not inferred.")}</p>
            <section class="zxs-frame zxs-stack" data-independent-history-inventory="">
                <h3 class="zxs-row">{format!("{}: {}", t("本盘保留的文件版本", "File versions retained on this disk"), inventory.versions().len())}</h3>
                <p class="zxs-row">{t("以下是本盘已发布元数据中保留的版本引用，使用原一级盘身份及稳定历史／文件标识，不是当前文件路径或正文健康报告。旧发布保留的来源可以不在当前来源图中。检查无需原系统、上游或下游磁盘在线。", "These version references come from this disk's published metadata and retain the original primary identity and stable history/file identifiers. They are not current paths or a content-health report. Retained earlier sources may be absent from the current source graph. Inspection does not need the original system, upstream or downstream disks.")}</p>
                {inventory.versions().iter().map(|version| view! {
                    <details class="zxs-stack" data-independent-retained-version=version.commit.as_str().to_owned()>
                        <summary>{format!("{} · {} · {}", identity_label(&version.owner), version.history_id.as_str(), version.file_id.as_str())}</summary>
                        <p class="zxs-row">{format!("incarnation: {}", version.owner.incarnation.as_str())}</p>
                        <p class="zxs-row">{format!("{}: {}", t("不可变版本", "Immutable version"), version.commit.as_str())}</p>
                        <p class="zxs-row">{format!("{}: {}", t("声明的正文摘要（未核验正文）", "Declared content digest (contents not verified)"), version.body.as_str())}</p>
                    </details>
                }).collect_view()}
            </section>
            {graph.nodes().iter().map(|node| {
                let tier = match node.identity.role {
                    IndependentRole::Primary => 1,
                    IndependentRole::Secondary => 2,
                    IndependentRole::Tertiary => 3,
                };
                let pins = node.pins.iter().map(|pin| pin.as_str()).collect::<Vec<_>>().join(" · ");
                view! {
                    <section class="zxs-frame zxs-stack" data-independent-source-node=node.identity.origin.disk_id.as_str().to_owned()>
                        <h3 class="zxs-row">{format!("{} {tier} · {}", t("来源层级", "Source tier"), identity_label(&node.identity))}</h3>
                        <p class="zxs-row">{format!("incarnation: {}", node.identity.incarnation.as_str())}</p>
                        <p class="zxs-row">{format!("{}: {pins}", t("准确发布引用", "Exact publication references"))}</p>
                    </section>
                }
            }).collect_view()}
            {graph.relations().iter().map(|relation| view! {
                <p class="zxs-row" data-independent-source-relation="">{format!(
                    "{} · {} → {} · {}",
                    identity_label(&relation.source.identity), relation.source.publication.as_str(),
                    identity_label(&relation.consumer.identity), relation.consumer.publication.as_str(),
                )}</p>
            }).collect_view()}
        </div>
    }
}
pub(super) fn view(state: RwSignal<State>) -> impl IntoView {
    view! {
        <Show when=move ||state.with(|state|source(&state.admin_console.storage).is_some())>
            <section class="zxs-stack" aria-label=t("独立格式来源检查", "Independent-format source inspection")>
                <p class="zxs-row">{t("若此目录使用独立磁盘格式，可单独只读核验其完整元数据来源图。未知或损坏格式会拒绝，不会转换、修复或初始化目录。", "For an independent-format disk, inspect its complete metadata source graph read-only. Unknown or corrupt formats are rejected without converting, repairing or initializing the directory.")}</p>
                <button type="button" class="zxs-button" data-independent-preview-start="" disabled=move ||state.with(|state|!inspectable(state)) on:click=move |_|inspect(state)>{t("只读检查独立盘来源", "Inspect independent disk sources read-only")}</button>
                <Show when=move ||state.with(|state|state.admin_console.storage.foreign.independent.reading)>
                    <p class="zxs-row" role="status">{t("正在只读核验独立盘元数据…", "Verifying independent disk metadata read-only…")}</p>
                </Show>
                {move ||state.with(|state|state.admin_console.storage.foreign.independent.error.clone()).map(|error| {
                    let feedback = crate::ui::feedback(&error);
                    view! {<ErrorState message=feedback.message request_id=feedback.request_id/>}
                })}
                {move ||state.with(|state|state.admin_console.storage.foreign.independent.preview.clone()).map(review)}
            </section>
        </Show>
    }
}

#[cfg(test)]
#[path = "../../../../../../test/web/unit/admin/storage_independent_tests.rs"]
mod tests;
