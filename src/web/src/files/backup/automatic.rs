//! Native file and directory protection share the editor and operation polling.
use crate::files::{actions, backup, model::file_context, page::Entry, review};
use crate::ui::State;
use data_serialization::Serialize;
use name_policy::PortablePath;
use storage_contract::{EntryId, FileId, Generation, NamespaceEntryKind, RepositoryId};
use web_interface::{i18n::t, prelude::*};

#[derive(Clone)]
pub(super) struct FileTarget {
    repository: RepositoryId,
    path: PortablePath,
    entry: EntryId,
    file: FileId,
}

impl FileTarget {
    pub(super) fn capture(entry: &Entry) -> Result<Option<Self>, String> {
        if entry.kind() != NamespaceEntryKind::File || entry.unborn().is_some() {
            return Ok(None);
        }
        let (repository, path) = file_context(entry).ok_or_else(stale_scope)?;
        Ok(Some(Self {
            repository: repository
                .parse()
                .map_err(|error: storage_contract::Error| format!("{}: {error}", stale_scope()))?,
            path: PortablePath::parse(&path).map_err(|error| error.to_string())?,
            entry: entry
                .entry_id()
                .parse()
                .map_err(|error: storage_contract::Error| format!("{}: {error}", stale_scope()))?,
            file: entry
                .file_id()
                .parse()
                .map_err(|error: storage_contract::Error| format!("{}: {error}", stale_scope()))?,
        }))
    }
}

#[derive(Debug, Serialize)]
#[serialization(untagged)]
enum Body {
    Directory {
        path: PortablePath,
        directory_id: EntryId,
        target_copies: u32,
        expected_generation: Generation,
    },
    File {
        path: PortablePath,
        entry_id: EntryId,
        file_id: FileId,
        target_copies: u32,
        expected_generation: Generation,
    },
}

struct Request {
    repository: RepositoryId,
    body: Body,
}

impl Request {
    fn endpoint(&self) -> String {
        let endpoint = match &self.body {
            Body::Directory { .. } => "automatic-directory-backup-configuration",
            Body::File { .. } => "automatic-file-backup-configuration",
        };
        format!("/api/repositories/{}/{endpoint}", self.repository)
    }
}

fn ordinary_directory(draft: &backup::Draft) -> bool {
    !draft.path.is_empty()
        && draft
            .configuration
            .as_ref()
            .is_some_and(|configuration| configuration.scope.kind == NamespaceEntryKind::Directory)
}

fn file_scope(draft: &backup::Draft) -> bool {
    draft.automatic_file.is_some()
        && draft
            .configuration
            .as_ref()
            .is_some_and(|configuration| configuration.scope.kind == NamespaceEntryKind::File)
}

fn stale_scope() -> String {
    t(
        "条目身份或路径已失效，请关闭后重新读取。",
        "The entry identity or path is stale. Close and reload.",
    )
}

fn request(draft: &backup::Draft) -> Result<Request, String> {
    let Some(configuration) = draft
        .configuration
        .as_ref()
        .filter(|_| !draft.loading && (ordinary_directory(draft) || file_scope(draft)))
    else {
        return Err(stale_scope());
    };
    if configuration.target_copies == 0
        || configuration.target_copies as usize > backup::max_copies(configuration).min(3)
    {
        return Err(t(
            "自动备份须在当前路径内选择 1 至 3 份（含主盘）。",
            "Choose 1 to 3 copies within this route, including the primary.",
        ));
    }
    let invalid = |error| format!("{}: {error}", stale_scope());
    let repository = draft.repository.parse().map_err(invalid)?;
    let path = PortablePath::parse(&draft.path).map_err(|error| error.to_string())?;
    let target_copies = configuration.target_copies;
    let expected_generation = Generation::new(configuration.generation);
    let body = match configuration.scope.kind {
        NamespaceEntryKind::Directory => Body::Directory {
            path,
            directory_id: configuration.scope.id.parse().map_err(invalid)?,
            target_copies,
            expected_generation,
        },
        NamespaceEntryKind::File => {
            let target = draft.automatic_file.as_ref().ok_or_else(stale_scope)?;
            let file = configuration.scope.id.parse::<FileId>().map_err(invalid)?;
            if repository != target.repository || path != target.path || file != target.file {
                return Err(stale_scope());
            }
            Body::File {
                path,
                entry_id: target.entry,
                file_id: file,
                target_copies,
                expected_generation,
            }
        }
    };
    Ok(Request { repository, body })
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
    let visible = move || {
        state.with(|s| {
            s.files
                .backup
                .as_ref()
                .is_some_and(|draft| ordinary_directory(draft) || file_scope(draft))
        })
    };
    let is_file = move || state.with(|s| s.files.backup.as_ref().is_some_and(file_scope));
    let disabled = move || {
        state.with(|s| {
            !actions::write_allowed(s)
                || s.files
                    .backup
                    .as_ref()
                    .is_none_or(|draft| request(draft).is_err())
        })
    };
    let title = move || {
        if is_file() {
            t("自动文件备份", "Automatic file backups")
        } else {
            t("自动目录备份", "Automatic folder backups")
        }
    };
    let directory = move || !is_file();
    let save_label = move || {
        if is_file() {
            t("保存自动文件备份", "Save automatic file backups")
        } else {
            t("保存自动目录备份", "Save automatic folder backups")
        }
    };
    let save = move |_| submit(state);
    view! {
        <Show
            when=visible
        >
            <section
                class="zxs-stack"
                aria-label=t("自动备份设置", "Automatic backup settings")
            >
                <h3
                    class="zxs-row"
                >
                    {title}
                </h3>
                <Show
                    when=directory
                >
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
                            "Set automatic protection for an ordinary folder using the count ",
                            "above (up to 3, including the primary). Saving backfills available ",
                            "history ",
                            "and current empty descendants. Future files inherit the rule; ",
                            "independent child rules stay unchanged.",
                        ),
                        )}
                    </p>
                </Show>
                <Show
                    when=is_file
                >
                    <p
                        class="zxs-row"
                    >
                        {t(
                            concat!(
                                "仅设置当前准确文件的备份份数（含主盘）。",
                                "保存会补齐该文件仍可用的历史正文；后续版本沿用此规则。",
                                "不修改父目录或其他文件，也不改变手动/自动提交模式。",
                            ),
                            concat!(
                                "Set the copy count for this exact file, including the primary. ",
                                "Saving backfills its available history; later versions keep the ",
                                "rule. Parent folders, other files and commit mode stay unchanged.",
                            ),
                        )}
                    </p>
                </Show>
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
                    {save_label}
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
