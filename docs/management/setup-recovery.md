<a id="protected-setup-and-offline-management-boundary"></a>

# 首次账户与受保护存储管理边界

`storage_server::setup_recovery::SetupRecovery` 是显式首次初始化与隔离恢复的同步服务边界。受管理运行时合并其公开/受保护 HTTP 路由组，并将操作与后端打开、重试、安装串行化。

<a id="local-deployment-inputs"></a>

## 本地部署输入

系统目录与管理目录属于可信本地配置，服务可创建这些系统状态目录；这不创建或初始化任何数据盘。首次设置只创建管理员账户，不读取候选盘是否为空、是否挂载。登录后管理员通过只读目录浏览选择当前已挂载文件系统，发现范围只限默认 `/zxs/data/disks` 或管理员持久设置的其他根内目录，根本身只作列表容器，不可选为存储。分别设置 `mount_path` 与实际 `root_path`，后者可以是任意层级的合法空子目录，无需部署者预先登记候选白名单。路径每个分量禁止符号链接，所选根须为服务账户所有的私有 0700 空目录，且不与系统、管理或其他选定根重叠。目录检查与提交重新绑定挂载及目录身份，核心仍核验 seal。服务不分区、格式化、挂载、不改权限，也不把多个同设备目录声称为独立磁盘。`--setup-disk` 的允许 ID 仅保留既有离线恢复用途。

首次账户设置不需要密钥；无账户且无历史权威时，任何可访问 Web 的人可创建唯一管理员。必须在安装阶段控制网络可达范围。只有明确的灾备恢复，本地管理员才须配置 `management_root/bootstrap.key`：精确 64 个十六进制 ASCII 字符、无换行、0600 权限、普通文件、一个硬链接、非符号链接。服务不生成或记录 bootstrap 密钥，回环访问本身不是授权。适配器只通过 `X-ZXS-Bootstrap` 传递它，不能放入 URL、浏览器持久存储或日志。此能力与唯一管理员的日常会话不同。

密钥绑定配置的系统与管理目录；删除或修改密钥会撤销既有内存许可。密钥在文件修改时间后 30 分钟过期，内存许可另在十分钟后过期。bootstrap 恢复不能绕过既有管理凭据，包括损坏凭据文件。不提供默认账户、已有或损坏凭据的匿名重置、磁盘格式化或匿名存储管理。

<a id="dtos-and-adapter-obligations"></a>

## DTO 与适配器义务

- `SetupStatus`：状态 `empty`、`management_ready`、`existing`、`recovery_required`、`unavailable`；公开首次状态只返回 `state`；独立 `recovery-status` 在 bootstrap 授权后返回恢复字段与 `csrf_token`。`management_ready` 表示系统管理权威已建立、尚未激活存储，不代表零成员池。
- 首次初始化请求只含 `username`、`password`，拒绝 `disk_ids`、`mode`、预设或配置字段；成功响应为 `initialized: true, storage_pending: true`。凭据不进入诊断或浏览器持久存储。
- `StorageActivationRequest`：`selections` 为 1—32 项已检查的目录身份，另有 `mode`。只接受当前管理员会话，开发模式必须由可信本地配置启用。第一项为 P01 主存储，后续按顺序形成逐级备份链，所有项都是元数据成员；正常写入全部成员在线，系统恢复要求原成员严格多数。
- `RestoreRequest`：1—32 个不同允许 ID；bootstrap 恢复要求 `accept_stale_credentials: true`。选定幸存者必须满足原配置成员法定数，不能按可达盘重算法定数。管理盘丢失后旧密码/授权可能过时，所有恢复会话必须清除。
- `RecoveryPreviewRequest`：`operation_id`、`reason`。
- `RecoveryCommitRequest`：`recovery_id`、`digest`、`accept_permanent_loss`。

全部请求 DTO 拒绝未知字段。HTTP 适配器要求首次账户与恢复请求体最大 16 KiB，目录检查／发现根修改为 64 KiB，首次激活为 4 MiB（覆盖 32 项、四条 4096 字节路径及 JSON 转义），所有请求保留十秒期限、首次设置要求精确 Origin、Fetch Metadata 与 JSON；已有管理员操作使用普通 CSRF 保护、请求准入/速率限制（含 bootstrap 尝试），管理员操作须使用当前系统管理员新鲜认证。旧凭据快照不能作为离线准入。管理阻塞任务使用独立有界信号量，防止数据工作耗尽管理容量；整个阻塞操作期间持有运行时生命周期门，而非只持有到提交请求。健康的 `/api/admin/check` 不应再经离线检查器重复执行。此边界不暴露离线导出路径或文件正文。

<a id="initialization-and-interrupted-setup"></a>

## 初始化与中断

账户初始化以独立的 `setup.lock` 一次性准入，不占用存储恢复门，也不排空已有池。它在系统目录持久保存管理初始化意图，先以 create-new 创建凭据，再发布系统管理权威并安装当前认证，不调用 `Engine::initialize`。已存在配置、凭据、权威或旧池证据不能当作新安装；凭据已创建但权威发布中断时不允许匿名覆盖账户，服务重启仅补建该现有账户的权威。既有权威但凭据缺失仍保持恢复要求。

