use crate::files::commit_mode::{Choice, Target, can_change, can_load, load, submit};
use crate::ui::State;
use storage_contract::file_draft::CommitMode;
use web_interface::{i18n::t, prelude::*};

pub(crate) fn view(state: RwSignal<State>) -> impl IntoView {
    // Read on target admission, not on every reactive redraw or failed request.
    // A changed current location/session releases the old surface immediately.
    let observed = Memo::new(move |_| {
        state.with(|s| {
            (
                Target::capture(s),
                !s.files.loading
                    && !s.files.mutation_pending
                    && s.files.commit_mode.pending.is_none(),
            )
        })
    });
    Effect::new(move |_| {
        if let (Some(target), true) = observed.get()
            && state.with_untracked(|s| {
                can_load(s) && s.files.commit_mode.reviewed(s).is_none() && target.current(s)
            })
        {
            load(state);
        }
    });
    view! {
        <Show when=move ||state.with(|s| Target::capture(s).is_some())>
            <section class="zxs-entry-section zxs-stack" data-zxs-file-commit-mode="">
                <h3 class="zxs-row">{t("此文件的提交模式", "This file's commit mode")}</h3>
                <p class="zxs-row">{t(
                    "新文件默认手动。手动模式的普通保存只更新工作稿，显式提交才创建历史节点。模式仅影响此文件，不改变其他文件。",
                    concat!("New files default to Manual. Ordinary Manual saves update the draft; ",
                        "only an explicit commit creates history. ",
                        "This setting affects only this file."),
                )}</p>
                <button class="zxs-button"
                    disabled=move ||!state.with(can_load)
                    on:click=move |_|load(state)>
                    {t("读取当前模式", "Read current mode")}
                </button>
                <Show when=move ||state.with(|s| {
                    s.files.commit_mode.reading.as_ref().is_some_and(|target| target.current(s))
                })>
                    <p role="status">{t("正在读取…", "Reading…")}</p>
                </Show>
                {move ||state.with(|s| s.files.commit_mode.reviewed(s).map(|review| {
                    let manual = review.snapshot.mode() == CommitMode::Manual;
                    let dirty = manual && review.snapshot.dirty();
                    let protected=review.snapshot.controller().and_then(|c|c.protection()).is_some();
                    let status=super::warning::status(&review.snapshot);
                    view! {
                        {status}
                        <Show when=move ||dirty>
                            <p class="zxs-row" role="alert">{t(
                                "有未提交工作稿。请先显式提交或丢弃，再启用自动；切换不会隐式提交或删除工作稿。",
                                concat!("An uncommitted draft is present. ",
                                    "Commit or discard it before enabling Automatic. ",
                                    "Switching never commits or deletes a draft."),
                            )}</p>
                        </Show>
                        <div class="zxs-flow">
                            <Show when=move ||protected>
                                <button class="zxs-button" data-zxs-resume-automatic=""
                                    disabled=move ||!state.with(|s|can_change(s,Choice::Resume))
                                    on:click=move |_|submit(state,Choice::Resume)>
                                    {t("明确解除本次保护，仅处理以后的保存","Resume after this protection, for future saves only")}
                                </button>
                            </Show>
                            <button class="zxs-button" data-zxs-mode-manual=""
                                disabled=move ||!state.with(|s| can_change(s, CommitMode::Manual))
                                on:click=move |_|submit(state, CommitMode::Manual)>
                                {t("切换为手动", "Switch to Manual")}
                            </button>
                            <button class="zxs-button" data-zxs-mode-automatic=""
                                disabled=move ||!state.with(|s| {
                                    can_change(s, CommitMode::Automatic)
                                })
                                on:click=move |_|submit(state, CommitMode::Automatic)>
                                {t("明确启用自动提交", "Enable Automatic commits")}
                            </button>
                        </div>
                    }
                }))}
                <p class="zxs-row" role="note">{t(
                    "虚拟机盘等持续写入文件建议保持手动。切换到手动会保留已保存工作稿，并阻止新的自动发表。",
                    "Keep continuously written files such as VM disks in Manual. Switching to Manual preserves the saved draft and stops new automatic publications.",
                )}</p>
                <p class="zxs-row">{t(
                    "保存与发表是两次不同决定。启用自动不会发表已有工作稿；满足条件的后续保存才可创建自动节点。",
                    "Saving and publishing are separate decisions. Enabling Automatic never publishes an existing draft. Only eligible later saves can create automatic nodes.",
                )}</p>
                {move ||state.with(|s| s.files.commit_mode.error.clone())
                    .map(|error| view! { <p role="alert">{error.to_string()}</p> })}
            </section>
        </Show>
    }
}
