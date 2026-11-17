use crate::files::commit_mode::{Target, can_change, can_load, load, submit};
use crate::ui::State;
use storage_contract::file_draft::CommitMode;
use web_interface::{i18n::t, prelude::*};

pub(crate) fn view(state: RwSignal<State>) -> impl IntoView {
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
                    let manual = review.snapshot.working.mode() == CommitMode::Manual;
                    let dirty = review.snapshot.working.draft().is_some();
                    view! {
                        <p class="zxs-row" role="status">{if manual {
                            t("当前：手动", "Current: Manual")
                        } else { t("当前：自动", "Current: Automatic") }}</p>
                        <Show when=move ||dirty>
                            <p class="zxs-row" role="alert">{t(
                                "有未提交工作稿。请先显式提交或丢弃，再启用自动；切换不会隐式提交或删除工作稿。",
                                concat!("An uncommitted draft is present. ",
                                    "Commit or discard it before enabling Automatic. ",
                                    "Switching never commits or deletes a draft."),
                            )}</p>
                        </Show>
                        <div class="zxs-flow">
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
                    "高频更改保护尚未启用。虚拟机盘和其他持续写入的文件请保持手动。",
                    concat!("High-frequency protection is not active. Keep virtual machine disks ",
                        "and other continuously written files in Manual mode."),
                )}</p>
                <p class="zxs-row">{t(
                    "自动模式会在成功保存更改时创建历史节点。切换本身不创建节点，也不会提交已有工作稿。",
                    concat!("Automatic creates history when changed content is saved successfully. ",
                        "Switching itself creates no history and never commits an existing draft."),
                )}</p>
                {move ||state.with(|s| s.files.commit_mode.error.clone())
                    .map(|error| view! { <p role="alert">{error.to_string()}</p> })}
            </section>
        </Show>
    }
}
