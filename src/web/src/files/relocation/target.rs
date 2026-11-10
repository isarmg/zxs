//! Resolve the displayed target parent once through the existing listing API.
use super::*;
use crate::files::page::{DirectoryKey, Page, PageParts};

pub(super) fn input_error(source: storage_contract::Error) -> api::Error {
    let mut error = api::error(
        storage_contract::ErrorCode::InvalidRequest,
        source.to_string(),
    );
    error.source = Some(std::sync::Arc::new(api::ErrorSource::ObjectMoveInput(
        source,
    )));
    error
}

pub(super) async fn review(
    state: RwSignal<State>,
    editor: Editor,
    read: &ReadTarget,
    destination: name_policy::PortablePath,
) -> Result<Option<Review>, api::Error> {
    let basis = read.basis.clone();
    let slot = read.slot.clone();
    let (parent, name) = destination
        .as_str()
        .rsplit_once('/')
        .unwrap_or(("", destination.as_str()));
    let page = folder_page(&slot, parent, 1, None, read.cancellation.clone()).await?;
    if !page.writable {
        return Err(api::error(
            storage_contract::ErrorCode::InvalidRequest,
            t(
                "目标父目录必须是可写的当前工作目录。",
                "The destination parent must be a writable current folder.",
            ),
        ));
    }
    let context = page.context;
    if parent.is_empty() {
        if !read.current(state, editor) {
            return Ok(None);
        }
        // A completed request drops and aborts its controller. Continue this
        // same review generation with a new controller owned by the editor.
        let cancellation = api::ReadCancellation::new()?;
        editor.cancellation.set(Some(cancellation.clone()));
        let target = folder_page(&slot, destination.as_str(), 1, None, cancellation).await;
        check_root_target(context.as_ref(), target)?;
    }
    let target_path = name_policy::PortablePath::parse(&super::super::model::join_path(
        context
            .as_ref()
            .map_or("", |context| context.repository_path()),
        name,
    ))
    .map_err(|source| input_error(source.into()))?;
    if basis.slot == slot
        && context
            .as_ref()
            .is_some_and(|context| basis.repository == context.repository_id())
        && basis.source == target_path.as_str()
    {
        return Err(api::error(
            storage_contract::ErrorCode::InvalidRequest,
            t("目标位置未改变。", "The destination is unchanged."),
        ));
    }
    let request = ObjectMoveRequest {
        source_repository_id: HistoryRepositoryId::parse(&basis.repository).map_err(input_error)?,
        source_entry_id: NamespaceEntryId::parse(&basis.entry).map_err(input_error)?,
        source_path: name_policy::LookupPath::parse(&basis.source)
            .map_err(|source| input_error(source.into()))?,
        target_repository_id: context
            .as_ref()
            .map(|context| HistoryRepositoryId::parse(context.repository_id()))
            .transpose()
            .map_err(input_error)?,
        target_slot_id: slot,
        target_path,
        replace: false,
        target_entry_id: None,
    };
    Ok(Some(Review {
        basis,
        destination: destination.as_str().to_owned(),
        request,
    }))
}

fn check_root_target(
    parent: Option<&crate::files::page::Context>,
    target: Result<PageParts, api::Error>,
) -> Result<(), api::Error> {
    // Named repositories occupy root display names outside the root namespace.
    match target {
        Ok(target)
            if target.context.as_ref().is_some_and(|target| {
                target.repository_path().is_empty()
                    && parent.is_none_or(|parent| parent.repository_id() != target.repository_id())
            }) =>
        {
            Err(api::error(
                storage_contract::ErrorCode::NameCollision,
                t(
                    "目标名称已被命名仓库占用，请选择仓库内的位置或其他名称。",
                    "This name belongs to a named repository. Choose a location inside it or another name.",
                ),
            ))
        }
        Ok(_) => Ok(()),
        // A file also produces DirectoryNotFound. Ordinary existence and
        // replacement admission remain owned by the original move operation.
        Err(error) if error.code == storage_contract::ErrorCode::DirectoryNotFound => Ok(()),
        Err(error) => Err(error),
    }
}

pub(super) async fn folder_page(
    slot: &str,
    parent: &str,
    limit: usize,
    cursor: Option<&str>,
    cancellation: api::ReadCancellation,
) -> Result<PageParts, api::Error> {
    let parent_path = if parent.is_empty() {
        None
    } else {
        Some(
            name_policy::PortablePath::parse(parent)
                .map_err(|source| input_error(source.into()))?,
        )
    };
    let key = DirectoryKey::destination("", parent_path.as_ref());
    let mut endpoint = format!(
        "/api/file-manager/disks/{}/entries?path={}&limit={limit}",
        api::segment(slot),
        api::segment(parent)
    );
    if let Some(cursor) = cursor {
        endpoint.push_str(&format!("&cursor={}", api::segment(cursor)));
    }
    let page = Page::parse(
        api::cancellable_get(&endpoint, cancellation).await?,
        slot,
        &key,
        limit,
        None,
    )?
    .into_parts();
    if page.preview || (page.context.is_none() && !parent.is_empty()) {
        return Err(api::error(
            storage_contract::ErrorCode::InvalidRequest,
            t(
                "目标必须是当前目录，并具有明确的仓库位置。",
                "The destination must be a current folder with a repository location.",
            ),
        ));
    }
    Ok(page)
}

#[cfg(test)]
#[path = "../../../../../test/web/unit/files/relocation_target_tests.rs"]
mod tests;
