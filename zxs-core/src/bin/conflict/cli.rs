use std::path::Path;
use zxs_core::{
    Result,
    r3::{Id, Target},
    workspace::Workspace,
};

pub(super) fn run_if_selected(
    args: &[String],
    confirm: impl FnOnce(&str) -> Result<bool>,
) -> Result<bool> {
    let command = |arg: &str| matches!(arg, "archive-conflict" | "archived-conflicts");
    let unsupported =
        "use the offline form: zxs-client /private/state archive-conflict OP|archived-conflicts";
    if let [directory, verb, rest @ ..] = args
        && command(verb)
        && Path::new(directory).is_absolute()
    {
        match (verb.as_str(), rest) {
            ("archived-conflicts", []) => {
                let state = Workspace::open_local(Path::new(directory))?;
                for op in state.archived_conflicts()? {
                    println!("已归档 {op}；仍可按原号查询回执，不可重发。");
                }
            }
            ("archive-conflict", [op]) => archive(directory, &Id::parse(op)?, confirm)?,
            _ => return Err(unsupported.into()),
        }
        return Ok(true);
    }
    if args.get(3).is_some_and(|arg| command(arg)) {
        return Err(unsupported.into());
    }
    Ok(false)
}

fn archive(directory: &str, op: &Id, confirm: impl FnOnce(&str) -> Result<bool>) -> Result<()> {
    let mut state = Workspace::open_local(Path::new(directory))?;
    let choice = state.review_conflict_archive(op)?;
    let original = choice.intent().clone();
    println!(
        "只解除本机原号 {op} 的待决占位。保留普通文件及其原基线 {}，并保留原封存和回执证据。",
        choice.working_path()
    );
    println!("{}", serde_json::to_string_pretty(&original)?);
    println!("不会请求服务器；不表示远端取消。之后仍可按原号查回执，本应用不会重发它。");
    if !confirm(&format!("archive {op}"))? {
        println!("已取消，原冲突仍占位。");
        return Ok(());
    }
    state.archive_conflict(choice, true)?;
    println!("已归档原冲突 {op}，原工作文件未改动。");
    match original.target {
        Target::Branch { branch, .. } => println!(
            "下一笔须签出当前分支头到全新路径：zxs-client HTTPS地址 账号 私用目录 checkout {} {branch} /全新/文件。手动合并所需修改，再对新路径核对和提交。",
            original.container_id,
        ),
        Target::HistoricalMain { .. } => println!(
            "不会自动分叉。请选明要继续的现有分支，签出当前头至全新路径，手动合并后再核对。"
        ),
        Target::FirstMain { .. } => println!(
            "先查原回执与远端清单。若已有目标文件，签出当前头至新路径；若确定要新建，在新路径准备所选文件与明确的新名字。"
        ),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::run_if_selected;

    #[test]
    fn reject_archive_in_authenticated_command_slot_and_require_an_exact_offline_choice() {
        for args in [
            vec![
                "https://example.invalid",
                "selected",
                "/private",
                "archive-conflict",
                "op",
            ],
            vec!["/private", "archive-conflict"],
            vec!["/private", "archived-conflicts", "unexpected"],
            vec!["/private", "archive-conflict", "not-an-original"],
        ] {
            let args: Vec<_> = args.into_iter().map(str::to_owned).collect();
            assert!(run_if_selected(&args, |_| panic!("must fail before confirmation")).is_err());
        }
    }
}
