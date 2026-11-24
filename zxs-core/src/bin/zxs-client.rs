#[cfg(not(target_os = "linux"))]
fn main() {
    eprintln!("This narrow standalone CLI is admitted only on Linux.");
    std::process::exit(2);
}

#[cfg(target_os = "linux")]
fn main() {
    if let Err(error) = run() {
        eprintln!("{error}");
        std::process::exit(1);
    }
}

#[cfg(target_os = "linux")]
fn run() -> zxs_core::Result<()> {
    use std::{
        env,
        io::{self, Write},
        path::Path,
    };
    use zeroize::Zeroizing;
    use zxs_core::{
        Result, bundle,
        r3::{Branch, DisplayName, Id},
        session::Session,
        workspace::Workspace,
    };

    fn usage() -> Box<dyn std::error::Error + Send + Sync> {
        "zxs-client /absolute/private-state open|prepare /file [first DISPLAY_NAME]|cancel-prepared OP|dirty /file|bundle-diff /dir|bundle-pack /dir /new.zip\n\
         zxs-client HTTPS_ORIGIN ACCOUNT /absolute/private-state init|reconcile|list [after]|detail ID|checkout ID BRANCH /new/file|status OP|send OP\n\
         zxs-client --receipt-session HTTPS_ORIGIN ACCOUNT /absolute/private-state reconcile|status OP\n\
         zxs-client --payload-session HTTPS_ORIGIN ACCOUNT /absolute/private-state slice /mapped/file START LENGTH /new/output\n\
         zxs-client --upload-session HTTPS_ORIGIN ACCOUNT /absolute/private-state status|send OP".into()
    }

    fn ask(word: &str) -> Result<bool> {
        print!("输入 {word} 确认；其他内容取消：");
        io::stdout().flush()?;
        let mut answer = String::new();
        io::stdin().read_line(&mut answer)?;
        Ok(answer.trim_end_matches(['\n', '\r']) == word)
    }

    let args: Vec<String> = env::args().skip(1).collect();
    if args.first().is_some_and(|arg| arg == "--help") {
        println!("{}", usage());
        return Ok(());
    }
    if args.first().is_some_and(|arg| arg == "--receipt-session") {
        return run_receipts(&args);
    }
    if args.first().is_some_and(|arg| arg == "--upload-session") {
        return upload::run(&args);
    }
    if args.first().is_some_and(|arg| arg == "--payload-session") {
        return run_payload(&args);
    }
    if let Some((directory, op)) = cancel_prepared_args(&args)? {
        // This branch does not touch other receipts or open any network session.
        let state = Workspace::open_local(directory)?;
        let choice = state.review_prepared_cancellation(&op)?;
        println!(
            "只丢弃尚未尝试的本机封存 {op}：{} 字节 SHA-256 {}。\n保留普通编辑文件 {} 和确认基线，不请求服务器。",
            choice.intent().body.length,
            choice.intent().body.sha256,
            choice.working_path()
        );
        if ask(&format!("cancel {op}"))? {
            state.cancel_prepared(choice, true)?;
            println!("已取消本机封存 {op}；未发送。");
        }
        return Ok(());
    }
    if let [state, verb, rest @ ..] = args.as_slice()
        && matches!(
            verb.as_str(),
            "open" | "dirty" | "prepare" | "bundle-diff" | "bundle-pack"
        )
    {
        // Completely offline: no auth prompt, DNS, TLS or operation request.
        let mut state = Workspace::open_local(Path::new(state))?;
        for prior in state.cleanup_on_open()? {
            println!(
                "原操作 {}: {:?}{}",
                prior.operation,
                prior.status,
                if prior.local_cleanup_pending {
                    "（只待本地清理）"
                } else {
                    ""
                }
            );
        }
        match (verb.as_str(), rest) {
            ("open", []) => {
                for operation in state.pending()? {
                    println!("保留的原操作 {operation}，重新连同一账号时只查回执。");
                }
            }
            ("dirty", [file]) => {
                let review = state.review(Path::new(file), None)?;
                println!(
                    "{}",
                    if review.changed() {
                        "本机仍有未提交修改"
                    } else {
                        "与已确认基线完全相同"
                    }
                );
            }
            ("prepare", [file]) => local_review(&mut state, Path::new(file), None)?,
            ("prepare", [file, flag, name]) if flag == "first" => {
                local_review(&mut state, Path::new(file), Some(DisplayName::parse(name)?))?
            }
            ("bundle-diff", [dir]) => {
                let review = state.review_bundle(Path::new(dir))?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&state.diff_bundle(&review)?)?
                );
            }
            ("bundle-pack", [dir, name]) => {
                let review = state.review_bundle(Path::new(dir))?;
                let name = Path::new(name);
                let destination = bundle::Destination::new(&review, name)?;
                println!(
                    "{}",
                    serde_json::to_string_pretty(&state.diff_bundle(&review)?)?
                );
                println!("仅生成普通本机 ZIP：{name:?}；不提交服务器。");
                if ask("pack")? {
                    let body = state.pack_bundle(&review, &destination, true)?;
                    println!("已核验 ZIP: {} 字节 SHA-256 {}", body.length, body.sha256);
                }
            }
            _ => return Err(usage()),
        }
        return Ok(());
    }
    let [origin, account, state, verb, rest @ ..] = args.as_slice() else {
        return Err(usage());
    };
    let state = Path::new(state);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        if verb == "init" && rest.is_empty() {
            let password = Zeroizing::new(rpassword::prompt_password("ZXS 口令（不回显）：")?);
            let remote = Session::connect(origin, account, password).await?;
            remote.catalog(None).await?; // Prove real authenticated read before enrolling one principal.
            Workspace::initialize(state, remote.principal())?;
            println!("已登记独立私用目录，不会自动提交。");
            return Ok(());
        }
        let mut state = Workspace::open_local(state)?;
        let ready = state.begin_existing_basic(origin, account).await?;
        let password = Zeroizing::new(rpassword::prompt_password("ZXS 口令（不回显）：")?);
        let remote = state.finish_existing_basic(ready, password)?;
        for prior in state.reconcile_existing(&remote).await? {
            println!(
                "原操作 {}: {:?}{}",
                prior.operation,
                prior.status,
                if prior.local_cleanup_pending {
                    "（只待本地清理）"
                } else {
                    ""
                }
            );
        }
        match (verb.as_str(), rest) {
            ("reconcile", []) => (),
            ("list", []) => println!(
                "{}",
                serde_json::to_string_pretty(&state.catalog_existing(&remote, None).await?)?
            ),
            ("list", [after]) => println!(
                "{}",
                serde_json::to_string_pretty(
                    &state
                        .catalog_existing(&remote, Some(&Id::parse(after)?))
                        .await?
                )?
            ),
            ("detail", [key]) => println!(
                "{}",
                serde_json::to_string_pretty(
                    &state.detail_existing(&remote, &Id::parse(key)?).await?
                )?
            ),
            ("checkout", [key, branch, file]) => {
                let basis = state
                    .checkout_existing(
                        &remote,
                        &Id::parse(key)?,
                        &Branch::parse(branch)?,
                        Path::new(file),
                    )
                    .await?;
                println!("已核验并另存为新文件；原节点 {}", basis.node);
            }
            ("status", [op]) => {
                let result = state.lookup_existing(&remote, &Id::parse(op)?).await?;
                println!("{:?}", result);
            }
            ("send", [op]) => {
                let op = Id::parse(op)?;
                let intent = state.intent(&op)?;
                println!(
                    "服务器 {}，账号 {}；原意图如下。仅服务器明确无原回执才会发送同一笔。",
                    remote.principal().origin(),
                    remote.principal().account()
                );
                println!("{}", serde_json::to_string_pretty(&intent)?);
                if ask(&format!("send {op}"))? {
                    println!("{:?}", state.submit_existing(&remote, &op, true).await?);
                }
            }
            _ => return Err(usage()),
        }
        Ok(())
    })
}

