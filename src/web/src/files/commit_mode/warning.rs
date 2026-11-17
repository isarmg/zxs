//! Every displayed phase is a projection of an original working decision. A
//! durable save never implies a completed synthesis, history node, or backup.
use super::wire::Snapshot;
use storage_contract::file_draft::{
    CommitMode,
    automatic::{AutomaticPolicy, MAX_SAVES, ProtectionReason, SaveDecision, WINDOW_SECONDS},
};
use web_interface::{i18n::t, prelude::*};

fn decision(save: &SaveDecision) -> String {
    format!(
        "{} {} · {} {} · {} {} · {} {}",
        t("修订", "Revision"),
        save.revision,
        t("保存操作", "Save operation"),
        save.operation,
        t("决定序号", "Decision sequence"),
        save.sequence,
        t(
            "服务器时间（UTC Unix 秒）",
            "Server time (UTC Unix seconds)"
        ),
        save.server_time
    )
}
pub(super) fn status(snapshot: &Snapshot) -> AnyView {
    let revision = snapshot.revision();
    let mode = match snapshot.mode() {
        CommitMode::Manual => t("手动", "Manual"),
        CommitMode::Automatic => t("自动", "Automatic"),
    };
    let title = format!(
        "{}: {mode} · {}: {revision}",
        t("当前模式", "Current mode"),
        t("最新持久工作修订", "Latest durable working revision")
    );
    let Some(controller) = snapshot.controller() else {
        return view! {<p role="status">{title}</p>}.into_any();
    };
    let policy = match controller.policy() {
        AutomaticPolicy::TwentySavesInSixtySeconds => format!(
            "{}: {} {} / {} {} · {}: {}",
            t("保护策略", "Protection policy"),
            MAX_SAVES,
            t("次不同成功保存", "distinct successful saves"),
            WINDOW_SECONDS,
            t("秒", "seconds"),
            t("当前策略代数", "Current policy epoch"),
            controller.epoch()
        ),
    };
    let trigger=controller.protection().map(|trigger|{
        let reason=match trigger.reason{ProtectionReason::SaveFrequency=>t("保存过于频繁","Save frequency exceeded")};
        let text=format!("{reason}. {}: {} · {} · {}: {}",t("触发代数","Triggered epoch"),trigger.epoch,
            decision(trigger.decision()),t("有效窗口水位（UTC Unix 秒）","Effective window time (UTC Unix seconds)"),trigger.decision().effective_time);
        view!{<div role="alert" data-zxs-automatic-protection-warning="">
            <strong>{t("已暂停自动建节点。后续保存仍写入工作稿。","Automatic publication is paused. New saves still update the working draft.")}</strong>
            <p>{text}</p><p>{t("保护不会自行解除。可切回手动，或明确恢复自动。恢复不补建积压节点。","Protection does not clear by itself. Switch to Manual, or explicitly resume Automatic. Resume does not publish a backlog.")}</p>
        </div>}
    });
    let saved = controller
        .latest_save()
        .map(|s| {
            format!(
                "{}: {}",
                t("最近已持久保存", "Latest durable save"),
                decision(s)
            )
        })
        .unwrap_or_else(|| {
            t(
                "没有已记录的自动保存决定。",
                "No automatic save decision is recorded.",
            )
        });
    let pending = controller.pending_save().map(|s| {
        format!(
            "{}: {}",
            t(
                "等待自动发表的保存修订",
                "Save revision eligible for automatic publication"
            ),
            s.revision
        )
    });
    let node = controller
        .last_node()
        .map(|n| {
            format!(
                "{}: {} · {}: {} · {}: {} · {}: {}",
                t("最近实际发表节点", "Last actually published node"),
                n.node,
                t("发表操作", "Publication operation"),
                n.operation,
                t("源保存修订", "Source save revision"),
                n.save.revision,
                t("发表序号", "Publication sequence"),
                n.sequence
            )
        })
        .unwrap_or_else(|| {
            t(
                "这个控制器未记录已发表的自动节点。",
                "This controller records no published automatic node.",
            )
        });
    view!{<section class="zxs-stack zxs-stack--flush" data-zxs-file-automatic-state="">
        <p role="status">{title}</p><p>{policy}</p>
        {trigger}
        <p>{saved}</p>{pending.map(|p|view!{<p>{p}</p>})}<p>{node}</p>
        <p role="note">{t("完整正文的准备进度、备份结果另行核验。保存成功或已有历史节点都不是备份完成的凭据。",
            "Full-content preparation and backup outcomes are verified separately. Neither a saved draft nor a history node proves a backup completed.")}</p>
    </section>}.into_any()
}
