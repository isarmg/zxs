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
        "zxs-client /absolute/private-state open|prepare /file [first DISPLAY_NAME]|dirty /file|bundle-diff /dir|bundle-pack /dir /new.zip\n\
         zxs-client HTTPS_ORIGIN ACCOUNT /absolute/private-state init|reconcile|list [after]|detail ID|checkout ID BRANCH /new/file|status OP|send OP".into()
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
        let password = Zeroizing::new(rpassword::prompt_password("ZXS 口令（不回显）：")?);
        let remote = Session::connect(origin, account, password).await?;
        if verb == "init" && rest.is_empty() {
            remote.catalog(None).await?; // Prove real authenticated read before enrolling one principal.
            Workspace::initialize(state, remote.principal())?;
            println!("已登记独立私用目录，不会自动提交。");
            return Ok(());
        }
        let mut state = Workspace::open(state, remote.principal())?;
        for prior in state.reconcile_on_open(&remote).await? {
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
                serde_json::to_string_pretty(&remote.catalog(None).await?)?
            ),
            ("list", [after]) => println!(
                "{}",
                serde_json::to_string_pretty(&remote.catalog(Some(&Id::parse(after)?)).await?)?
            ),
            ("detail", [key]) => println!(
                "{}",
                serde_json::to_string_pretty(&remote.detail(&Id::parse(key)?).await?)?
            ),
            ("checkout", [key, branch, file]) => {
                let basis = state
                    .checkout(
                        &remote,
                        &Id::parse(key)?,
                        &Branch::parse(branch)?,
                        Path::new(file),
                    )
                    .await?;
                println!("已核验并另存为新文件；原节点 {}", basis.node);
            }
            ("status", [op]) => {
                let result = state.lookup(&remote, &Id::parse(op)?).await?;
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
                    println!("{:?}", state.submit(&remote, &op, true).await?);
                }
            }
            _ => return Err(usage()),
        }
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
    if confirmation.trim_end_matches(['\n', '\r']) == "prepare" {
        if let Some(intent) = state.prepare(&review, true)? {
            println!("已封存，不上传。原操作 {}", intent.operation_id);
        }
    }
    Ok(())
}
