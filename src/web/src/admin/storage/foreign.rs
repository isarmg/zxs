use super::*;

mod independent;
mod plan;
use std::collections::{BTreeMap, BTreeSet};
use storage_contract::{
    storage_directories::{DirectorySource, DiscoveredDiskState},
    storage_foreign::*,
};

#[derive(Clone, Default)]
pub(super) struct Model {
    generation: u64,
    reading: bool,
    cancellation: Option<api::ReadCancellation>,
    preview: Option<ForeignDiskPreview>,
    error: Option<api::Error>,
    plan: plan::Model,
    independent: independent::Model,
}
impl Model {
    pub(super) fn blocks_edit(&self) -> bool {
        self.plan.blocks_edit()
    }
    pub(super) fn invalidate(&mut self) {
        self.independent.invalidate();
        if let Some(cancellation) = self.cancellation.take() {
            cancellation.abort();
        }
        self.plan.leave();
        self.plan.invalidate();
        self.generation += 1;
        self.reading = false;
        self.preview = None;
        self.error = None;
    }
}
fn source(model: &super::Model) -> Option<&DirectorySource> {
    candidate_source(&model.wizard.chosen()?.state)
}
fn candidate_source(state: &DiscoveredDiskState) -> Option<&DirectorySource> {
    match state {
        DiscoveredDiskState::Zxs { source, .. } => Some(source),
        DiscoveredDiskState::Unsupported { source, .. } => source.as_ref(),
        DiscoveredDiskState::Empty { .. } | DiscoveredDiskState::Unavailable { .. } => None,
    }
}
fn inspectable(state: &State) -> bool {
    state.session_active()
        && state.admin_console.storage.editable()
        && !state.admin_console.storage.foreign.reading
        && source(&state.admin_console.storage).is_some()
}
fn hash(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
fn identity_valid(identity: &ForeignDiskIdentity) -> bool {
    storage_contract::validate_id(&identity.pool_id, "pool").is_ok()
        && storage_contract::validate_id(&identity.disk_id, "disk").is_ok()
        && name_policy::validate_slot(&identity.slot_id).is_ok()
        && identity.binding_generation > 0
}
fn read(value: Value, source: &DirectorySource) -> Result<ForeignDiskPreview, api::Error> {
    let preview: ForeignDiskPreview = decode(value)?;
    if preview.source != *source
        || !identity_valid(&preview.identity)
        || preview.source_level == 0
        || !hash(&preview.seal_digest)
        || !hash(&preview.review_digest)
        || !hash(&preview.checkpoint.candidate_hash)
        || !hash(&preview.checkpoint.publication_hash)
        || preview
            .checkpoint
            .authority_hash
            .as_ref()
            .is_some_and(|value| !hash(value))
        || preview.histories.len() > 128
    {
        return Err(invalid(ResponseError::Identity));
    }
    let mut groups = BTreeSet::new();
    let mut versions = 0usize;
    for history in &preview.histories {
        let unavailable: BTreeSet<_> = history.temporarily_unavailable_versions.iter().collect();
        let accounted = history
            .verified_local_versions
            .checked_add(unavailable.len())
            .and_then(|count| count.checked_add(history.recorded_purged_versions))
            .and_then(|count| count.checked_add(history.recorded_migrated_versions));
        if !hash(&history.group_id)
            || !groups.insert(&history.group_id)
            || storage_contract::validate_id(&history.history_tree_id, "history").is_err()
            || !identity_valid(&history.origin)
            || !hash(&history.manifest_hash)
            || history.versions > 10_000
            || accounted != Some(history.versions)
            || unavailable.len() != history.temporarily_unavailable_versions.len()
            || unavailable
                .iter()
                .any(|id| storage_contract::validate_id(id, "ver").is_err())
        {
            return Err(invalid(ResponseError::Identity));
        }
        versions += history.versions;
        if versions > 10_000 {
            return Err(invalid(ResponseError::Identity));
        }
    }
    Ok(preview)
}
fn current(model: &super::Model, source_expected: &DirectorySource, generation: u64) -> bool {
    model.foreign.generation == generation && source(model) == Some(source_expected)
}
fn inspect(state: RwSignal<State>) {
    let captured = state.with_untracked(|state| {
        inspectable(state).then(|| {
            (
                state.session_epoch,
                source(&state.admin_console.storage)
                    .expect("invariant: inspection requires an observed source")
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
            state.update(|state| state.admin_console.storage.foreign.error = Some(error));
            return;
        }
    };
    let generation = state
        .try_update(|state| {
            let model = &mut state.admin_console.storage.foreign;
            model.invalidate();
            model.reading = true;
            model.cancellation = Some(cancellation.clone());
            model.generation
        })
        .expect("invariant: active inspection owns the application state");
    spawn_local(async move {
        let body = json!(ForeignDiskPreviewRequest {
            source: source.clone()
        });
        let result = api::cancellable_request(
            "POST",
            "/api/admin/storage/foreign/preview",
            Some(body),
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
            let model = &mut state.admin_console.storage.foreign;
            model.reading = false;
            model.cancellation = None;
            match result {
                Ok(preview) => {
                    model.plan.set_source(&preview);
                    model.preview = Some(preview);
                }
                Err(error) => model.error = Some(error),
            }
        });
    });
}
fn origin_label(identity: &ForeignDiskIdentity) -> String {
    format!(
        "{} · {} · {} · {} {}",
        identity.slot_id,
        identity.pool_id,
        identity.disk_id,
        t("绑定代数", "binding generation"),
        identity.binding_generation
    )
}
fn history_view(history: ForeignHistorySummary) -> impl IntoView {
    view! {<section class="zxs-stack" data-foreign-history=history.group_id>
        <h3 class="zxs-row">{history.repository_name}</h3>
        <p class="zxs-row">{history.history_tree_id}</p>
        <p class="zxs-row">{format!("{}: {} · {}: {} · {}: {} · {}: {}",t("文件分支","File branches"),history.branches,t("目录分支","Directory branches"),history.directory_branches,t("目录树","Directory trees"),history.directory_trees,t("提交记录","Commit records"),history.commit_records)}</p>
        <p class="zxs-row">{format!("{}: {} · {}: {} · {}: {} · {}: {} · {}: {}",t("版本总数","Total versions"),history.versions,t("本地正文已核验","Verified local contents"),history.verified_local_versions,t("正文暂不可用","Contents temporarily unavailable"),history.temporarily_unavailable_versions.len(),t("已记录清除","Recorded purged"),history.recorded_purged_versions,t("已记录迁移","Recorded migrated"),history.recorded_migrated_versions)}</p>
        <Show when=move ||!history.temporarily_unavailable_versions.is_empty()><p class="zxs-row">{t("暂不可用的正文仍保留历史记录，不会被当作无效分支裁剪。","Unavailable contents retain their history and are never pruned as invalid branches.")}</p></Show>
    </section>}
}
fn review(preview: ForeignDiskPreview) -> impl IntoView {
    let mut groups: BTreeMap<(String, String, String, u64), Vec<ForeignHistorySummary>> =
        BTreeMap::new();
    for history in preview.histories {
        let origin = &history.origin;
        groups
            .entry((
                origin.pool_id.clone(),
                origin.disk_id.clone(),
                origin.slot_id.clone(),
                origin.binding_generation,
            ))
            .or_default()
            .push(history);
    }
    view! {<div class="zxs-stack" data-foreign-preview="">
        <p class="zxs-row">{format!("{}: {} · {}",t("来源层级","Source tier"),preview.source_level,origin_label(&preview.identity))}</p>
        <p class="zxs-row">{match preview.role_evidence {
            storage_contract::storage_directories::DiskRoleEvidence::InitialDeclaration => t("角色证据来自与本盘身份匹配的原始声明，不代表完整作者授权，也不授予本地写权限。","Role evidence is an original declaration matching this disk's identity. It is not complete author authorization and grants no local write permission."),
            storage_contract::storage_directories::DiskRoleEvidence::GenerationEvidence => t("角色证据来自已核验成员代际，不代表完整作者授权，也不授予本地写权限。","Role evidence is a verified membership generation. It is not complete author authorization and grants no local write permission."),
        }}</p>
        <p class="zxs-row">{format!("{}: {} · {}: {}",t("已发布检查点序列","Published checkpoint sequence"),preview.checkpoint.sequence,t("核对摘要","Review digest"),preview.review_digest)}</p>
        <p class="zxs-row">{t("检查点只代表本盘最后完整发布的状态，不保证是原系统全局最新状态。","The checkpoint is this disk's last complete publication, not necessarily the original system's latest global state.")}</p>
        <Show when=move ||preview.checkpoint.unpublished_tail><p class="zxs-row" role="status">{t("存在未发布尾部记录，本次只展示已证明发布的历史。","An unpublished journal tail exists. This review shows only proven published history.")}</p></Show>
        {groups.into_values().map(|histories|{
            let origin=origin_label(&histories[0].origin);
            view!{<section class="zxs-frame zxs-stack" data-foreign-origin=""><h3 class="zxs-row">{format!("{}: {origin}",t("可证明的原一级来源","Proven original primary"))}</h3>{histories.into_iter().map(history_view).collect_view()}</section>}
        }).collect_view()}
        <p class="zxs-row" role="status">{match preview.execution {ForeignExecutionCapability::Unavailable=>t("这是只读历史预览。外来磁盘并入、降级与目录重建尚不可执行；本次没有修改来源或加入磁盘。","This is a read-only history preview. Foreign-disk admission, tier conversion and directory reconstruction are not executable yet. No source was changed or disk admitted.")}}</p>
    </div>}
}
pub(super) fn view(state: RwSignal<State>) -> impl IntoView {
    on_cleanup(move || {
        let _ = state.try_update(|state| state.admin_console.storage.foreign.invalidate());
    });
    view! {<Show when=move ||state.with(|state|source(&state.admin_console.storage).is_some())>
        <section class="zxs-stack" aria-label=t("外来 ZXS 历史检查","Foreign ZXS history inspection")>
            <p class="zxs-row">{t("可明确启动只读 ZXS 历史检查，最多核验 128 组历史、10,000 个版本及 8 GiB 本地正文；不改写、迁入或删除源盘。扫描可能需要一些时间，超出预算会报错而非截断结果。","Start a read-only ZXS history inspection of up to 128 histories, 10,000 versions and 8 GiB of local contents. It never writes, imports or deletes the source. Scanning may take time; exceeding a budget fails instead of returning a partial review.")}</p>
            <button type="button" class="zxs-button" data-foreign-preview-start="" disabled=move ||state.with(|state|!inspectable(state)) on:click=move |_|inspect(state)>{t("只读检查 ZXS 历史","Inspect ZXS history read-only")}</button>
            <Show when=move ||state.with(|state|state.admin_console.storage.foreign.reading)><p class="zxs-row" role="status">{t("正在只读核验历史与本地正文…","Verifying history and local contents read-only…")}</p></Show>
            {move ||state.with(|state|state.admin_console.storage.foreign.error.clone()).map(|error|{let feedback=crate::ui::feedback(&error);view!{<ErrorState message=feedback.message request_id=feedback.request_id/>}})}
            {move ||state.with(|state|state.admin_console.storage.foreign.preview.clone()).map(review)}
        </section>
    </Show>
    {independent::view(state)}
    }
}

pub(super) fn planning(state: RwSignal<State>) -> impl IntoView {
    plan::view(state)
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/admin/storage_foreign_tests.rs"]
mod tests;
