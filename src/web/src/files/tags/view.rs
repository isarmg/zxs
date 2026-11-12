//! A container-wide editor, deliberately outside the version graph.
use crate::{
    files::{
        actions, model, review,
        tags::{
            Target, accept_current, add_input, cancel, edit, reload, remove, start, submit,
        },
    },
    ui::State,
};
use web_interface::{i18n::t, prelude::*};

pub(crate) fn view(state: RwSignal<State>) -> impl IntoView {
    // ui::app owns this signal above the detail view. Construction is synchronous;
    // only the later cleanup/async callbacks may observe owner disposal.
    let mount = state
        .try_update(|state| {
            state.files.container_tags.mount += 1;
            state.files.container_tags.mount
        })
        .expect("invariant: mounted container tag panel owns its read");
    let target = Memo::new(move |_| state.with(Target::capture));
    Effect::new(move |_| synchronize_target(state, mount, target.get()));
    on_cleanup(move || clear_mount(state, mount));
    let disabled = Signal::derive(move || state.with(write_disabled));
    view! {
        <section
            class="zxs-entry-section zxs-stack"
            aria-label=t("容器标签", "Container tags")
        >
            <h3
                class="zxs-row"
            >
                {t("容器标签", "Container tags")}
            </h3>
            <p
                class="zxs-row"
            >
                {t(
                    "同一文件的全部版本和分支共享这些标签。",
                    "All versions and branches of this file share these tags.",
                )}
            </p>
            <p
                class="zxs-row"
                role="status"
            >
                {move || state.with(read_status)}
            </p>
            <div
                class="zxs-file-toolbar zxs-flow"
            >
                <button
                    class="zxs-button"
                    disabled=move || state.with(reload_disabled)
                    on:click=move |_| reload(state)
                >
                    {t("重新读取标签", "Reload tags")}
                </button>
                <button
                    class="zxs-button"
                    disabled=move || disabled.get() || state.with(start_disabled)
                    on:click=move |_| start(state)
                >
                    {t("编辑容器标签", "Edit container tags")}
                </button>
            </div>
            <For
                each=move || state.with(editor_instances)
                key=|instance| *instance
                children=move |instance| editor(state, instance, disabled)
            />
        </section>
    }
}

fn synchronize_target(state: RwSignal<State>, mount: u64, target: Option<Target>) {
    if state.with_untracked(|state| state.files.container_tags.mount != mount) {
        return;
    }
    state.update(|state| state.files.container_tags.set_target(target.clone()));
    if target.is_some() {
        reload(state);
    }
}

fn clear_mount(state: RwSignal<State>, mount: u64) {
    let _ = state.try_update(|state| {
        if state.files.container_tags.mount == mount {
            state.files.container_tags.clear();
        }
    });
}

fn write_disabled(state: &State) -> bool {
    !actions::write_allowed(state) || model::selected_entry_read_only(&state.files)
}

fn reload_disabled(state: &State) -> bool {
    actions::pending(state)
        || state.files.container_tags.loading
        || Target::capture(state).is_none()
}

fn start_disabled(state: &State) -> bool {
    state.files.tag_edit.is_some() || state.files.container_tags.snapshot.is_none()
}

fn read_status(state: &State) -> String {
    let read = &state.files.container_tags;
    if read.loading {
        return t("正在读取标签…", "Loading tags…");
    }
    if let Some(error) = &read.error {
        return error.to_string();
    }
    read.snapshot
        .as_ref()
        .map(|snapshot| {
            if snapshot.tags.is_empty() {
                t("无标签", "No tags")
            } else {
                snapshot.tags.iter().map(|tag| tag.as_str()).collect::<Vec<_>>().join(" / ")
            }
        })
        .unwrap_or_else(|| t("标签尚未读取。", "Tags have not been read."))
}

fn editor_instances(state: &State) -> Vec<u64> {
    state.files.tag_edit.as_ref().map(|draft| vec![draft.instance()]).unwrap_or_default()
}

fn editor_is_stale(state: &State, instance: u64) -> bool {
    state.files.tag_edit
        .as_ref()
        .filter(|draft| draft.instance() == instance)
        .is_none_or(|draft| Target::capture(state).as_ref() != Some(&draft.owner.target))
}

fn editor_tags(state: &State, instance: u64) -> Vec<String> {
    state.files.tag_edit
        .as_ref()
        .filter(|draft| draft.instance() == instance)
        .map(|draft| draft.command.tags.iter().map(|tag| tag.as_str().to_owned()).collect())
        .unwrap_or_default()
}

