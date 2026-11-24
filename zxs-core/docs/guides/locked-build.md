# 独立锁定与资格

- [仅静态] 初始锁见 [`24e38d1`](https://github.com/isarmg/zxs/blob/24e38d1bd42fde3d95b69afa29cea4afc20ccc0d/zxs-core/Cargo.lock)。Dot 隔离目录用 Rust 1.100.0（编译器提交 `0ef1bc386059596dada8fb65b50425aab2ea637a`）从 crates.io 解析 108 个依赖包。历史锁 SHA-256：`5f646a89312e230181634fc82ec7e5ce82f2b42db134f49b19e9c9c56f53079a`。当时核对了 Ring（无 AWS-LC）、miniz_oxide 0.9.0 和 bundled SQLite，未安装产品。
- [CI通过] [`b2958a6` Linux 作业](https://github.com/isarmg/zxs/actions/runs/45356345062/job/138315006700)验证了锁定的 `rpassword 7.5.0`（[上游修复公告](https://github.com/conradkleinespel/rpassword/security/advisories/GHSA-2p6r-x3vv-xqm2)）、格式、全目标严格 Clippy、20 个常规测试与 9 个隔离 ext4 用例。新锁 SHA-256：`0f556d1419f7443de085ea92d5642f1c9b59674b80714cc2513b85fe4596393b`；官方包校验和见[该提交的锁](https://github.com/isarmg/zxs/blob/b2958a6d1fbcdb6d61c31a79fa1c648e6b0fdbe5/zxs-core/Cargo.lock)。

这两条是各自版本的证据，不是独立安装或真实 HTTPS/2 验收结论。[手动比较方案](installed-h2-qualification.md)保留单独的历史版本。

1. 在批准的 Dot 云或隔离 CI，把**仅** `zxs-core/` 的确切待审源码取到独立目录。使用仓库已批准的 Rust 1.100 工具链，普通干净环境，不注入 `LIBSQLITE3_SYS_USE_PKG_CONFIG`、`LIBSQLITE3_FLAGS`、`SSL_CERT_FILE`、`SSL_CERT_DIR` 或旁系项目。
2. 以标准 Cargo 注册源解析一次锁；记录工具版本和完整 `Cargo.lock`，核对注册源、校验和、所有传递版本和实际终端目标。审查 `cargo metadata --locked` 与 `cargo tree --locked -e features`。不允许 TLS 引入第二个加密提供者，或 Deflate 无意换后端。该锁和树与源码一起由独立审核方确认。
3. 发布及任何独立安装声明均要求 `--locked`。有完整独立包后再用 [任务清单](../../TASKS.md) 验收；本文件不授权触发 CI，不设置发布路径。缺锁时只称源码候选。
4. 上游依据：[h2](https://docs.rs/crate/h2/0.4.19)，[rustls](https://docs.rs/crate/rustls/0.23.46)，[tokio-rustls](https://docs.rs/crate/tokio-rustls/0.26.5)，[flate2](https://docs.rs/crate/flate2/1.1.10/features)，[SQLite 绑定](https://docs.rs/crate/rusqlite/0.37.0)，[ZIP APPNOTE](https://pkware.cachefly.net/webdocs/casestudies/APPNOTE.TXT)。这不是实际构建库存。后续从准确的解析版本收集全部许可、第三方通知及 Ring 的随附材料，SQLite 文件保持仅位于客户端，不引入服务端依赖。
