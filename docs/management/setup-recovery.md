<a id="protected-setup-and-offline-management-boundary"></a>

# 受保护初始化与离线管理边界

`storage_server::setup_recovery::SetupRecovery` 是显式首次初始化与隔离恢复的同步服务边界。受管理运行时合并其公开/受保护 HTTP 路由组，并将操作与后端打开、重试、安装串行化。

<a id="local-deployment-inputs"></a>

## 本地部署输入

系统目录、管理目录，以及不透明磁盘 ID 到既有绝对磁盘目录的允许列表属于可信本地配置。HTTP 请求不能提供文件系统路径。不存在的目录不会被创建：缺失挂载表示磁盘不可用，不是新盘。不可用的允许成员仍作为状态描述保留，不阻止检查或从选定幸存成员恢复。首次初始化仍要求完整配置允许列表全部可用且为空。生产初始化接受已有受控存储目录，不要求活动挂载点、Btrfs 或物理设备独立性；共享底层故障域由用户保证，不新增确认开关。开发模拟仍需可信部署配置显式启用；目录 device/inode 和严格 seal 身份会绑定并复查，旧 seal 不迁移、不重置。

首次初始化前，本地管理员须配置 `management_root/bootstrap.key`：精确 64 个十六进制 ASCII 字符、无换行、0600 权限、普通文件、一个硬链接、非符号链接。服务不生成或记录 bootstrap 密钥，回环访问本身不是授权。适配器只通过 `X-ZXS-Bootstrap` 传递它，不能放入 URL、浏览器持久存储或日志。此能力与唯一管理员的日常会话不同。

密钥绑定配置的系统与管理目录；删除或修改密钥会撤销既有内存许可。密钥在文件修改时间后 30 分钟过期，内存许可另在十分钟后过期。bootstrap 恢复不能绕过既有管理凭据，包括损坏凭据文件。不提供默认账户、缺少认证时重置凭据、磁盘格式化或匿名远程管理。

<a id="dtos-and-adapter-obligations"></a>

## DTO 与适配器义务

- `SetupStatus`：状态 `empty`、`existing`、`recovery_required`、`unavailable`，不透明磁盘 ID 及 `local_authorization_required`；同时说明可信部署是否显式启用开发模拟。公开状态仅包含粗粒度状态，授权后才提供候选磁盘。
- `SetupRequest`：`disk_ids` 为 1—32 个不同的允许 ID，另有 `mode`（`production` / `development`）、`username`、`password`。一个存储盘足够，附加备份层可选。凭据有意不实现 Debug 或 Serialize。
- `RestoreRequest`：1—32 个不同允许 ID；bootstrap 恢复要求 `accept_stale_credentials: true`。选定幸存者必须满足原配置成员法定数，不能按可达盘重算法定数。管理盘丢失后旧密码/授权可能过时，所有恢复会话必须清除。
- `RecoveryPreviewRequest`：`operation_id`、`reason`。
- `RecoveryCommitRequest`：`recovery_id`、`digest`、`accept_permanent_loss`。

全部请求 DTO 拒绝未知字段。HTTP 适配器要求解码请求体最大 16 KiB、十秒期限、普通精确 origin / CSRF 保护、请求准入/速率限制（含 bootstrap 尝试），管理员操作须使用当前系统管理员新鲜认证。旧凭据快照不能作为离线准入。管理阻塞任务使用独立有界信号量，防止数据工作耗尽管理容量；整个阻塞操作期间持有运行时生命周期门，而非只持有到提交请求。健康的 `/api/admin/check` 不应再经离线检查器重复执行。此边界不暴露离线导出路径或文件正文。

<a id="initialization-and-interrupted-setup"></a>

## 初始化与中断

新安装要求所有配置磁盘目录为空、无配置、无新旧管理凭据、无权威标记、无先前初始化意图。未知目录内容是已有数据，不是清空授权。凭据无效必须在写存储前拒绝。

初始化取得进程级排他 `setup.lock`、验证核心配置，并在登记任何磁盘前持久写入 `setup-intent.json`。核心另获取正常系统/磁盘写锁。多盘初始化不能靠重命名单个系统文件实现原子性，因此中断留下持久围栏；不静默清理、回滚磁盘身份或按全新安装重试。

可信安装回调必须在建立 `AuthStore::initialize_management` 前持久发布管理权威标记，再按受管理运行时协议复制初始快照；不得调用旧引擎权威 `open_auth`、`protect_auth`。仅回调成功后报告成功，随后由运行时安排串行后端重开。响应说明恢复待完成或需要重启。成功后也保留意图证据，防止日后配置丢失被视为首次安装。

<a id="recovery"></a>

## 恢复

