//! Explicit repository-wide repair, with no automatic execution or job polling.
use super::{Model, Target, actions};
use crate::{
    api,
    ui::{State, accept_read_response, current_session},
};
use storage_contract::directory_git::DirectoryLayoutResult;
use web_interface::{components::ErrorState, i18n::t, prelude::*, tasks::spawn_local};

#[derive(Clone, Default)]
enum Phase {
    #[default]
    Idle,
    Review(Target),
    Running(Target),
    Finished(Target, Result<DirectoryLayoutResult, api::Error>),
}
impl Phase {
    fn target(&self) -> Option<&Target> {
        match self {
            Self::Idle => None,
            Self::Review(target) | Self::Running(target) | Self::Finished(target, _) => {
                Some(target)
            }
        }
    }
}
fn review(state: &State, model: &Model, phase: &Phase) -> Option<Target> {
    let (target, status) = model.ready(state)?;
    if !(status.initialized || status.owner.is_some())
        || matches!(phase, Phase::Running(_))
        || matches!(phase, Phase::Finished(failed, Err(_)) if failed == target)
    {
        return None;
    }
    Some(target.clone())
}
fn begin(state: &mut State, model: &Model, phase: &mut Phase) -> Option<Target> {
    let target = review(state, model, phase)?;
    if !matches!(phase, Phase::Review(confirmed) if confirmed == &target) {
        return None;
    }
    state.files.mutation_pending = true;
    *phase = Phase::Running(target.clone());
    Some(target)
}
/// Files navigation preserves this exclusive pending gate. Session replacement
/// replaces the entire Files model; an older epoch must never release its gate.
fn release(state: &mut State, target: &Target) -> bool {
    if !state.session_active() || state.session_epoch != target.epoch {
        return false;
    }
    state.files.mutation_pending = false;
    Target::capture(state).as_ref() == Some(target)
}
fn decode(value: json_codec::Value, target: &Target) -> Result<DirectoryLayoutResult, api::Error> {
    let result: DirectoryLayoutResult = json_codec::from_value(value).map_err(|source| {
        api::Error::invalid_response(
            t(
                "布局结果格式无效；不能据此判断操作未执行。",
                "The layout result is invalid; this does not mean the repair was not executed.",
            ),
            api::ErrorSource::DirectoryLayout(source),
        )
    })?;
    if result.repository_id != target.repository {
        return Err(api::error(
            storage_contract::ErrorCode::InvalidResponse,
            t(
                "布局结果不属于本次仓库；操作结果尚未确认。",
                "The layout result belongs to another repository; this repair is unconfirmed.",
            ),
        ));
    }
    Ok(result)
}
fn submit(state: RwSignal<State>, model: RwSignal<Model>, phase: RwSignal<Phase>) {
    let target = state
        .try_update(|state| {
            model.with_untracked(|model| {
                phase
                    .try_update(|phase| begin(state, model, phase))
                    .flatten()
            })
        })
        .flatten();
    let Some(target) = target else {
        return;
    };
    spawn_local(async move {
        let endpoint = format!(
            "/api/repositories/{}/directory-layout/reconcile",
            api::segment(&target.repository)
        );
        let body = json_codec::json!({"expected_namespace_head":target.namespace_head});
        let result = api::request("POST", &endpoint, Some(body))
            .await
            .and_then(|value| decode(value, &target));
        finish(state, phase, target, result);
    });
}
fn finish(
    state: RwSignal<State>,
    phase: RwSignal<Phase>,
    target: Target,
    result: Result<DirectoryLayoutResult, api::Error>,
) {
    if !current_session(state, target.epoch) {
        return;
    }
    let refresh_storage = needs_storage_refresh(&result);
    let same_page = state.try_update(|state| {
        if refresh_storage {
            state.management.invalidate_storage();
        }
        release(state, &target)
    }) == Some(true);
    if refresh_storage {
        crate::management::load(state);
    }
    // Even an obsolete page's 401 revokes its current session, not a newer one.
    let accepted = accept_read_response(state, target.epoch, &result);
    let _ = phase.try_update(|phase| complete(phase, target, result, accepted && same_page));
}
fn needs_storage_refresh(result: &Result<DirectoryLayoutResult, api::Error>) -> bool {
    matches!(result, Err(error) if error.mutation_outcome_unknown()
        || error.code == storage_contract::ErrorCode::RecoveryRequired)
}
fn complete(
    phase: &mut Phase,
    target: Target,
    result: Result<DirectoryLayoutResult, api::Error>,
    same_page: bool,
) {
    if matches!(phase, Phase::Running(running) if running == &target) {
        *phase = if same_page {
            Phase::Finished(target, result)
        } else {
            Phase::Idle
        };
    }
}
fn summary(result: &DirectoryLayoutResult) -> String {
    format!(
        "{}: {} · {}: {} · {}: {} · {}: {} · {}: {}",
        t("状态序号", "State sequence"),
        result.state_sequence,
        t("目录", "Directories"),
        result.directories,
        t("容器", "Containers"),
        result.containers,
        t("不可用容器", "Unavailable containers"),
        result.unavailable_containers,
        t("不可用历史", "Unavailable histories"),
        result.unavailable_histories
    )
}
fn outcome(result: &DirectoryLayoutResult) -> String {
    if result.unavailable_containers != 0 || result.unavailable_histories != 0 {
        t(
            "可用部分的布局已整理；仍有不可用副本，不能视为全部完成；成员重新准入后请重新检查。",
            "Available placements were reconciled. Some replicas remain unavailable; this is not full completion. Review again after those members are readmitted.",
        )
    } else {
        t(
            "当前可用布局已整理；这些数量不证明正文完整，也不代表新目录提交。",
            "Current available placements were reconciled. These counts do not verify file contents or represent a new directory commit.",
        )
    }
}
pub(super) fn view(state: RwSignal<State>, model: RwSignal<Model>) -> impl IntoView {
    let phase = RwSignal::new(Phase::Idle);
    let target = Memo::new(move |_| state.with(Target::capture));
    Effect::new(move |_| {
        let target = target.get();
        phase.update(|phase| {
            if !matches!(phase, Phase::Running(_)) && phase.target() != target.as_ref() {
                *phase = Phase::Idle;
            }
        });
    });
    view! {<section class="zxs-stack zxs-frame" aria-label=t("物理目录布局", "Physical directory layout")>
        <h3 class="zxs-row">{t("按稳定 main 整理实体布局", "Reconcile physical layout from stable main")}</h3>
        <p class="zxs-row">{t("这是整个外层仓库的操作，不只当前文件夹。请从已显式初始化的目录或其子目录进入；不会替你初始化或提交目录。", "This affects the entire outer repository, not only this folder. Enter an explicitly initialized directory or its descendant to review; this action never initializes or commits one for you.")}</p>
        <button type="button" class="zxs-button" data-review-directory-layout="" disabled=move ||state.with(|state|model.with(|model|phase.with(|phase|review(state,model,phase).is_none())))
            on:click=move |_|{let reviewed=state.with_untracked(|state|model.with_untracked(|model|phase.with_untracked(|phase|review(state,model,phase))));if let Some(target)=reviewed {phase.set(Phase::Review(target));}}>{t("检查整理范围", "Review reconciliation scope")}</button>
        {move ||match phase.get() {
            Phase::Review(reviewed) => {
                let repo=reviewed.repository.clone();
                view! {<div class="zxs-stack" data-directory-layout-review="">
                    <p class="zxs-row">{format!("{}: {}", t("整个外层仓库", "Entire outer repository"), repo)}</p>
                    <p class="zxs-row">{t("将按执行时每个独立目录 main 最新已保存状态整理实体位置，包括未提交内容，不是当前页的冻结预览。子仓库各用自己的 main；正文与历史不被复制或删除，后续保存与冷恢复继续维护已启用布局。", "Reconciliation uses each independent directory main’s latest saved state at execution, including uncommitted work, not a frozen preview of this page. Each child uses its own main. Contents and history are not copied or deleted; later saves and cold recovery maintain the activated layout.")}</p>
                    <p class="zxs-row">{t("关闭页面或网络断开不会撤销已开始的整理。结果未知时先刷新存储状态与目录，再明确重新检查；不自动重试。", "Closing the page or losing the connection does not cancel an admitted repair. If its outcome is unknown, refresh storage status and the folder before explicitly reviewing again. No automatic retries.")}</p>
                    <div class="zxs-flow"><button type="button" class="zxs-button" data-confirm-directory-layout="" disabled=move ||state.with(|state|model.with(|model|phase.with(|phase|review(state,model,phase).is_none()))) on:click=move |_|submit(state,model,phase)>{t("整理整个仓库布局", "Reconcile the entire repository")}</button>
                    <button type="button" class="zxs-button" data-cancel-directory-layout="" on:click=move |_|phase.update(|phase|{if matches!(phase,Phase::Review(_)){*phase=Phase::Idle;}})>{t("取消", "Cancel")}</button></div>
                </div>}.into_any()
            }
            Phase::Running(running) if Some(&running)==target.get().as_ref()=>view!{<p class="zxs-row" role="status">{t("实体布局整理中；离开页面不会取消。", "Reconciling physical layout. Leaving this page does not cancel it.")}</p>}.into_any(),
            Phase::Finished(finished, Ok(result)) if Some(&finished)==target.get().as_ref()=>view!{<div class="zxs-stack" role="status" data-directory-layout-result=""><p class="zxs-row">{outcome(&result)}</p><p class="zxs-row">{summary(&result)}</p></div>}.into_any(),
            Phase::Finished(finished, Err(error)) if Some(&finished)==target.get().as_ref()=>{
                let feedback=crate::ui::feedback(&error);
                view!{<div class="zxs-stack">
                    <ErrorState message=feedback.message request_id=feedback.request_id/>
                    <p class="zxs-row" role="status">{t(
                        "未收到可确认的整理结果。操作可能已执行或仍在运行，不能假定磁盘未变化。先刷新存储状态及目录；恢复要求须先处理，再明确重新检查。",
                        "No confirmed layout result was received. The repair may have executed or still be running; do not assume the disks are unchanged. Refresh storage status and the folder, resolve any recovery requirement, then explicitly review again."
                    )}</p>
                    <button type="button" class="zxs-button" data-refresh-directory-layout=""
                        disabled=move ||state.with(|state|actions::pending(state)||state.files.loading||state.management.loading||state.management.retrying)
                        on:click=move |_|crate::management::refresh_storage_and_folder(state)>{t(
                            "检查存储状态并刷新目录", "Check storage status and refresh folder"
                        )}</button>
                </div>}.into_any()
            },
            _ => ().into_any(),
        }}
    </section>}
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/directory_layout_tests.rs"]
mod tests;