fn input_value(state: &State) -> String {
    state.files.tag_edit.as_ref().map(|draft| draft.input.clone()).unwrap_or_default()
}

fn needs_review(state: &State) -> bool {
    state.files.tag_edit.as_ref().is_some_and(|draft| draft.requires_review())
}

fn save_disabled(state: &State) -> bool {
    state.files.tag_edit.as_ref().is_none_or(|draft| draft.requires_review())
}

fn accept_disabled(state: &State) -> bool {
    state.files.container_tags.loading || state.files.container_tags.snapshot.is_none()
}

fn edit_input(state: RwSignal<State>, instance: u64, event: web_sys::Event) {
    edit(state, instance, |draft| draft.input = event_target_value(&event));
}

fn input_keydown(state: RwSignal<State>, instance: u64, event: web_sys::KeyboardEvent) {
    if event.key() == "Enter" && !event.is_composing() {
        event.prevent_default();
        add_input(state, instance);
    }
}

fn tag_button(
    state: RwSignal<State>,
    instance: u64,
    disabled: Signal<bool>,
    tag: String,
) -> impl IntoView {
    let remove_tag = tag.clone();
    view! {
        <button
            class="zxs-button"
            disabled=disabled
            on:click=move |_| remove(state, instance, &remove_tag)
        >
            {format!("{} {tag}", t("移除", "Remove"))}
        </button>
    }
}

fn editor(state: RwSignal<State>, instance: u64, disabled: Signal<bool>) -> impl IntoView {
    let stale = Memo::new(move |_| state.with(|state| editor_is_stale(state, instance)));
    let disabled = Signal::derive(move || disabled.get() || stale.get());
    let tags = Memo::new(move |_| state.with(|state| editor_tags(state, instance)));
    view! {
        <div
            class="zxs-stack"
            data-container-tag-editor=""
        >
            <Show
                when=move || stale.get()
            >
                <p
                    class="zxs-row"
                    role="status"
                >
                    {t(
                        "文件上下文已刷新，原草稿已保留但不能提交。取消后按当前上下文重新编辑。",
                        "The file context changed. Your draft is kept but cannot be submitted. \
                         Cancel it and edit again in the current context.",
                    )}
                </p>
            </Show>
            <div
                class="zxs-node-tags zxs-flow"
            >
                <For
                    each=move || tags.get()
                    key=|tag| tag.clone()
                    children=move |tag| tag_button(state, instance, disabled, tag)
                />
            </div>
            <label
                class="zxs-form-field"
            >
                <span
                    class="zxs-row"
                >
                    {t("新增标签", "Add tag")}
                </span>
                <input
                    class="zxs-input"
                    disabled=disabled
                    data-max-bytes=storage_contract::file_container_tags::MAX_TAG_BYTES.to_string()
                    prop:value=move || state.with(input_value)
                    on:input=move |event| edit_input(state, instance, event)
                    on:keydown=move |event| input_keydown(state, instance, event)
                />
            </label>
            <div
                class="zxs-file-toolbar zxs-flow"
            >
                <button
                    class="zxs-button"
                    disabled=disabled
                    on:click=move |_| add_input(state, instance)
                >
                    {t("添加标签", "Add tag")}
                </button>
                <button
                    class="zxs-button"
                    disabled=move || state.with(actions::pending)
                    on:click=move |_| cancel(state, instance)
                >
                    {t("取消", "Cancel")}
                </button>
                <button
                    class="zxs-button"
                    disabled=move || disabled.get() || state.with(save_disabled)
                    on:click=move |_| submit(state, instance)
                >
                    {t("保存标签", "Save tags")}
                </button>
            </div>
            {review_confirmation(state, instance, disabled)}
            <p
                class="zxs-row zxs-inline-error"
                role="status"
            >
                {move || state.with(|state| review::error_text(state, review::Kind::Tags))}
            </p>
        </div>
    }
}

fn review_confirmation(
    state: RwSignal<State>,
    instance: u64,
    disabled: Signal<bool>,
) -> impl IntoView {
    view! {
        <Show
            when=move || state.with(needs_review)
        >
            <p
                class="zxs-row"
                role="status"
            >
                {t(
                    "标签已改变。重新读取后核对当前标签，再明确确认替换。草稿不会自动合并。",
                    "Tags changed. Reload and review current tags, then confirm replacement. \
                     Your draft is not merged automatically.",
                )}
            </p>
            <button
                class="zxs-button"
                disabled=move || disabled.get() || state.with(accept_disabled)
                on:click=move |_| accept_current(state, instance)
            >
                {t("按重读结果确认替换", "Confirm replacement against reloaded tags")}
            </button>
        </Show>
    }
}