#[cfg(target_os = "linux")]
#[path = "upload/cli.rs"]
mod upload;

#[cfg(target_os = "linux")]
fn cancel_prepared_args(
    args: &[String],
) -> zxs_core::Result<Option<(&std::path::Path, zxs_core::r3::Id)>> {
    use std::path::Path;
    use zxs_core::r3::Id;

    let bad = || "use the offline form: zxs-client /absolute/private-state cancel-prepared OP";
    if let [directory, verb, rest @ ..] = args
        && verb == "cancel-prepared"
        && Path::new(directory).is_absolute()
    {
        let [raw] = rest else {
            return Err(bad().into());
        };
        return Ok(Some((Path::new(directory), Id::parse(raw)?)));
    }
    // Deny this verb in the fourth (online) slot before the CLI can authenticate.
    // The second slot of a valid online invocation is the account, not a verb.
    if args.get(3).is_some_and(|arg| arg == "cancel-prepared") {
        return Err(bad().into());
    }
    Ok(None)
}

#[cfg(target_os = "linux")]
enum ReceiptAction {
    Reconcile,
    Status(zxs_core::r3::Id),
}

#[cfg(target_os = "linux")]
fn receipt_args(args: &[String]) -> zxs_core::Result<(&str, &str, &str, ReceiptAction)> {
    use zxs_core::r3::Id;
    let unsupported =
        || "receipt-only mode supports reconcile or status OP, not legacy reads or sends";
    let [mode, origin, account, state, verb, rest @ ..] = args else {
        return Err(unsupported().into());
    };
    if mode != "--receipt-session" {
        return Err(unsupported().into());
    }
    let action = match (verb.as_str(), rest) {
        ("reconcile", []) => ReceiptAction::Reconcile,
        ("status", [op]) => ReceiptAction::Status(Id::parse(op)?),
        _ => return Err(unsupported().into()),
    };
    Ok((origin, account, state, action))
}

