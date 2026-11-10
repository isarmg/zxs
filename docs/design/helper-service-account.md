# 辅助工具本机服务账户准备

首装向导保持八个原步骤。安装根和内置程序完成后，第 3 步可带入同一账户／主组到独立账户卡片，明确准备后回到私有布局核对；不重做此前步骤，也不增加浏览器格式兼容。

## 当前入口与行为

- CLI：`initialization account-preview` 和 `account-prepare`，传入 `--service-user`、`--service-group`；写入另需准确摘要、`--confirm-local-service-account` 和 `--external-administration-stopped`。
- HTTP：`POST /api/initialization/account-preview`、`account-prepare`。继续使用 helper 的同源／CSRF、并发任务所有者和 root 边界。无认证明文控制台必须限制在可信隔离网络。
- 已有账户只准入本地唯一非 root UID 和准确主组 GID，不修改账户或密码。缺失账户必须使用同名主组；首次选择时主组也必须缺失，不能隐式借用已有组。
- 复用固定 ELF 执行器打开 `/usr/bin/systemd-sysusers`，保留程序身份并以 fd 执行 `--inline` 的单条 `u` 指令，不经 shell、PATH 或运行时下载。新账户使用 `/nonexistent` 和 `/usr/sbin/nologin`，不创建 home、不设置登录密码、不启动或启用服务。

本机 systemd-sysusers 负责系统账户文件锁与原生发布。独立 Samba Unix 账户复用同一 CLI／HTTP 原生入口，不另造账户创建器。控制台的两张账户卡片分别保存原输入和摘要，避免完成 ZXS 账户后冻结记录阻断第二个账户；首装第 3–5 步可带入原配置的 Samba 名称及同名组，准备后回到同一原步骤。部署入口仍核验两者为独立非 root 身份，Unix 用户准备不等于 Samba passdb 登记或密码认证。该入口不安装 OS 依赖；已有正式发布链负责把同源码 zxs、Web、VFS 和法律材料嵌入可安装 helper。

## 边界与中断

共享本地身份解析新增 typed `Option` 查找，真正缺失与坏记录、链接、权限或 I/O 错误分开。名称仍使用既有 `Name`，没有第二套正则或忽略错误后当成缺失的路径；只保留 home／shell 和 UID/GID，不读取或保存 shadow 密码材料。

在 `/zxs` 原目录租约内，以固定账户名保存私有原计划，绑定宿主、输入、既有 UID/GID 或准确 sysusers 程序。执行前保存原计划；成功读回后保存 UID/GID 完成绑定。确认返回的仅是本机服务身份已准备，不是完整安装、持久存储或服务就绪。

响应丢失后，浏览器只读查询同一计划，不自动重发。原命令可能已完成且当前名称、UID/GID 与非登录资料满足原计划时，另行确认可保存完成回执而不再执行命令。仍完全缺失时才可明确重试；若只出现主组而账户仍缺失，保留材料并要求检查部分原生结果，不借用或删除该组。完成后账户消失或 UID/GID 改变时拒绝补造。外部账户管理员须另行停止，helper 租约不能替代他们的系统锁。

## 验证边界

根 `test/platform/` 覆盖缺失与损坏区分、唯一非 root UID/GID 和资料读取；根 `test/helper/` 覆盖固定指令、不可覆写 UID／程序／home／shell、确认前拒绝及实际 JS 处理器的发送前保存、丢响应、刷新、冻结输入和明确续作。Node 使用 DOM／transport doubles，不执行系统账户命令。

按当前要求未本地编译或运行原生账户创建。Rust 编译／单测和 Debian 13 隔离 root 环境的实际 sysusers、首次布局、systemd 与 SMB 联合流程仍待 CI／验收，未修改真实主机账户或磁盘。
