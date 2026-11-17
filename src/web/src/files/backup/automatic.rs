//! Native directory protection shares the existing editor and operation polling.
use crate::files::{actions, backup, review};
use crate::ui::State;
use data_serialization::Serialize;
use name_policy::PortablePath;
use storage_contract::{EntryId, Generation, NamespaceEntryKind, RepositoryId};
use web_interface::{i18n::t, prelude::*};

#[derive(Debug, Serialize)]
struct Body {
    path: PortablePath,
    directory_id: EntryId,
    target_copies: u32,
    expected_generation: Generation,
}

struct Request {
    repository: RepositoryId,
    body: Body,
}

impl Request {
    fn endpoint(&self) -> String {
        format!(
            "/api/repositories/{}/automatic-directory-backup-configuration",
            self.repository,
        )
    }
}

fn ordinary_directory(draft: &backup::Draft) -> bool {
    !draft.path.is_empty()
        && draft
            .configuration
            .as_ref()
            .is_some_and(|configuration| configuration.scope.kind == NamespaceEntryKind::Directory)
}

fn request(draft: &backup::Draft) -> Result<Request, String> {
    let Some(configuration) = draft
        .configuration
        .as_ref()
        .filter(|_| !draft.loading && ordinary_directory(draft))
    else {
        return Err(t(
            "先读取一个普通目录的备份设置。磁盘根和文件不使用此入口。",
            "Load an ordinary folder's backup settings first, not a disk root or file.",
        ));
    };
    if configuration.target_copies == 0
        || configuration.target_copies as usize > backup::max_copies(configuration).min(3)
    {
        return Err(t(
            "自动目录备份须在当前路径内选择 1 至 3 份（含主盘）。",
            "Choose 1 to 3 copies within this route, including the primary.",
        ));
    }
    let invalid = |error| {
        format!(
            "{}: {error}",
            t(
                "目录身份无效，请关闭后重新读取",
                "Invalid folder identity; close and reload"
            ),
        )
    };
    Ok(Request {
        repository: draft.repository.parse().map_err(invalid)?,
        body: Body {
            path: PortablePath::parse(&draft.path).map_err(|error| error.to_string())?,
            directory_id: configuration.scope.id.parse().map_err(invalid)?,
            target_copies: configuration.target_copies,
            expected_generation: Generation::new(configuration.generation),
        },
    })
}

fn submit(state: RwSignal<State>) {
    if !state.with_untracked(actions::write_allowed) {
        return;
    }
    let Some(draft) = state.with_untracked(|s| s.files.backup.clone()) else {
        return;
    };
    let result = request(&draft).and_then(|request| {
        let body = json_codec::to_value(&request.body)
            .map_err(|source| backup::configuration_error(source).to_string())?;
        Ok((request.endpoint(), body))
    });
    match result {
        Ok((endpoint, body)) => {
            // Review freezes one operation and this exact body before sending.
            // Unknown results only enter existing operation polling, not resend.
            review::submit(state, review::Kind::Backup, "PUT", endpoint, body);
        }
        Err(error) => review::error(state, review::Kind::Backup, error),
    }
}

pub(super) fn view(state: RwSignal<State>) -> impl IntoView {
    let visible = move || state.with(|s| s.files.backup.as_ref().is_some_and(ordinary_directory));
    let disabled = move || {
        state.with(|s| {
            !actions::write_allowed(s)
                || s.files
                    .backup
                    .as_ref()
                    .is_none_or(|draft| request(draft).is_err())
        })
    };
    let save = move |_| submit(state);
    view! {
        <Show
            when=visible
        >
            <section
                class="zxs-stack"
                aria-label=t("自动目录备份", "Automatic folder backups")
            >
                <h3
                    class="zxs-row"
                >
                    {t("自动目录备份", "Automatic folder backups")}
                </h3>
                <p
                    class="zxs-row"
                >
                    {t(
                    concat!(
                        "普通目录可按上方份数设置自动备份（最多 3 份，含主盘）。",
                        "保存会补齐仍可用的历史正文及当前空子目录，",
                        "后续新文件继承此规则；独立子规则保持不变。",
                    ),
                    concat!(
                        "Set automatic protection for an ordinary folder using the count above ",
                        "(up to 3, including the primary). Saving backfills available history ",
                        "and current empty descendants. Future files inherit the rule; ",
                        "independent child rules stay unchanged.",
                    ),
                    )}
                </p>
                <p
                    class="zxs-row"
                >
                    {t(
                    concat!(
                        "此动作不使用上方自定义前缀。实际目录按来源磁盘名和 UUID 逐级生成，",
                        "不是更换备份盘，也不包含 .zxs 快照或 worktree。",
                    ),
                    concat!(
                        "This action does not use custom prefixes above. Native paths follow ",
                        "source disk names and UUIDs at each tier. It does not replace a backup ",
                        "disk or protect .zxs snapshots or worktrees.",
                    ),
                    )}
                </p>
                <button
                    class="zxs-button"
                    disabled=disabled
                    on:click=save
                >
                    {t("保存自动目录备份", "Save automatic folder backups")}
                </button>
                <p
                    class="zxs-row"
                >
                    {t(
                    concat!(
                        "结果未确认时仅查询原操作，不自动重发。",
                        "请勿刷新或退出登录；此面板尚不保存跨刷新恢复记录。",
                    ),
                    concat!(
                        "An unconfirmed result only polls the original operation, without ",
                        "resending. Do not refresh or sign out: this panel does not yet retain ",
                        "recovery records across reloads.",
                    ),
                    )}
                </p>
            </section>
        </Show>
    }
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/backup/automatic_tests.rs"]
mod tests;