#[cfg(target_os = "linux")]
fn run_receipts(args: &[String]) -> zxs_core::Result<()> {
    use std::path::Path;
    use zeroize::Zeroizing;
    use zxs_core::workspace::Workspace;
    // Reject all out-of-scope verbs and unknown original IDs before a prompt,
    // DNS or TLS. Enrollment and file upload are not part of this mode.
    let (origin, account, state, action) = receipt_args(args)?;
    let mut state = Workspace::open_local(Path::new(state))?;
    if let ReceiptAction::Status(op) = &action {
        state.intent(op)?;
    }
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let receipt = state
            .connect_receipts(origin, account, || {
                Ok(Zeroizing::new(rpassword::prompt_password(
                    "ZXS 回执会话口令（不回显）：",
                )?))
            })
            .await?;
        match action {
            ReceiptAction::Reconcile => {
                for original in state.reconcile_receipts(&receipt).await? {
                    println!("{original:?}");
                }
            }
            ReceiptAction::Status(op) => {
                println!("{:?}", state.receipt_status(&receipt, &op).await?)
            }
        }
        Ok(())
    })
}

#[cfg(target_os = "linux")]
struct PayloadArgs<'a> {
    origin: &'a str,
    account: &'a str,
    state: &'a str,
    mapped: &'a str,
    window: zxs_core::session::ByteWindow,
    output: &'a str,
}

#[cfg(target_os = "linux")]
fn payload_args(args: &[String]) -> zxs_core::Result<PayloadArgs<'_>> {
    use std::path::Path;
    use zxs_core::session::{ByteWindow, PayloadError};
    let [
        mode,
        origin,
        account,
        state,
        verb,
        mapped,
        start,
        length,
        output,
    ] = args
    else {
        return Err(
            "payload-only mode supports one slice of an already-bound workspace file".into(),
        );
    };
    if mode != "--payload-session" || verb != "slice" {
        return Err(
            "payload-only mode supports slice, not receipts, raw URLs or legacy writes".into(),
        );
    }
    if [state, mapped, output]
        .iter()
        .any(|p| !Path::new(p).is_absolute())
    {
        return Err(
            "slice selection requires exact absolute local paths, not an endpoint URL".into(),
        );
    }
    let decimal = |s: &str| -> zxs_core::Result<u64> {
        if s.is_empty() || !s.bytes().all(|n| n.is_ascii_digit()) {
            return Err(PayloadError::InvalidWindow.into());
        }
        s.parse().map_err(|_| PayloadError::InvalidWindow.into())
    };
    Ok(PayloadArgs {
        origin,
        account,
        state,
        mapped,
        window: ByteWindow::new(decimal(start)?, decimal(length)?)?,
        output,
    })
}

#[cfg(target_os = "linux")]
fn run_payload(args: &[String]) -> zxs_core::Result<()> {
    use std::path::Path;
    use zeroize::Zeroizing;
    use zxs_core::workspace::PayloadWorkspace;
    let command = payload_args(args)?;
    let state = PayloadWorkspace::open_local(Path::new(command.state))?;
    let selected = state.choose_payload_slice(
        Path::new(command.mapped),
        command.window,
        Path::new(command.output),
    )?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let payload = state
            .connect_payload(&selected, command.origin, command.account, || {
                Ok(Zeroizing::new(rpassword::prompt_password(
                    "ZXS 正文区间会话口令（不回显）：",
                )?))
            })
            .await?;
        let saved = state.save_payload_slice(&payload, &selected).await?;
        println!(
            "已另存所选节点区间：{} 字节，片段 SHA-256 {}。片段摘要不是整文件校验。",
            saved.length, saved.sha256,
        );
        Ok(())
    })
}

#[cfg(target_os = "linux")]
fn local_review(
    state: &mut zxs_core::workspace::Workspace,
    path: &std::path::Path,
    name: Option<zxs_core::r3::DisplayName>,
) -> zxs_core::Result<()> {
    use std::io::{self, Write};
    let review = state.review(path, name)?;
    println!(
        "原本机文件 {:?}: {} 字节 SHA-256 {}",
        review.path(),
        review.body().length,
        review.body().sha256
    );
    if !review.changed() {
        println!("核对时内容与原节点相同，没有新意图。");
        return Ok(());
    }
    if let Some(basis) = review.basis() {
        println!(
            "追加到原容器 {} 分支 {}，原节点 {}",
            basis.container, basis.branch, basis.node
        );
    } else if let Some(name) = review.name() {
        println!("准备 {} 的 main 首版。", name.as_str());
    }
    print!("输入 prepare 只封存原号，不上传；其他内容取消：");
    io::stdout().flush()?;
    let mut confirmation = String::new();
    io::stdin().read_line(&mut confirmation)?;
    if confirmation.trim_end_matches(['\n', '\r']) == "prepare"
        && let Some(intent) = state.prepare(&review, true)?
    {
        println!("已封存，不上传。原操作 {}", intent.operation_id);
    }
    Ok(())
}

#[cfg(all(test, target_os = "linux"))]
#[path = "../../tests/unit/receipt_cli_cases.rs"]
mod receipt_tests;

#[cfg(all(test, target_os = "linux"))]
#[path = "../../tests/unit/payload_cli_cases.rs"]
mod payload_tests;

#[cfg(all(test, target_os = "linux"))]
#[path = "../../tests/unit/cancel_cli_cases.rs"]
mod cancel_tests;