首次存储激活取得排他生命周期门与 `setup.lock`，重新校验当前管理员、精确目录选择及持有描述符，先保存独立激活意图，再建立至少一个元数据成员的真实池。不会构造零成员配置，也不接受任意 Config JSON。初始账户沿用现有系统权威，只将其灾备快照复制到新池。

只有持久完成证明成立后才报告激活成功并安排后端打开。后台启动先检查激活准入，即使 `config.json` 已写但激活未完成，也不能提前接收业务数据。多目录创建中断保留原意图与证据，不清理身份或按新池重试。浏览器关闭不取消服务已接管的工作；不确定结果先查状态。明确恢复/重启状态的成功响应不是后台存储已可读写的保证。

<a id="recovery"></a>

## 恢复

认证离线检查在一个允许盘上调用现有验证证明的核心检查器。普通系统恢复需当前系统管理员；全部管理凭据缺失时，可用明确本地 bootstrap 授权恢复既有、已验证权威凭据，不能创建新凭据。核心要求原配置元数据成员严格多数，不初始化旧盘。单成员池需要唯一幸存成员；三成员池只有一个幸存者不能授权恢复。

降级恢复使用 `Engine::open_for_recovery`，不能用可能推进已决定候选的普通 open。核心限制为已 ACCEPT 但未发布的操作，且全部原磁盘可访问。预览产生绑定计划；提交要求其 ID、摘要、同一主体及明确永久损失确认。这不是新写入的法定数绕过。

这些是应用级恢复，不是 SMART、fsck、硬件维修或任意文件系统管理。

<a id="http-endpoints"></a>

## HTTP 端点

`GET /api/setup/status` 公开返回最小 `{state}`；`POST /api/setup/initialize` 无授权密钥，仅接收账户密码，保留同源、Fetch Metadata、JSON、来源限流与锁内一次性准入。`GET /api/setup/recovery-status` 仍要求 `X-ZXS-Bootstrap` 并提供灾备 `csrf_token`；`POST /api/setup/restore-system` 使用该 bootstrap 头与 `X-CSRF-Token`。首次设置与灾备授权是独立边界。

登录后的存储管理端点：

- `GET/PUT /api/admin/storage/discovery-root` 读取或以 `expected_discovery_root` 原值比较保存发现根；默认 `/zxs/data/disks`。拒绝未完成激活时更改范围；不会迁移已激活目录。
- `GET /api/admin/storage/status` 返回 `pending`、`active`、`recovery_required` 或 `unavailable`、原意图补全是否可用及开发模式策略。
- `GET /api/admin/storage/directories?path=…` 有界浏览真实目录；`POST /api/admin/storage/directories/inspect` 只读检查挂载目录与存储子目录并返回身份绑定。检查通过不等于已添加。
- `POST /api/admin/storage/activate` 提交已确认目录选择；`POST /api/admin/storage/complete-activation` 只接受空对象、仅完成服务器已证明的原激活。成功响应 `activated: true`，`recovering` 与 `restart_required` 恰有一项为真。

认证离线端点仍为 `POST /api/management/offline-check`、`/api/management/restore-system`、`/api/management/recovery-preview`、`/api/management/recovery-commit`，保留当前会话、CSRF 与 origin 检查。

<a id="precisely-bounded-interrupted-initialization-completion"></a>

## 原激活补全与结果未知

补全绑定原意图、配置和选定目录证据，只使用当前已有管理账户，不接收新路径或密码。已完成证明存在时重复查询/补全不得再次建立池或替换凭据；目录身份、内容或权威证据变化时明确拒绝并保留现场。不能仅从缺失配置或网络超时推断可重试。实际可补全阶段以服务验证的 `completion_available` 为准，不宣称所有初始化中断均可自动续做。

<a id="locally-provisioned-configuration-presets"></a>

## 已移除的旧首次存储入口

公开 `preset-preview`、`initialize-preset`、`complete-initialization` 路由已删除，首次设置没有选盘或预设分支，不保留旧请求格式兼容。`zxs-rescue configuration` 仍可生成未初始化草稿，但当前 Web 不应用任意 Config 或本机预设；不能用草稿修改已激活池。已激活池在线增删成员仍待实现，见[设计边界](../design/online-disk-management.md)。

<a id="rejected-or-interrupted-storage-recovery"></a>

## 拒绝或中断的存储恢复

bootstrap 恢复在磁盘恢复后、安装管理凭据前再次核验本地许可。恢复期间密钥删除、轮换、过期或变得不安全都会拒绝发布。存储配置及恢复凭据快照可能已经写入；这些是恢复证据，不是授权。

后台存储发现及进程重启从不从引擎快照安装凭据，即使管理凭据文件与权威标记均缺失。缺失/损坏凭据继续失败关闭。有效本地管理凭据保留权威；显式授权恢复成功时，在安装快照前撤销恢复会话。

离线 `zxs-rescue restore-system` 仅恢复存储元数据；`credential_recovery_snapshot_available` 不授权管理登录。保留部分或被拒绝恢复状态供检查。恢复管理凭据须停止相关服务，在另行配置的干净系统状态中明确授权 bootstrap 恢复，并确认凭据可能过时。不得删除或覆盖在线权威、凭据或部分恢复证据，把安装伪装为空。没有自动迁移或 CLI 密码重置回退。
