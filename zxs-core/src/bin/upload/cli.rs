use std::{
    io::{self, Write as _},
    path::Path,
    time::{Duration, Instant},
};
use zeroize::Zeroizing;
use zxs_core::{
    Result,
    r3::{Id, Lookup},
    workspace::{UploadObservation, Workspace},
};

struct Input<'a> {
    origin: &'a str,
    account: &'a str,
    directory: &'a Path,
    op: Id,
    send: bool,
}

fn parse(args: &[String]) -> Result<Input<'_>> {
    let [mode, origin, account, directory, verb, op] = args else {
        return Err(
            "use --upload-session HTTPS_ORIGIN ACCOUNT /private/state status|send OP".into(),
        );
    };
    let directory = Path::new(directory);
    if mode != "--upload-session"
        || !directory.is_absolute()
        || !matches!(verb.as_str(), "status" | "send")
    {
        return Err("select an exact registered workspace and one status or send action".into());
    }
    Ok(Input {
        origin,
        account,
        directory,
        op: Id::parse(op)?,
        send: verb == "send",
    })
}

pub(super) fn run(args: &[String]) -> Result<()> {
    let input = parse(args)?;
    let mut state = Workspace::open_local(input.directory)?;
    let choice = if input.send {
        state.choose_upload_for_send(&input.op)?
    } else {
        state.choose_upload(&input.op)?
    };
    if input.send {
        println!(
            "断点上传到 {}，账号 {}。原本机文件：{}。核对原意图：\n{}",
            input.origin,
            input.account,
            choice.path(),
            serde_json::to_string_pretty(choice.intent())?
        );
        print!(
            "按原号上传剩余分块并发布；输入 send {} 确认，其他输入取消：",
            input.op
        );
        io::stdout().flush()?;
        let mut answer = String::new();
        io::stdin().read_line(&mut answer)?;
        if answer.trim_end_matches(['\r', '\n']) != format!("send {}", input.op) {
            println!("已取消；未连接，也未发送。");
            return Ok(());
        }
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let ready = state
            .begin_existing_basic(input.origin, input.account)
            .await?;
        let password = Zeroizing::new(rpassword::prompt_password("断点上传会话口令（不回显）：")?);
        let basic = state.finish_existing_basic(ready, password)?;
        let result = if input.send {
            let mut last: Option<Instant> = None;
            state
                .send_upload(&basic, &choice, true, |n, length| {
                    if n == length
                        || last.is_none_or(|then| then.elapsed() >= Duration::from_secs(2))
                    {
                        eprintln!("服务端已确认分块 {n}/{length} 字节；尚非发布回执");
                        last = Some(Instant::now());
                    }
                })
                .await
        } else {
            state.upload_status(&basic, &choice).await
        };
        let result = result.map_err(|error| {
            format!(
                "原操作 {}：{error}；保留原件，先用同号 status 检查，不会自动补发。",
                input.op
            )
        })?;
        match result {
            UploadObservation::Receipt(r) => match r.status {
                Lookup::Confirmed { sequence } => {
                    println!("原操作 {} 已确认，版本序号 {sequence}。", r.operation);
                    if r.local_cleanup_pending {
                        println!("只待本机清理，不需重传。");
                    }
                }
                other => println!("原回执状态 {other:?}，未确认发布。"),
            },
            UploadObservation::NoProgress => {
                println!("原回执不存在，未查到上传进度；此查询不会上传。")
            }
            UploadObservation::Staged {
                next_offset,
                length,
            } => {
                println!("已暂存 {next_offset}/{length} 字节，未收到发布回执。")
            }
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::parse;

    #[test]
    fn rejects_writes_in_status_spelling_before_authentication() {
        let base: Vec<String> = [
            "--upload-session",
            "https://example.invalid",
            "user",
            "/state",
            "status",
            "11111111111111111111111111111111",
        ]
        .map(str::to_owned)
        .to_vec();
        assert!(!parse(&base).unwrap().send);
        let mut args = base.clone();
        args[4] = "send".into();
        assert!(parse(&args).unwrap().send);
        for (slot, bad) in [
            (0, "--receipt-session"),
            (3, "relative"),
            (4, "retry"),
            (5, "bad"),
        ] {
            let mut args = base.clone();
            args[slot] = bad.into();
            assert!(parse(&args).is_err());
        }
    }
}
