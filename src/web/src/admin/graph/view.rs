use super::*;
use web_interface::i18n::t;

fn text_button(
    buttons: &mut Vec<(String, DetailRequest)>,
    label: String,
    text: &TextSummary,
    target: TextTarget,
) {
    if matches!(text, TextSummary::Deferred { .. }) {
        buttons.push((label, DetailRequest::Text { target, offset: 0 }));
    }
}
fn buttons(page: &GraphPage) -> Vec<(String, DetailRequest)> {
    let mut buttons = Vec::new();
    if let Some(provenance) = &page.history.provenance {
        text_button(
            &mut buttons,
            "history.provenance".into(),
            provenance,
            TextTarget::History {
                field: HistoryField::Provenance,
            },
        );
    }
    match &page.items {
        GraphItems::Commits(records) => {
            for record in records {
                for (field, value) in [
                    (CommitField::Kind, &record.kind),
                    (CommitField::TreeId, &record.tree_id),
                    (CommitField::Author, &record.author),
                    (CommitField::Message, &record.message),
                ] {
                    text_button(
                        &mut buttons,
                        format!("{}.{}", record.commit_id, field.as_str()),
                        value,
                        TextTarget::Commit {
                            id: record.commit_id.clone(),
                            field,
                        },
                    );
                }
                if let Some(value) = &record.provenance {
                    text_button(
                        &mut buttons,
                        format!("{}.provenance", record.commit_id),
                        value,
                        TextTarget::Commit {
                            id: record.commit_id.clone(),
                            field: CommitField::Provenance,
                        },
                    );
                }
                if record.pending_file_versions > 0 {
                    buttons.push((
                        format!("{}.pending_file_versions", record.commit_id),
                        DetailRequest::Pending {
                            target: PendingTarget::Commit {
                                id: record.commit_id.clone(),
                            },
                            after: None,
                        },
                    ));
                }
                if record.namespace_deletions.is_some() {
                    buttons.push((
                        format!("{}.namespace_deletions", record.commit_id),
                        DetailRequest::Deletions {
                            commit: record.commit_id.clone(),
                            offset: 0,
                        },
                    ));
                }
            }
        }
        GraphItems::Branches(records) => {
            for record in records {
                if record.pending_file_versions > 0 {
                    buttons.push((
                        format!("{}.pending_file_versions", record.branch_id),
                        DetailRequest::Pending {
                            target: PendingTarget::Branch {
                                id: record.branch_id.clone(),
                            },
                            after: None,
                        },
                    ));
                }
            }
        }
        GraphItems::MergeLinks(records) => {
            for record in records {
                text_button(
                    &mut buttons,
                    format!("{}.author", record.merge_link_id),
                    &record.author,
                    TextTarget::MergeLink {
                        id: record.merge_link_id.clone(),
                        field: MergeLinkField::Author,
                    },
                );
            }
        }
        GraphItems::BoundaryMarks(records) => {
            for record in records {
                text_button(
                    &mut buttons,
                    format!("{}.relation", record.boundary_id),
                    &record.relation,
                    TextTarget::Boundary {
                        id: record.boundary_id.clone(),
                        field: BoundaryField::Relation,
                    },
                );
                if let Some(value) = &record.source_merge_link_id {
                    text_button(
                        &mut buttons,
                        format!("{}.source_merge_link_id", record.boundary_id),
                        value,
                        TextTarget::Boundary {
                            id: record.boundary_id.clone(),
                            field: BoundaryField::SourceMergeLinkId,
                        },
                    );
                }
            }
        }
    }
    buttons
}
pub(super) fn inspector(state: RwSignal<State>, model: RwSignal<Model>) -> impl IntoView {
    let page = Memo::new(move |_| model.with(|m| m.page.clone()));
    let detail = Memo::new(move |_| model.with(|m| m.detail.clone()));
    let output = Memo::new(move |_| page.with(|page| page.as_ref().map(|page| pretty(&**page))));
    let detail_output =
        Memo::new(move |_| detail.with(|detail| detail.as_ref().map(|detail| detail.pretty())));
    view! {
        <section class="zxs-flow" aria-label=t("历史图", "History graph")>
            <h3>{t("历史图分页检查", "Paged history graph inspection")}</h3>
            <p>{t("每次仅显示当前页。长字段、待处理版本和删除记录可按需分段读取。", "Only the current page is retained. Read long fields, pending versions and deletions in bounded detail pages.")}</p>
            <nav class="zxs-row">{[
                (GraphCollection::Commits, t("提交", "Commits")),
                (GraphCollection::Branches, t("分支", "Branches")),
                (GraphCollection::MergeLinks, t("合并连线", "Merge links")),
                (GraphCollection::BoundaryMarks, t("边界", "Boundaries")),
            ].into_iter().map(move |(collection, label)| view! {
                <button class="zxs-button" aria-current=move || model.with(|m| (m.collection == collection).then_some("page"))
                    disabled=move || !state.with(State::session_active)
                    on:click=move |_| load_main(state, model, collection, None)>{label}</button>
            }).collect_view()}</nav>
            <div class="zxs-row">
                <button class="zxs-button" disabled=move || !state.with(State::session_active)
                    on:click=move |_| { let collection = model.with_untracked(|m| m.collection); load_main(state, model, collection, None); }>{t("重新加载", "Reload")}</button>
                <button class="zxs-button" disabled=move || model.with(|m| m.stale || m.main_read.cancellation.is_some() || m.page.as_ref().is_none_or(|page| page.next_cursor.is_none()))
                    on:click=move |_| {
                        if let Some((collection, cursor)) = model.with_untracked(|m| m.page.as_ref().and_then(|page| page.next_cursor.as_ref().map(|cursor| (m.collection, cursor.clone())))) {
                            load_main(state, model, collection, Some(cursor));
                        }
                    }>{t("下一页", "Next page")}</button>
            </div>
            <Show when=move || model.with(|m| m.main_read.cancellation.is_some())><p>{t("正在读取…", "Loading…")}</p></Show>
            <Show when=move || model.with(|m| m.stale)><p role="alert">{t("历史已变化，请重新加载。", "The history changed. Reload to continue.")}</p></Show>
            <p role="alert">{move || model.with(|m| m.error.as_ref().map(ToString::to_string))}</p>
            <pre>{move || output.get()}</pre>
            <div class="zxs-flow">{move || page.with(|page| page.as_ref().map(|page| buttons(page)).unwrap_or_default()).into_iter().map(move |(label, request)| view! {
                <button class="zxs-button" disabled=move || model.with(|m| m.stale || m.main_read.cancellation.is_some())
                    on:click=move |_| load_detail(state, model, request.clone())>{label}</button>
            }).collect_view()}</div>
            <Show when=move || model.with(|m| m.detail_read.cancellation.is_some())><p>{t("正在读取详情…", "Loading detail…")}</p></Show>
            <Show when=move || detail.with(Option::is_some)>
                <h3>{t("当前详情片段", "Current detail page")}</h3>
                <pre>{move || detail_output.get()}</pre>
                <button class="zxs-button" disabled=move || model.with(|m| m.stale || m.detail_read.cancellation.is_some() || m.detail.as_ref().is_none_or(|detail| !detail.has_next()))
                    on:click=move |_| { if let Some(request) = model.with_untracked(|m| m.detail.as_ref().and_then(|detail| detail.next())) { load_detail(state, model, request); } }>{t("下一段详情", "Next detail page")}</button>
                <button class="zxs-button" on:click=move |_| model.update(|m| { m.detail_read.cancel(); m.detail = None; })>{t("关闭详情", "Close detail")}</button>
            </Show>
        </section>
    }
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/admin/graph/view_tests.rs"]
mod tests;
