# zxs-core

统一 Rust 客户端核心。首条入口是 Linux 的 `zxs-client`：浏览远端，把单个文件另存到普通本机目录，用自己的编辑器修改，再明确提交。无需工作树挂载、本机 `.zxs` 版本库或 `STORE`；现有客户端和旧数据不动。

**当前范围：** [实机已验] 普通用户 ext4 上的离线 ZIP 与文件安全用例。[仅静态] 尚未验收独立安装后的真实 HTTPS/2 提交、丢回执重开或正式发布。证据与待验项见[任务清单](TASKS.md)。

## 5 分钟上手

以下是 Linux 源码候选的操作路径。准备符合条件的本机 ext4/XFS、一个单独新建的空 0700 目录，以及拥有系统受信证书、实际可用 HTTP/2 的 ZXS R3 入口。口令在终端隐藏输入。

在 `zxs-core/` 用[锁定工具链](docs/guides/locked-build.md)构建：

```bash
cargo install --locked --path .
```

1. 用 `zxs-client HTTPS地址 账号 /绝对/私用目录 init` 登记。之后用同一前缀的 `list` 浏览，或用 `detail 容器ID` 看分支。
2. 用同一前缀的 `checkout 容器ID main /绝对/本地新文件` 另存当前主分支头。照常编辑文件；编辑器保存不会触发服务器操作。
3. 用 `zxs-client /绝对/私用目录 prepare /绝对/本地文件` 核对并在本机封存。首版另加 `first 显示名`；内容未变不生成操作。
4. 用带 HTTPS 的前缀执行 `send 操作ID`，再输入提示中的确认词。先查原回执，只有原号明确不存在才可以发送同一全文。

离线 `open` 查看待决，不连网。重新明确认证后的 `reconcile` 自动按原号查回执，不自动重发。原基线更新不会覆盖你后来保存的内容。

## 本机代码包

`bundle-diff /绝对/目录` 与 `bundle-pack /绝对/目录 /绝对/新文件.zip` 都用离线前缀。支持根 `.zxsignore`；当前只比对和生成标准 Deflate ZIP，不解包或提交 ZIP。

[架构与源码](ARCHITECTURE.md) · [ZIP32 规格](docs/specs/zip32-bundle.md) · [本机回执边界](docs/invariants/receipt-recovery.md) · [复现 ext4 检查](docs/guides/linux-ext4-qualification.md)。
