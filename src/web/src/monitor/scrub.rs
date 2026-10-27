//! Read-only operational visibility, independent of storage Ready and write receipts.
mod wire;
use crate::{
    api,
    ui::{State, accept_read_response, feedback},
};
use json_codec::Value;
use web_interface::{
    components::{ErrorState, Feedback, LoadingState},
    i18n::t,
    prelude::*,
    tasks::spawn_local,
};
use wire::{Baseline, Completion, Freshness, Outcome, Phase, Status, Trigger};

#[derive(Clone, Default)]
pub(super) struct Model {
    observation: Option<Status>,
    loading: bool,
    failure: Option<Feedback>,
}
pub(super) fn load(state: RwSignal<State>) {
    if state.with_untracked(|s| !s.session_active() || s.monitor.scrub.loading) {
        return;
    }
    state.update(|s| s.monitor.scrub.loading = true);
    let epoch = state.with_untracked(|s| s.session_epoch);
    spawn_local(async move {
        complete(
            state,
            epoch,
            api::request("GET", "/api/admin/scrub", None).await,
        );
    });
}
fn complete(state: RwSignal<State>, epoch: u64, result: Result<Value, api::Error>) {
    if !accept_read_response(state, epoch, &result) {
        return;
    }
    state.update(|s| {
        let model = &mut s.monitor.scrub;
        model.loading = false;
        match result {
            Ok(value) => match Status::decode(value) {
                Ok(status) => {
                    model.observation = Some(status);
                    model.failure = None;
                }
                Err(error) => {
                    model.observation = None;
                    model.failure = Some(Feedback {
                        message: format!(
                            "{}: {error}",
                            t("巡检状态格式无效", "Invalid integrity status")
                        ),
                        request_id: None,
                    });
                }
            },
            Err(error) => {
                // A failed refresh must never preserve a green old observation.
                model.observation = None;
                model.failure = Some(feedback(&error));
            }
        }
    });
}
fn freshness_key(status: Option<&Status>) -> &'static str {
    match status.map(|status| status.freshness) {
        Some(Freshness::Fresh) => "fresh",
        Some(Freshness::Expired) => "expired",
        Some(Freshness::NotChecked) => "not_checked",
        Some(Freshness::Unavailable) | None => "unavailable",
    }
}
fn freshness(status: Option<&Status>) -> String {
    match status.map(|status| status.freshness) {
        Some(Freshness::Fresh) => t("最近完整检查有效", "Recent complete check is current"),
        Some(Freshness::Expired) => t("完整检查已过期", "Complete check has expired"),
        Some(Freshness::NotChecked) => t("尚无完整成功检查", "No complete successful check yet"),
        Some(Freshness::Unavailable) | None => {
            t("完整性结论不可用", "Integrity conclusion unavailable")
        }
    }
}
fn baseline(sequence: u64, hash: Option<&str>) -> String {
    format!(
        "{} {sequence} · {}",
        t("基线序号", "Baseline sequence"),
        hash.unwrap_or("—")
    )
}
fn phase(status: &Status) -> String {
    match &status.phase {
        Phase::NotReady => t("存储尚未准入巡检", "Storage is not admitted for scanning"),
        Phase::Idle => t("当前无活动巡检", "No active scan"),
        Phase::Queued {
            job_id,
            cancellation_requested,
        } => format!(
            "{} #{job_id}{}",
            t("已排队", "Queued"),
            cancelling(*cancellation_requested)
        ),
        Phase::Running {
            job_id,
            baseline:
                Baseline {
                    sequence,
                    snapshot_hash,
                },
            started_at,
            bytes_read,
            expected_payload_bytes,
            cancellation_requested,
        } => format!(
            "{} #{job_id}{} · {} · {} {started_at} · {} {bytes_read} B · {} {expected_payload_bytes} B",
            t("巡检中", "Scanning"),
            cancelling(*cancellation_requested),
            baseline(*sequence, snapshot_hash.as_deref()),
            t("开始", "Started"),
            t("实际读取", "Actual bytes read"),
            t("预期正文", "Expected payload"),
        ),
        Phase::Stopping => t(
            "正在取消并等待真实 I/O 结束",
            "Stopping; waiting for actual I/O to finish",
        ),
        Phase::SchedulerUnavailable { error } => failure(error),
    }
}
fn cancelling(requested: bool) -> String {
    if requested {
        t(
            "（已请求取消，仍在排空）",
            " (cancellation requested; still draining)",
        )
    } else {
        String::new()
    }
}
fn failure(error: &wire::Failure) -> String {
    format!("{} · {} · {}", error.code, error.message, error.request_id)
}
fn latest(status: &Status) -> String {
    let Some(run) = &status.latest else {
        return t("尚无巡检结果", "No scan result yet");
    };
    let trigger = match run.trigger {
        Trigger::Manual => t("手动", "Manual"),
        Trigger::Periodic => t("周期", "Periodic"),
    };
    let outcome = match &run.outcome {
        Outcome::Rejected { error } => format!(
            "{}: {}",
            t("准入拒绝", "Admission rejected"),
            failure(error)
        ),
        Outcome::Report { report } => {
            let completion = match &report.completion {
                Completion::CompleteSuccess => t("完整成功", "Complete success"),
                Completion::PartialFailure => t("部分故障", "Partial failure"),
                Completion::Cancelled => t("已取消，非完整成功", "Cancelled, not complete success"),
                Completion::Expired => t(
                    "基线已过期，非完整成功",
                    "Baseline expired, not complete success",
                ),
                Completion::Running => t("尚未完成", "Not completed"),
                Completion::Failed { failure: error } => failure(error),
            };
            format!(
                "{completion} · {} · {} — {} · {} {}/{} · {} {} · {} {} · {} {} · {} {}",
                baseline(report.sequence, report.snapshot_hash.as_deref()),
                report.started_at,
                report.finished_at,
                t("已验证副本", "Verified copies"),
                report.payload_copies_verified,
                report.payload_copies_required,
                t("身份故障", "Identity failures"),
                report.identity_failures,
                t("元数据故障", "Metadata failures"),
                report.metadata_failures,
                t("不可用副本", "Unavailable copies"),
                report.payload_copies_unavailable,
                t("未验证副本", "Unverified copies"),
                report.payload_copies_unverified,
            )
        }
    };
    format!("{trigger} #{} · {outcome}", run.job_id)
}
pub(super) fn view(state: RwSignal<State>) -> impl IntoView {
    view! {
        <article class="zxs-content-panel zxs-frame zxs-frame--inset" data-integrity-scrub="" aria-label=t("完整性巡检", "Full integrity scan")>
            <h2>{t("完整性巡检", "Full integrity scan")}</h2>
            <p>{t("以下为最后查询的巡检状态；存储就绪和写入成功不等于全池完整性检查。", "This is the last queried scan status; storage readiness and successful writes are not full-pool integrity checks.")}</p>
            {move || state.with(|s| s.monitor.scrub.failure.clone()).map(|error| view! { <ErrorState message=error.message request_id=error.request_id/> })}
            <Show when=move || state.with(|s| s.monitor.scrub.loading && s.monitor.scrub.observation.is_none())><LoadingState/></Show>
            <p role="status" data-integrity-freshness=move || state.with(|s| freshness_key(s.monitor.scrub.observation.as_ref()))>{move || state.with(|s| freshness(s.monitor.scrub.observation.as_ref()))}</p>
            {move || state.with(|s| s.monitor.scrub.observation.as_ref().map(|status| view! {
                <p>{phase(status)}</p>
                <p>{format!("{}: {}", t("最后完整成功", "Last complete success"), status.last_complete_success_at.as_deref().unwrap_or("—"))}</p>
                <p>{latest(status)}</p>
            }))}
            <p>{t("实际读取含元数据与验证开销，预期正文不是总 I/O，不能据此计算总完成百分比。调度间隔不是完成时限；新鲜度从扫描开始计。", "Actual reads include metadata and verification. Expected payload is not total I/O or a completion percentage. The scheduling interval is not a deadline; freshness starts when scanning begins.")}</p>
        </article>
    }
}
#[cfg(test)]
#[path = "../../../../test/web/unit/monitor/scrub/tests.rs"]
mod tests;
