//! Real child HEADs and owned-boundary review; no response or state replacement.
use super::*;
use storage_engine::directory_git::{DirectoryGitChildStatus, DirectoryGitStatus};

fn read_count(browser: &Browser) -> Result<u64> {
    browser
        .script("return window.__zxsDirectoryGit.reads.length")?
        .as_u64()
        .ok_or_else(|| "invalid directory Git read count".into())
}
fn current(
    browser: &Browser,
    repository: Option<&str>,
    path: &str,
    after: u64,
) -> Result<DirectoryGitStatus> {
    wait_for("fresh rendered directory repository status", || {
        let snapshot = browser.script(r#"
            const p=window.__zxsDirectoryGit,read=p.reads.at(-1),s=read?.body;
            const row=document.querySelector('[data-directory-staging]');
            const ids=Array.from(document.querySelectorAll('[data-directory-git-child]'),e=>e.dataset.directoryGitChild).sort();
            return {count:p.reads.length,read,ready:read?.status===200&&!!row&&
                !!document.querySelector('[data-directory-git-refresh]:not([disabled])')&&
                !!document.querySelector('.zxs-file-manager[aria-busy="false"]')&&
                row.dataset.directoryGitInitialized===String(s.initialized)&&
                row.dataset.directoryGitStaged===String(s.staged)&&
                row.dataset.directoryGitUnstaged===String(s.unstaged)&&
                row.dataset.directoryGitNeedsRestage===String(s.needs_restage)&&
                JSON.stringify(ids)===JSON.stringify(s.children.map(child=>child.directory_id).sort())};
        "#)?;
        if snapshot["ready"] != true
            || snapshot["count"]
                .as_u64()
                .is_none_or(|count| count <= after)
        {
            return Err(
                "directory repository response has not reached its exact rendered scope".into(),
            );
        }
        let status: DirectoryGitStatus = json_codec::from_value(snapshot["read"]["body"].clone())?;
        if status.path != path || repository.is_some_and(|id| status.repository_id != id) {
            return Err("directory repository response belongs to another scope".into());
        }
        Ok(status)
    })
}
fn open_root(browser: &Browser) -> Result<()> {
    browser.click(&browser.button("Files", "文件")?)?;
    browser.click(&browser.button("All disks", "所有硬盘")?)?;
    browser.click(&browser.css(".zxs-disk-entry[data-slot='P01']:not([disabled])")?)?;
    browser.click(&browser.element(
        "xpath",
        &format!(
            "//button[contains(@class,'zxs-file-name')][normalize-space(.)='{}']",
            fixture::REPOSITORY,
        ),
    )?)?;
    browser.element(
        "xpath",
        "//button[contains(@class,'zxs-file-name')][normalize-space(.)='parent.txt']",
    )?;
    Ok(())
}
fn child<'a>(parent: &'a DirectoryGitStatus, path: &str) -> Result<&'a DirectoryGitChildStatus> {
    parent
        .children
        .iter()
        .find(|child| child.path == path)
        .ok_or_else(|| format!("missing independent child {path}").into())
}
fn mutate(browser: &Browser, selector: &str, endpoint: &str, expected: Value) -> Result<u64> {
    browser.css(&format!("{selector}:not([disabled])"))?;
    let before = browser.script("return {writes:window.__zxsDirectoryGit.writes.length,reads:window.__zxsDirectoryGit.reads.length}")?;
    let writes = before["writes"].as_u64().ok_or("invalid write count")?;
    let reads = before["reads"].as_u64().ok_or("invalid read count")?;
    browser.script(&format!(
        "const b=document.querySelector({});b.click();b.click();return true",
        json!(selector)
    ))?;
    wait_for(
        "one permanent directory mutation receipt after duplicate clicks",
        || {
            let observed = browser.script("return {writes:window.__zxsDirectoryGit.writes,responses:window.__zxsDirectoryGit.responses,unexpected:window.__zxsDirectoryGit.unexpected}")?;
            if observed["writes"].as_array().map(Vec::len) != Some(writes as usize + 1)
                || observed["responses"].as_array().map(Vec::len) != Some(writes as usize + 1)
                || observed["unexpected"] != json!([])
            {
                return Err(
                    format!("directory mutation count or receipt mismatch: {observed}").into(),
                );
            }
            let write = &observed["writes"][writes as usize];
            let mut body = write["body"]
                .as_object()
                .ok_or("missing mutation body")?
                .clone();
            let operation = body.remove("operation_id").ok_or("missing operation ID")?;
            let receipt = &observed["responses"][writes as usize];
            if write["path"] != endpoint
                || Value::Object(body) != expected
                || !operation
                    .as_str()
                    .is_some_and(|id| id.parse::<storage_engine::OperationId>().is_ok())
                || receipt["status"] != 200
                || receipt["body"]["operation_id"] != operation
                || receipt["body"]["state"] != "completed"
                || receipt["body"]["error"] != Value::Null
            {
                return Err(format!(
                    "mutation did not preserve its exact reviewed inputs: {observed}"
                )
                .into());
            }
            Ok(reads)
        },
    )
}
fn stage_child(browser: &Browser, parent: &DirectoryGitStatus) -> Result<DirectoryGitStatus> {
    let child = child(parent, "child")?;
    let after = mutate(
        browser,
        &format!("[data-stage-directory-child='{}']", child.directory_id),
        &format!(
            "/api/repositories/{}/directory-git/children/stage",
            parent.repository_id
        ),
        json!({"directory_id":parent.directory_id,"directory_branch_id":parent.directory_branch_id,
            "expected_index_generation":parent.index_generation.to_string(),"child_directory_id":child.directory_id,
            "expected_child_head_tree_id":child.head_tree_id}),
    )?;
    current(browser, Some(&parent.repository_id), "", after)
}
fn commit(browser: &Browser, status: &DirectoryGitStatus) -> Result<DirectoryGitStatus> {
    browser.css(".zxs-directory-staging input:not([disabled])")?;
    browser.script("const input=document.querySelector('.zxs-directory-staging input');input.value='Explicit independent repository review';input.dispatchEvent(new Event('input',{bubbles:true}));return true")?;
    let after = mutate(
        browser,
        "[data-commit-directory]",
        &format!(
            "/api/repositories/{}/directory-branches/{}/commits",
            status.repository_id,
            status
                .directory_branch_id
                .as_deref()
                .ok_or("missing initialized branch")?
        ),
        json!({"expected_tree_id":status.working_tree_id,"expected_index_generation":status.index_generation.to_string(),
            "message":"Explicit independent repository review"}),
    )?;
    let committed = current(browser, Some(&status.repository_id), &status.path, after)?;
    if committed.staged || committed.needs_restage || committed.head_tree_id == status.head_tree_id
    {
        return Err("explicit commit did not publish a new clean user HEAD".into());
    }
    Ok(committed)
}
fn review_boundaries(browser: &Browser, parent: &DirectoryGitStatus) -> Result<DirectoryGitStatus> {
    let original = child(parent, "child")?;
    if !parent.needs_restage
        || !original.dirty
        || original.head_tree_id.is_none()
        || child(parent, "unborn")?.head_tree_id.is_some()
    {
        return Err("child repository fixture did not retain its intended real states".into());
    }
    browser.css(&format!(
        "[data-stage-directory-child='{}'][disabled]",
        child(parent, "unborn")?.directory_id
    ))?;
    browser.css("[data-commit-directory][disabled]")?;
    let pinned = stage_child(browser, parent)?;
    if !pinned.needs_restage || child(&pinned, "child")?.staged_tree_id != original.head_tree_id {
        return Err(
            "explicit child pin must not silently confirm changed parent boundaries".into(),
        );
    }
    browser.css("[data-commit-directory][disabled]")?;
    let after = mutate(
        browser,
        "[data-stage-directory-owned]",
        &format!(
            "/api/repositories/{}/directory-git/stage",
            parent.repository_id
        ),
        json!({"directory_id":pinned.directory_id,"directory_branch_id":pinned.directory_branch_id,
            "expected_tree_id":pinned.working_tree_id,"expected_index_generation":pinned.index_generation.to_string()}),
    )?;
    let reviewed = current(browser, Some(&parent.repository_id), "", after)?;
    if reviewed.needs_restage || child(&reviewed, "child")?.staged_tree_id != original.head_tree_id
    {
        return Err("owned restage must preserve the already reviewed direct child pin".into());
    }
    let committed = commit(browser, &reviewed)?;
    let reference = child(&committed, "child")?;
    if reference.head_tree_id != original.head_tree_id
        || reference.committed_tree_id != original.head_tree_id
        || !reference.dirty
        || child(&committed, "unborn")?.head_tree_id.is_some()
        || child(&committed, "unborn")?.committed_tree_id.is_some()
    {
        return Err(
            "parent commit changed a child's HEAD or implicitly committed an unborn child".into(),
        );
    }
    Ok(committed)
}
fn advance_child(browser: &Browser, parent: &DirectoryGitStatus) -> Result<()> {
    let after = read_count(browser)?;
    browser.click(&browser.element(
        "xpath",
        "//button[contains(@class,'zxs-file-name')][normalize-space(.)='ordinary']",
    )?)?;
    let ordinary = current(browser, Some(&parent.repository_id), "ordinary", after)?;
    if ordinary.initialized
        || ordinary.owner.as_ref().map(|owner| &owner.directory_id) != Some(&parent.directory_id)
    {
        return Err(
            "ordinary descendant must remain owned by its parent without auto-initializing".into(),
        );
    }
    browser.css("[data-initialize-directory-git]:not([disabled])")?;
    open_root(browser)?;
    let after = read_count(browser)?;
    browser.click(&browser.element(
        "xpath",
        "//button[contains(@class,'zxs-file-name')][normalize-space(.)='child']",
    )?)?;
    let current_child = current(browser, Some(&parent.repository_id), "child", after)?;
    if current_child.head_tree_id != child(parent, "child")?.head_tree_id || !current_child.unstaged
    {
        return Err("parent publication lost the child's uncommitted working content".into());
    }
    browser.css("[data-commit-directory][disabled]")?;
    let after = mutate(
        browser,
        "[data-stage-directory-owned]",
        &format!(
            "/api/repositories/{}/directory-git/stage",
            parent.repository_id
        ),
        json!({"directory_id":current_child.directory_id,"directory_branch_id":current_child.directory_branch_id,
            "expected_tree_id":current_child.working_tree_id,"expected_index_generation":current_child.index_generation.to_string()}),
    )?;
    let staged_child = current(browser, Some(&parent.repository_id), "child", after)?;
    if !staged_child.staged || staged_child.unstaged {
        return Err("explicit child staging did not freeze its working content".into());
    }
    let new_child = commit(browser, &staged_child)?;
    let after = read_count(browser)?;
    open_root(browser)?;
    let unchanged = current(browser, Some(&parent.repository_id), "", after)?;
    let reference = child(&unchanged, "child")?;
    if unchanged.staged
        || unchanged.head_tree_id != parent.head_tree_id
        || reference.dirty
        || !reference.reference_changed
        || reference.head_tree_id != new_child.head_tree_id
        || reference.committed_tree_id != current_child.head_tree_id
        || reference.staged_tree_id != current_child.head_tree_id
    {
        return Err("child commit must not automatically advance the parent's staged or committed reference".into());
    }
    let staged = stage_child(browser, &unchanged)?;
    let committed = commit(browser, &staged)?;
    if child(&committed, "child")?.committed_tree_id != new_child.head_tree_id
        || child(&committed, "unborn")?.head_tree_id.is_some()
    {
        return Err(
            "explicit parent reference update did not retain the independent unborn child".into(),
        );
    }
    Ok(())
}

pub(crate) fn run(browser: &Browser) -> Result<()> {
    browser.script(OBSERVE)?;
    with_cleanup(
        || {
            open_root(browser)?;
            let parent = current(browser, None, "", 0)?;
            if browser.script("return window.__zxsDirectoryGit.writes.length")? != 0 {
                return Err("reading child status must not publish".into());
            }
            let committed = review_boundaries(browser, &parent)?;
            advance_child(browser, &committed)?;
            let exact_writes = browser.script(
                "return window.__zxsDirectoryGit.writes.length===7&&window.__zxsDirectoryGit.unexpected.length===0",
            )?;
            if exact_writes != true {
                return Err("parent/child flow performed an implicit or duplicate write".into());
            }
            println!(
                "PASS real child pin before boundary restage, parent commit preserves dirty and unborn children, ordinary ancestor ownership, explicit child HEAD advancement and parent reference update, exact request fences and duplicate-click suppression"
            );
            Ok(())
        },
        || {
            browser.script("window.__zxsDirectoryGit.restore();return true")?;
            Ok(())
        },
    )
}
