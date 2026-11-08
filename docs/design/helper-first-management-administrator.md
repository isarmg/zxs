# 辅助工具：首次管理账户，不是完整存储初始化

基于 `766d8a52`，本片将首次 ZXS 管理账户的创建与原请求恢复接入独立辅助 Web。**新增源码未编译、测试、审查或运行 CI，未创建任何真实账户或修改主机。** 首批磁盘、三范围数据库、完整系统就绪交接和实际 Samba 账户/密码仍未完成。

## 入口与准确范围

新增 POST `/api/initialization/administrator-preview`、`administrator-create`、`administrator-recover-preview`。Web 位于服务私有空布局之后，输入已有本机服务账户与新 ZXS 管理员名称，预览后手动输入并重复初始密码，明确确认只创建管理账户。可保存不含密码的原请求，刷新后以原摘要恢复预览，再输入同一初始密码完成原操作。

工具仍是手动 root 二进制、默认 `0.0.0.0:9040`、无登录、明文 HTTP，不注册自启。风险提示包括网络可达者可能首次创建管理员或重置密码；只应临时用于可信隔离网络。密码不进 URL、argv、日志或回执，复用秘密字段清零、受保护 JSON 解码、同源/CSRF/IP Host、一个阻塞工作 owner 和退出排空。不声称能清零浏览器和传输栈中的全部副本。

只接受此前准备的 `/zxs/data/{state,management,disks}`，全部是同一数据树内准确服务 UID/GID、0700 的已有目录；不创建 Unix 账户、不 chown 现用目录、不接受调用者指定路径。数据根只能有这三个直接子目录，`disks` 必须为空。新请求的 `state` 只允许 `setup.lock`，`management` 只允许 `auth.lock`；配置、磁盘、旧账户、损坏文件、未知目录或临时证据均拒绝保留，不猜测它是“空系统”。

预览持有 `/zxs` 生命周期租约和正常初始化使用的 `setup.lock`，可创建保持服务所有权的 0600 锁文件，因此不是绝对零写入。创建时另外取得正常 `auth.lock`。当前 daemon、普通 setup 和凭据写者不能并行，但旧版/第三方进程仍须另行停止；操作者确认不替代 OS 排他证明。

## 通用协议与 helper 归属

通用 AuthStore 新增类型化初始候选与 no-clobber 安装，原正常初始化也共用该候选生成，保留当前用户名/密码规则、代数 1 和空会话。首次候选包含 verifier，不保存初始密码明文，不公开返回或打印。管理初始化意图编码和管理权威 marker 字节与现有 ZXS writer 共用，不复制另一份格式或增加旧格式兼容。

root 选择、空树证明、计划、日志、Web 与中断编排均放 `helper/src/initialization/administrator*`。平台新增由已持有 fd 构造行政私有目录的通用能力，保持原 UID/GID，无路径重开；原 owner-only 运行时入口不放宽。所有 marker/凭据写入锚定原目录，身份变化阻断发布，不把新路径对象当原计划目标。

## 原子文件与跨文件持久顺序

计划摘要绑定完整服务账户来源、根和三个目录身份、规范化管理员名称，不包含明文密码。确认后：

1. 在 root 所有 0700 的 `/zxs/.zxs-helper-first-admin-<摘要>/administrator.json` 先持久保存原计划和受保护候选，文件 0600。
2. 无覆盖写入并同步 `state/management-initialization.json`，使用当前 `management-without-storage` 格式。
3. 持有管理凭据锁，用原候选无覆盖创建 `management/management-auth.json`，保持服务 UID/GID、0600，文件和目录同步。
4. 无覆盖发布并同步 `state/management-authority.json`。凭据锁保持到标记与完成记录都结束，之后再释放 setup 和主服务生命周期锁。
5. 在 `/zxs/.zxs-helper-first-admin-complete-<摘要>/receipt.json` 落完成记录，包含管理员、代数 1、`management_initialized=true`，但始终 `storage_initialized=false`、`normal_storage_start_ready=false`、`service_started=false`。

这些是多个独立文件，不是一个跨目录原子事务。中断后可能只写了管理意图，也可能已经创建管理员；失败不能解释为“账户不存在”。原请求/原密码可继续准确缺项，已存在的相同初始凭据只重新核验和同步，不重生成 salt、不增加代数、不覆盖不同账户或后来的登录会话。原管理意图丢失但凭据存在、权威存在而凭据缺失、未知 staging、额外文件、双名称、身份或密码不同均拒绝，保留证据。

已完成账户之后若有新会话/密码代数、开始存储配置或 `disks` 不再为空，不能把旧初始化当重置工具；该旧操作恢复会明确拒绝，不清空后来状态。需要改现有管理员密码时使用单独的已存在管理员重置流程。完整 pending 中保留的原记录可继续发布；任意损坏/截断不是自动恢复成功。

## 尚未完成与预算

此状态对应现有管理协议的 **ManagementReady，无存储**。本片没有启动 ZXS，也没有把后台待初始化界面当最终“干净运行”要求已完成；完整 H04 启动准入门禁仍需与首批磁盘/数据库初始化一起实现。保留尚未迁入的现有安全入口，不能把本阶段成功冒充完整首次安装。

Samba Unix/passdb 创建、实际 SMB 密码重置、VFS/依赖、runtime/log 目录、systemd/Samba 语法验证和加载/启动/启用仍不在本片。没有生成空存储池、占位磁盘、外来链意图或假的 system-ready marker。

新增固定数量文件同步和意图/完成目录屏障；目录检查最多遇到允许项加一个未知项就拒绝，不扫描正文历史。账户源沿用 4 MiB，私有首次请求 64 KiB，凭据 4 MiB；Argon2 在唯一受管理阻塞 owner 内执行。测试源码覆盖准确创建/重试、后来的会话拒绝、不同账户/密码无覆盖、私有文件与 fd 身份、无明文和请求确认，但未运行，不算真实 root、浏览器或断电验收。