认证离线检查在一个允许盘上调用现有验证证明的核心检查器。普通系统恢复需当前系统管理员；全部管理凭据缺失时，可用明确本地 bootstrap 授权恢复既有、已验证权威凭据，不能创建新凭据。核心要求原配置元数据成员严格多数，不初始化旧盘。单成员池需要唯一幸存成员；三成员池只有一个幸存者不能授权恢复。

降级恢复使用 `Engine::open_for_recovery`，不能用可能推进已决定候选的普通 open。核心限制为已 ACCEPT 但未发布的操作，且全部原磁盘可访问。预览产生绑定计划；提交要求其 ID、摘要、同一主体及明确永久损失确认。这不是新写入的法定数绕过。

这些是应用级恢复，不是 SMART、fsck、硬件维修或任意文件系统管理。

<a id="http-endpoints"></a>

## HTTP 端点

`GET /api/setup/status` 验证 `X-ZXS-Bootstrap` 并返回授权状态与 `csrf_token`。`POST /api/setup/initialize`、`POST /api/setup/restore-system` 要求相同 bootstrap 头、状态返回的 `X-CSRF-Token`、精确同源校验及 JSON。初始化路由拒绝查询字符串；bootstrap 不能代替 CSRF。

认证管理员端点为 `POST /api/management/offline-check`、`/api/management/restore-system`、`/api/management/recovery-preview`、`/api/management/recovery-commit`，保留正常当前会话、CSRF、origin 检查。运行时结果原样返回；传输结果未知时 UI 不自动重发变更。

<a id="precisely-bounded-interrupted-initialization-completion"></a>

## 精确受限的中断初始化补全

`POST /api/setup/complete-initialization` 只接受 `username`、`password`，沿用本地 bootstrap、CSRF、origin、大小和准入检查。授权状态报告 `completion_available`，不能从凭据缺失推断。原初始化先写私有意图，绑定完整配置、池身份、选定磁盘 ID 与原 bootstrap 授权；只有存储初始化成功后，才在进入权威/凭据安装前持久记录精确原始磁盘树。

补全要求两条记录匹配、原本地密钥、配置/磁盘内容不变、当前挂载/模式检查、空工作区，以及核心原始初态无认证快照。不接受旧未绑定意图或任意既有池。既有权威标记或任何凭据文件（含损坏文件）均要求显式恢复，不能替换。它仅覆盖有证明的“存储完成、权威尚未发布”窗口；ready 之前的部分磁盘登记不支持自动续做，也不能仅凭 pool ID 认领现场；权威发布后、主凭据建立前崩溃仍失败关闭，不宣称所有中断都能自动补全。

<a id="locally-provisioned-configuration-presets"></a>

## 本地预设配置

部署可将不透明 preset ID 映射到本地绝对配置文件。登记不读取文件，不可用预设不能阻止管理监听。只有显式选择且 bootstrap 授权的动作才打开文件：不跟随末级符号链接、普通单链接文件、有效用户所有、0400/0600 权限、最多 64 MiB，与本地草稿边界一致。非法 ID 不作为 Web 候选；HTTP 从不接受配置路径或配置 JSON。

`POST /api/setup/preset-preview` 接受 `preset_id`，返回 `preset_id`、`config_digest` 及验证后的配置，将所有盘根替换为不透明 `candidate_id`，保留拓扑、资源上限及其他 Config 参数供审阅。`POST /api/setup/initialize-preset` 要求 `preset_id`、已审阅 `config_digest`、`username`、`password`。本地预设变化需重新预览，不能静默改变已确认安装。

每个选定预设盘根必须映射到本地允许盘。核心配置、全安装空状态、当前盘身份、生产挂载/独立性、开发显式启用、bootstrap 时效/撤销、串行化和持久初始化证明均不变。同一初始化事务保留全部验证 Config 参数，包含附加非元数据成员；不将预设挤入默认单盘或丢弃可选备份。精确权威前补全流程同样适用于按原绑定配置中断的预设初始化。

<a id="rejected-or-interrupted-storage-recovery"></a>

## 拒绝或中断的存储恢复

bootstrap 恢复在磁盘恢复后、安装管理凭据前再次核验本地许可。恢复期间密钥删除、轮换、过期或变得不安全都会拒绝发布。存储配置及恢复凭据快照可能已经写入；这些是恢复证据，不是授权。

后台存储发现及进程重启从不从引擎快照安装凭据，即使管理凭据文件与权威标记均缺失。缺失/损坏凭据继续失败关闭。有效本地管理凭据保留权威；显式授权恢复成功时，在安装快照前撤销恢复会话。

离线 `zxs-rescue restore-system` 仅恢复存储元数据；`credential_recovery_snapshot_available` 不授权管理登录。保留部分或被拒绝恢复状态供检查。恢复管理凭据须停止相关服务，在另行配置的干净系统状态中明确授权 bootstrap 恢复，并确认凭据可能过时。不得删除或覆盖在线权威、凭据或部分恢复证据，把安装伪装为空。没有自动迁移或 CLI 密码重置回退。
