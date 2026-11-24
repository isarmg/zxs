# 独立 Linux ext4 离线资格

用全新的合成样品复现本机保护与 ZIP 互操作。没有服务器请求，不使用自己的数据或系统盘目录，不安装产品。这不是 XFS、正式安装或完整 HTTPS 的资格。

## 环境和命令

仅带 `zxs-core/` 源码到隔离 Linux，使用 Rust 1.100.0 和源包的 `Cargo.lock`。先按[依赖核对](locked-build.md)备齐锁内依赖及该工具链的 rustfmt/Clippy。下面禁止 Cargo 联网解析；夹具只走本机。需 `findmnt`、Info-ZIP `unzip` 与 Python 3。

在本人所有的本机 ext4 草稿父目录里创建**新**的私用子目录。不要提权，实际有效能力须为零；用完只清理这一份新草稿。

```bash
umask 077
scratch_parent='/绝对/另选的ext4草稿父目录'
export ZXS_CORE_NATIVE_TEST_ROOT
ZXS_CORE_NATIVE_TEST_ROOT=$(mktemp -d -- "$scratch_parent/zxs-core-native.XXXXXXXX")
findmnt -rno FSTYPE -T "$ZXS_CORE_NATIVE_TEST_ROOT"
id -u
grep '^CapEff:' /proc/self/status
cargo fmt --all -- --check
cargo clippy --locked --offline --all-targets -- -D warnings
cargo test --locked --offline
cargo test --locked --offline --lib native::tests:: -- --ignored --nocapture
cargo test --locked --offline --lib workspace::tests::detached_lease_refuses_local_and_wire_custody -- --ignored --nocapture
cargo test --locked --offline --test linux_zip deterministic_native_zip -- --ignored --nocapture
```

标准测试命令只运行纯内存项。显式启用的本机用例拒绝未正确选择的草稿、非 ext4、提权和复用测试子目录；缺条件会失败，不当作通过。

## 本次记录

[实机已验] Dot 临时 ext4，UID 1000，`CapEff=0`；锁定格式检查、严格 Clippy、4 个纯内存及 5 个本机用例通过。这是本机复现，不是 GitHub Actions 托管运行，也未连接真实服务。

- [租约与末次插入](../../src/native_tests.rs)：合法普通文件保留实际锁竞争；FIFO、目录、链接和错误模式不能取得读写句柄。末次不覆盖插入保留原 inode 和字节；原租约在同一边界换件也不发布。
- [工作区原号](../../src/workspace_tests.rs)：晚换租约后，原操作保持现有阶段，不登记新正文、不推进基线，最终发送前核验拒绝。未发真实 HTTP 请求。
- [离线 CLI](../../tests/linux_zip.rs)与[独立读取](../../tests/zip_oracle.py)：标准 ZIP32/Deflate、CRC、字节和固定元数据，含空文件、空目录与 UTF-8 名。晚占名和确认前二次改动都不发布替件。
- 两份实际 ZIP 均 625 B，SHA-256 为 `6f24562e993621528ce86967a40fa3ef4848d09332a37c7531818e4b4dcc9642`。没有验证异平台解包。

[CI通过] [Ubuntu 24.04 托管复现](https://github.com/isarmg/zxs/actions/runs/45335009882/job/138244922987)同样通过 4 个纯内存和 5 个离线 ext4 用例，独立工具读回同一 ZIP 长度和摘要。

这些证据不替代[真实安装和丢回执恢复](../../TASKS.md#还要证明什么)。
