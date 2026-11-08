# 首次初始化前置：准确服务身份与私有数据目录

本片为首次初始化提供实际的本地身份核验和空目录发布，不是管理员/数据库/磁盘初始化完成。源码接在生命周期租约后，未构建、测试、审查或运行 CI，也未对真实主机执行操作。

## 可用源码入口

独立 Web 的“首次初始化前：服务私有目录”，以及：

```text
zxs-helper initialization layout-preview --service-user zxs
zxs-helper initialization layout-prepare --service-user zxs \
  --request-digest <准确预览摘要> --confirm-private-layout-only
```

工具仍由 root 手工运行、无登录、默认 `0.0.0.0:9040`。服务账户必须已经存在；本片不会创建、删除或修改 Unix 账户，也不触碰管理员密码或 Samba passdb。其余 root/无认证网络暴露警示保持。

## 身份边界

账户名与部署模板复用同一窄类型，禁止换行、空格、占位符和非预期字符。仅解析 root 所有、其他用户/组不可写、普通单链接的 `/etc/passwd`，使用目录 fd、拒绝链接、完整长度上限 4 MiB；不调用 NSS、域身份网络服务或 shell。不返回或记录 passwd 密码字段。

要求准确本地名称只出现一次、UID 和主 GID 非零，且该 UID 不被其他本地账户复用。不存在账户、异常本地记录、root UID/GID 或身份源变化均拒绝，不选择猜测的 UID。预览绑定账户 UID/GID、passwd 文件设备/inode/摘要及固定 `/zxs` 父身份；执行前、目录交接前、完成后重新核对身份源，变化时停止并保留已产生证据。仍须防止外部 root 管理员在操作中修改账户或重绑挂载，这些管理员不受 helper 的目录租约强制约束。

## 只发布空的固定布局

持续取得与当前 ZXS 可执行程序相同的 `/zxs` 生命周期租约，才检查和准备固定 `/zxs/data`，不接受浏览器任意目标路径。新树只含 `state`、`management`、`disks` 三个空目录，全部为所选 UID/GID、0700。`disks` 只是尚无成员的发现容器，不会创建、挂载、格式化或接纳任何磁盘。

在 `/zxs/.zxs-helper-data-<预览摘要>.pending/` 内逐个创建、设置新 inode 所有权并同步；不修改已存在的 live `/zxs/data` 或其子目录权限。只允许准确请求的受控 pending 完成顶层所有权交接。所有子目录、权限、空性和身份符合后，整棵树以 `RENAME_NOREPLACE` 发布为 `data`，同步父目录，返回清楚的准备结果。

- 已存在完整且仍空的准确布局：只核验并同步，返回 `matching_empty_private_layout_durably_verified`，不冒充新建。
- 原 pending 的已完成子目录准确、其余缺少：同一请求可补齐。子目录 mkdir/chown 中断留下的异常 owner/mode 不会猜测修复，保留并拒绝。
- 已有内容、未知条目、链接、不符 owner/mode、同时存在 pending 与 final：拒绝，不清空、不 chmod/chown live 目录、不重建已有数据库。
- rename/fsync 结果不确定：保留类型化 `Unknown` / `Published` / `PublishedDurabilityUnknown` 与 pending 名/父身份；不能因 HTTP 失败判断没写入，也不能删目录重新初始化。

结果始终明确 `system_initialized=false`、`administrator_created=false`、`databases_initialized=false`、`disks_initialized=false` 和 `service_started=false`。该布局不是“系统已就绪”的 marker，也不能用于跳过最终初始化交接。

## 后续仍需实现

完整首次管理员与必要磁盘配置、SQLite/权威初始化、完整成功交接、普通服务拒绝半成状态、root 至服务身份的受控业务执行、准确中断收束，以及凭据重置/现用 systemd-Samba 配置激活仍未接齐。当前只对新空布局设置服务所有权；不对现用数据执行递归 chown。原有安全 Web 初始化入口尚未因这一前置被删除。
