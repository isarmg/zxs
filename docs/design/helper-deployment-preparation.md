# 辅助工具：未激活的 systemd / SMB 部署配置包

本片源码接在独立 Web 与内置程序首装后，提供真实预览、摘要确认和可持久化配置包。**未构建、未运行测试、未审查、未运行 CI，未操作真实系统。它不是已安装或已启用的 systemd/Samba 配置，也不是一条龙安装完成。**

## 入口与范围

Web 的“准备 systemd / SMB 配置”和 CLI `deployment preview` / `deployment prepare` 使用同一实现。仅手工 root helper，无登录，默认 `0.0.0.0:9040`，helper 本身不注册服务、不自启。输入包括 ZXS 服务账户/组、HTTP 监听及可选的 Samba 独立认证账户/可信连接 UID。账户只允许明确的窄 Unix 名称；不能借换行、空格、`%`、`$` 或占位符注入 systemd/Samba 指令。服务账户与 Samba 认证账户必须不同，不接受名字 `root`，但**没有证明这些名字实际存在或其 UID 非零**，激活前必须另核。

生成的 ZXS unit 使用固定 `/usr/local/bin/zxs`、`/zxs/data/state`、`/zxs/data/management`、`/zxs/data/disks` 及 `/zxs` 内明确的私有运行目录。与程序首装目的地一致，不改变现有 `deploy/` 样例。Samba 配置绑定同一 socket `/zxs/run/smb.sock`、准确账户、独立只读空导出目录 `/zxs/smb-export` 与 VFS `zxs`，保留 SMB3、签名、加密、禁 guest 等约束。可信 UID 0 不是自动探测结果，只有已核实 Samba 在可信 root 身份下连接时才可采用。

可用 CLI 源码接口：

```text
zxs-helper deployment preview --service-user zxs --service-group zxs \
  --listen 0.0.0.0:7400 --smb-account zxs-owner --smb-peer-uid 0
zxs-helper deployment prepare --service-user zxs --service-group zxs \
  --listen 0.0.0.0:7400 --smb-account zxs-owner --smb-peer-uid 0 \
  --request-digest <准确预览摘要> --confirm-prepared-only
```

省略全部 SMB 参数只准备 Web 主服务 unit，不编造 SMB 账户。所有操作要求正式 helper 内置载荷有效，开发构建不会下载替代品。预览绑定当前 payload 的版本、Git 来源、ELF 目标和 SHA-256，并展示完整 unit/SMB 字节及各自 SHA-256。计划整体 SHA-256 同时绑定已存在 `/zxs` 的设备和 inode，不能换目录后复用旧确认。

## 原子发布与可恢复范围

固定父目录 `/zxs` 必须已存在、当前 root 所有且其他用户/组不可写。它不会被自动创建、chown 或放宽权限。持有目录 fd 和同目录非阻塞排他锁，所有目标文件相对该 fd 操作，并在完成点重核父路径身份。后继[生命周期互斥](helper-service-exclusion.md)使当前 ZXS 可执行程序也必须持有同一 inode 锁，故准备期间能排斥该当前 daemon；**不是 Samba、旧二进制或任意外部写者的停服证明**。

确认 `prepare-inactive-configuration` 后，将 `manifest.json`、`zxs.service` 及可选 `smb.conf` 写入 `0700` 的 `.zxs-helper-deployment-<摘要>.pending/`，文件为 `0600`、普通单链接。所有字节同步，目录同步后用 `RENAME_NOREPLACE` 整目录发布为 `.zxs-helper-deployment-<摘要>/`，再同步父目录。目标名不被系统服务自动读取；不写 `/etc`，没有 reload、start、enable、账户命令或密码提交。

- 准确既有完整目标：逐项核验字节、类型、权限及身份，重新同步并返回 `matching_inactive_bundle_durably_verified`，不冒充刚创建。
- 只有准确 pending、部分所需文件已完整发布：同一原请求可补齐缺项、同步并完成目录发布，返回 `interrupted_inactive_bundle_completed`。
- 不同正文、权限、链接、未知文件、目标与 pending 同时存在：拒绝并保留全部证据，不清空或替换。文件原子发布留下的未决临时文件也须显式调查，不能自动猜测删除；**不是任意中断都能自动完成**。
- rename 或 fsync 结果不确定：沿用 `Unknown` / `Published` / `PublishedDurabilityUnknown` 的错误来源及原父身份、staging 和目标；不假称回滚。浏览器断开不会释放工作 owner，重新连接可用准确输入重做预览/同一确认，遇证据冲突停止。

UI 输入更改立即取消旧预览与确认；确认请求携带冻结输入和摘要。HTTP 沿用独立控制台同源/CSRF/Host、请求预算、单任务持有者和退出排空。没有密码输入、秘密、动态 shell、命令执行、运行时下载或任意浏览器目标路径。

## 生效前仍未实现的条件

实际 systemd/Samba 安装、外部语法校验、依赖/VFS 安装、Unix/Samba 账户与 passdb、私有目录准备、首次系统/磁盘初始化、完整交接与旧版/第三方进程与 Samba 生命周期互斥均未完成。当前 ZXS 二进制的共用租约已有后继源码，但尚未构建/运行。不得将 `systemctl is-active` 的瞬时状态或“已停服”复选框当并发安全证明。后续真正替换现用配置前须有服务租约、原配置备份与持久提交/恢复，并显式决定是否加载、启动或启用开机服务。本片没有尝试这些动作，因此也不需要伪造已停服的事实。

部署包不是业务权威或运行准备完成标记。主服务不能消费它作为已初始化、已准入或已恢复的系统。包中的 `required_before_activation` 和结果中的 `live_configuration_modified=false`、`service_started_or_enabled=false`、`system_initialized=false` 必须保持可见。
