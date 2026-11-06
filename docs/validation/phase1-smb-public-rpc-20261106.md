# 一期 SMB 保存验收的运行边界

## 来源与分层

生产基线为 `2d983a9f19dc7c2ff2a4a623ada05feac37c6176`，本轮没有修改生产代码。
复用同一准确默认 Engine、`default/core` Server 和 smb-protocol 运行库，没有重新
构建 Engine、Server 或 Samba SDK。

云侧 `5dd8ba3c401b8e5f60a9bd2c52771badbd90bcb2` 增加三个私有 unit 场景：
读取源文件后新增独立目标、空文件 Close/Flush、同名与 Unicode 等价名称独占创建
冲突。该提交本身只记录格式/接线验证，不能据此称为原生运行或真实 SMB 通过。

新增独立目标 `phase1_smb_rpc` 适配这些场景，通过公开的
`storage_server::smb_rpc::serve` 和 `smb_protocol::client::Client` 使用真实路径型
Unix socket、进程 UID 准入及描述符传递。它没有复制生产 RPC 分派实现，也没有
使用模拟 Backend。与原私有 unit 模块是不同的测试目标，不能互相替代运行记录。

预期验证包括：

- 默认共享按真实一级盘根名 `p` 枚举，空浏览不创建仓库；首次写入使用一份正文的
  内部根命名空间。
- 正常创建、Flush/Close、显式 truncate 后保存较短正文、通过 Client 读回完整
  字节。普通“读源再新建目标”生成独立文件、节点及分支身份，不复制源标签、备注
  或历史；源旧版本及标签在冷重开后保留。
- 未写正文的空文件分别由 Close、Flush 后 Close 发布且不重复生成版本。
- 同名、大小写等价及 NFC/NFD 等价路径的独占创建返回准确冲突，原正文、暂存目录
  和连接保持可用，冷重开后原版本仍在。

上述均属于本机桥接验收，不包含 Samba VFS ABI、SMB 网络认证或真实客户端行为。

## 本轮实际结果

- 新目标 Rust 编译通过，使用 `-D warnings`，约 3.48 秒，峰值 RSS 733940 KiB。
- 测试进程实际运行三项，三项都在 `Client::connect_default` 内的 Unix 连接边界
  收到原始 `EPERM`，保存与恢复业务断言尚未进入。不能将此结果报告为业务通过，
  也没有据此修改保存实现。
- 对同一二进制申请了正式工具级的限范围执行授权，只允许临时目录中的本机 socket，
  不访问外部网络或真实用户数据。工具允许执行后，三项仍在同一边界返回 `EPERM`。
  已停止重试，没有改系统安全配置，没有用 socketpair 或假连接绕过限制。
- 聚焦 metadata-only Clippy `-D warnings -D clippy::all` 通过，约 0.17 秒；
  rustfmt 与 `git diff --check` 通过。编译和两次测试日志、准确库摘要与命令均保留。

本机桥接正式入口：

```sh
cargo test --locked --offline -p storage-server --test phase1_smb_rpc -- --test-threads=1
```

原 `5dd8` 私有场景仍应单独运行：

```sh
cargo test --locked --offline -p storage-server --lib phase1_primary_ -- --test-threads=1
```

本轮没有执行这两个 Cargo 命令。新目标只通过 direct-rustc 复用了准确生产运行库；
原私有模块的运行状态应以其正式 CI 记录为准。

## 真正 SMB 端到端还需要什么

当前环境未找到 `smbd`、`smbclient`、`smbpasswd`，也没有配置固定 Samba SDK。
Unix socket 边界本身还被执行环境拒绝，因此不能在此环境宣称 SMB 保存验收完成。

仓库已有 `.github/workflows/smb-sdk.yml` 正式路径：

1. Debian 13 的准确 `2:4.22.11+dfsg-0+deb13u1` 源包/系统包、VFS interface 50，
   以及同一源码提交构建的 `zxs.so`。不同 Samba 版本或仅存在协议 rlib 不能替代。
2. 在一次性、网络隔离且允许内部 loopback 的 runtime 容器中运行真实 apt smbd 和
   smbclient；固定配置由 `.github/scripts/check-system-samba.py` 核验。
3. 保留两个不同的合成非 root 账户，真实验证认证、VFS UID 切换、私有目录 0700、
   socket 0600 及可信进程凭据；不修改宿主账号或放宽权限。
4. 取得同一最终候选的 `system-samba.json` 和对应 VFS/运行包摘要，核对默认根
   put/get/rename、原文件读取、删除、错误凭据、guest 拒绝和运行中撤权结果。

现有工作流具备这些步骤不等于当前候选已经通过。即使正式 runtime 门禁通过，也
不自动证明用户机器的 systemd、真实物理盘或所有编辑器/客户端组合完成验收。
