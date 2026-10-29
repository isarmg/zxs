<a id="storage-server"></a>

# 存储服务端（storage-server）

Axum HTTP 服务、持久会话认证及受保护的 Samba 本机桥接。业务变更经过 `storage-engine::Engine`，HTTP 层不直接修改数据盘内容。

默认与无 `core` 构建都依赖 `platform-io` 的 Linux amd64 编译期准入；认证、
spool、管理及初始化恢复直接使用 Unix 权限与链接检查，不保留非 Unix
文件操作回退。权限、单链接、可信目录、拒绝符号链接及 I/O 错误处理仍在实际使用处执行。

业务接口统一使用无版本号的 `/api/` 前缀；客户端、Cookie 路径及路由校验共同使用该命名空间，不提供旧数字版本路由别名。

## 代码职责

| 模块 | 职责 |
| --- | --- |
| `lib.rs` | 服务配置、共享状态、路由组合、存储适配接口及通用 JSON 边界 |
| `http_auth` | 同源/CSRF 准入、登录与改密隔离的限流及共享密码计算容量、管理员会话 HTTP 接口 |
| `auth` | 唯一管理员凭据与会话持久化、备份发布、代际撤销和恢复阻断 |
| `transfer` / `uploads` | HTTP 内容流/范围/读租约；上传进度的有界读取与持久化 |
| `web_assets` | 静态资源准入和 ETag，保留 `/api` API 命名空间 |
| `engine_backend` / `file_manager` | 核心业务约束、业务动作和文件管理响应适配 |
| `engine_backend/file_manager/activity`、`backend_activity` | 在当前授权锁内捕获提交、文件元数据操作、目录分支操作、备份配置、路径节点及目录树六类活动的 typed 快照；按序号降序和事件 ID 保留最新 200 行，锁外转换 DTO/UTC 时间 |
| `telemetry` | 有界可重建的主机采样和历史，不参与存储提交真相 |
| `smb_rpc` | 本机 Samba 会话、句柄与工作区发布，不创建账户或仓库权限 |

密码、令牌和 CSRF 参数修改只在 [authentication](auth.md) 进行；Cookie 生命周期引用 `auth` 的同一会话有效期常量。存储格式和业务规则以核心为准。

## 启动

从项目根目录构建 `CARGO_TARGET_DIR=target/native cargo build --locked --offline -p storage-server -p storage-rescue`。下面是已有池或诊断模式的最小启动样例；首次安装须在启动前按下节配置 `--setup-disk` 白名单并预置引导密钥，再按[受保护 Web 引导](../getting-started.md)完成初始化。只运行下面的命令不会提供可选初始化磁盘：

```sh
target/native/debug/zxs --root /srv/zxs/system \
  --listen 127.0.0.1:7400 --web-dir /srv/zxs/web/dist
```

`RUST_LOG` 仅在环境变量缺失时默认 `warn`；已配置值交给当前本地 `EnvFilter` 严格解析，空值保留其显式关闭日志的语义，不当作缺失。无效 directive（包括合法项与无效项混合）或非 Unicode 值以退出码 1 拒绝启动，发生在创建 Tokio 运行时、状态目录及监听服务之前。stderr 分别报告 `zxs: RUST_LOG contains an invalid filter directive` 或 `zxs: RUST_LOG must contain valid Unicode`；具名错误保留原始解析／环境读取 source，但进程诊断不输出可能含凭据的环境值或 source 内容。`--help`、`--version` 和 CLI 语法检查仍先于日志配置，不受无效 `RUST_LOG` 影响。

HTTP 监听地址必须是回环地址。对外部署由同机反向代理终止 TLS；同时指定公开 HTTPS Origin，例如 `--external-origin https://storage.example.org`。服务只信任显式配置的 Origin，不根据任意转发头改变安全策略。`--web-dir` 可省略；省略时仅提供 API。

公开 `router` / `serve_with_shutdown` 入口（CLI 经 `serve` 转入）消费原始 `ServerConfig`，失败返回错误；准入后以私有不可变配置及 `Arc` 共享，不在请求提取或中间件克隆中复制路径、代理列表。静态 HTTPS Origin 在准入时解析一次，缓存浏览器规范化表示；同源比较和 Secure Cookie 使用同一证明。每个请求的 Origin 仍逐次检查数量、文本及精确同源值，未配置外部 Origin 时仍逐次解析并限制请求 Host 为回环；CSRF、Sec-Fetch-Site、真实代理对端与转发头校验不缓存、不省略。配置修改需重新准入，不提供运行中可变出口。

`--max-upload-bytes` 设置单次完整上传上限，默认 1073741824 字节；实际公开上限取它与存储池 `max_file_bytes` 的较小值。HTTP 上传和读取各自默认最多 2 个活动操作，唯一管理员的全部会话共享对应池，支持显式配置，见[传输准入与期限](#http-transfer-limits)；密码验证最多并发 4 个。引擎工作区数量、字节数、过期时间等限制来自初始化 JSON 配置。发布仍经互斥锁串行执行，上传、读取及工作区提交的部分准备在锁外完成，详见[写入流程](../architecture/write-flow.md)；未提供吞吐量保证。

## 初始化与故障启动

`zxs --root SYSTEM_STATE --management-root SYSTEM_MANAGEMENT --web-dir WEB_ROOT` 直接提供 Web/API，不分发管理子命令。系统盘管理目录与数据成员分离，HTTP 先监听，再打开数据引擎；数据故障不自动初始化、清空池、放宽授权或接受损失。systemd 不要求数据成员挂载成功，也不创建、挂载或格式化设备；Samba/smbd 是独立网络服务。

启用 SMB 时，其完整工作副本、未确认操作证据和预算记录固定保存在 `--management-root/smb-spool`，与可清理的 `--smb-socket` 运行目录分离。管理目录必须位于持久系统盘；spool 不可用只停止 SMB 桥接，不阻止管理 HTTP。旧 socket 旁的 spool 不自动迁移，部署与升级边界见 [SMB 本机 RPC](smb.md#smb-local-rpc)。

本机管理员可重复设置 `--setup-disk ID=/absolute/existing/root` 批准初始化磁盘，或使用 `--setup-config ID=/absolute/private-file` 提供配置预设。浏览器只能选择批准的 ID，并审查绑定摘要，不能提交任意系统路径或 Config JSON。预设文件须由服务账号拥有、为单链接普通文件且权限为 0400/0600，所有磁盘仍须在白名单中。

初始化需要管理目录中预置的私有 `bootstrap.key`：权限 0600、恰好 64 个十六进制字节、无换行，从 mtime 起 30 分钟有效。daemon 不自动生成或日志输出引导密钥。仅本地开发测试可显式设置 `--allow-development-setup` 使用目录模拟盘；生产默认拒绝，模拟盘不证明设备独立性或断电安全。

初始管理员与存储初始化绑定同一 setup intent；只允许补全可证明发生在权威建立前的中断，不提供通用密码重置。缺少可信权威快照且无绑定 setup intent 的孤立旧池，或已有权威标记但凭据缺失/损坏时，均不得自动创建管理员。

日常管理接口及页面对应见[管理参考](daily-management-coverage.md)，离线恢复及配置草稿见[救援工具](rescue.md)。

## 唯一管理员认证

首次受保护初始化只创建一个管理员，用户名仍可自选并按共享规则规范化。系统没有账户列表、新增、删除、禁用、角色或仓库授权操作。管理员身份不能代替存储就绪、主分支、只读别名、容量与发布检查。

`POST /api/auth/login` 接受严格的 `{username,password}`，返回 `token`、`csrf_token`、`principal` 与十进制字符串 `expires_at`，同时设置 HttpOnly、SameSite=Strict 的 `zxs_session` cookie；生产 HTTPS 模式带 Secure。会话默认 12 小时有效，最多 64 个，达到上限返回 429。密码采用 Argon2id，持久状态只保存会话 token 摘要。每个来源 IP 每 60 秒最多尝试登录 10 次，轮换用户名不增加额度；密码验证保持独立有界并发。

登录 JSON 请求体上限为 8 KiB，改密为 16 KiB，容纳合法密码的 JSON Unicode 转义膨胀；解码后的密码规则不变。超过字节上限返回 413，普通请求体仍受 30 秒总期限约束，不放宽密码验证并发或尝试次数限制。

`GET /api/auth/me` 只返回 `principal`、`csrf_token`。`POST /api/auth/logout` 持久撤销当前会话并返回 `{logged_out:true}`，清除 cookie。Cookie 变更请求需要精确 Origin 和 `X-CSRF-Token`；Bearer 认证不要求 CSRF，但提供的 Origin 仍必须通过检查。令牌、CSRF 和密码不进入日志。

`POST /api/auth/password` 只接受 `current_password`、`new_password`。新密码遵守 12 至 1024 UTF-8 字节且无 ASCII 控制字符的共享规则。旧密码不正确返回 `403 CURRENT_PASSWORD_INVALID` 并保留会话；成功返回 `{changed:true,reauthentication_required:true}`，推进凭据代数、撤销全部会话并清除 cookie。每个来源 IP 和管理员主体分别限制每分钟 10 次改密尝试，尝试计数与键容量独立于公开登录流量，但与登录共享密码计算并发上限。变更先耐久发布到系统盘主库，不以数据盘可用作为登录或改密的前提。

管理员凭据代数已达 UInt64 上限时，正确旧密码的改密请求返回 `409 AUTH_STATE_INVALID`、`retryable:false`。等待或重试不能释放代数；拒绝保留原密码、全部会话和持久字节，不清除 cookie、不回绕代数，也不触发重置或恢复。最后一次合法递增（上限减一到上限）仍正常改密并撤销旧会话；登录、尝试次数和密码计算并发的暂时准入上限仍返回可重试的 429。

私有凭据库是必填单体 `administrator` 与 `sessions`：管理员只保存 `username`、`password_hash`、字符串代数 `generation`；每个会话以 token 摘要为键，只保存字符串 `generation`、`expires_at`。旧 `accounts` 集合、角色、禁用状态、仓库准入及旧会话用户名等未知字段一律拒绝。不能自动选择旧账户、迁移格式或重置认证。

正常 daemon 的认证主权威是系统盘 `management-auth.json`。`management-authority.json` 围栏阻止主库丢失后自动恢复旧凭据；数据池仅接收不含会话的管理员灾备快照，不是离线授权回退。主库缺失或无效时登录失败关闭，静态 Web、公开诊断和受保护恢复仍可用。显式凭据恢复必须接受恢复旧密码的后果，恢复会话一律丢弃。

启动决定进入凭据恢复状态时，仅在该决定边界记录一次具体诊断，保留文件系统与认证来源链。凭据 JSON 的重复键、未知字段和类型错误可能含秘密输入，因此仅记录原错误码、解码类别及行列，不打印 JSON 错误文本或凭据内容；公开响应与失败关闭语义不变。离线 setup 命令在原有授权之后、取得恢复生命周期或撤下引擎之前解析为私有具名 DTO，执行时消费同一份值，不再次解码；实际磁盘读取、bootstrap 和发布前撤权检查仍独立保留。

受管理请求取得引擎锁后按当前主库重新核验管理员及会话；排队请求不能凭旧身份绕过注销或改密。已准入执行的事务可能完成，已传输的字节不能收回，后续请求必须重新验证。可信嵌入式模式的受保护认证快照也使用同一单体 schema，不是 daemon 管理主库的替代来源；未挂接认证库的可信本机调用仍需显式主体，不构成网络匿名入口。

后台主机采样使用服务内部只读观测能力，在引擎锁内捕获 `DiskObservationPlan`、锁外查询磁盘，不伪造管理员或发布业务状态。监控容量来自身份校验后的真实采样；未知值保持空值。此能力不能从 HTTP/SMB 请求构造，系统状态及历史响应仍需当前有效管理员会话。

目录分页的 `snapshot` 摘要由持有 Engine 的 `AdmissionEngine` 惰性保存一份；配置与已发布业务代数不能通过共享 Engine 引用修改，任何 `DerefMut` 在交出可变引用前清空摘要，包括失败操作、认证发布和整个 Engine 替换。后续首读按原 JSON 字节重新散列；仅认证发布仍得到相同摘要，不把有效游标误报为业务变化。每次请求仍先复核当前管理员，再校验游标的查询、快照与排序字段；该摘要不是授权凭据，也不缓存页面、配置权威或磁盘健康。请求摘要和游标编码保持原格式，不增加过期时间或旧格式分支。

主盘条目的排序键直接借用已准入 `PortablePath` 的末级规范化键再复制到页候选；路径按组件独立规范化，因此与 `lookup_key(entry.name())` 一致。Unicode 小写字段仍单独计算，不能用完整 case folding 代替小写排序／搜索；仓库显示名和备份逻辑路径仍沿用原计算。外来游标继续重新计算并核对其排序字段，不能因内部路径已准入而跳过输入校验。资源和剩余扫描成本见[目录分页预算](../19-capacity-and-performance.md#s19-04)。

公开 `/readyz` 不需要版本放置账本或磁盘容量：锁内捕获共享不可变配置、mode、sequence 和成员计数，锁外只逐个校验配置中的 metadata member 当前 seal/设备身份，不采样 `statvfs`、不扫描非 metadata 盘及历史 placements。配置准入时建立私有成员索引；配置克隆只共享 `Arc`，root rebinding 使用 copy-on-write 保留旧快照，成员关系不变。捕获为 O(1)，探测随 metadata 成员数增长，与历史量无关；这不是吞吐实测结论，也不是在线状态缓存。

只读恢复仍返回 503 `RECOVERY_REQUIRED`；任一配置 metadata member 不可用仍返回 503 `METADATA_UNAVAILABLE`，现在保留首个失败的内部 source，公开消息不暴露本机路径。没有 metadata member 的配置仍在输入边界拒绝。非 metadata 盘离线不改变 ready 的判断（此前也只统计 metadata 的 online）；移除无消费者的容量采样不改变判定。正常响应 mode、sequence、storage/member counts 和 Management 状态语义不变。此观测不是写入许可，写侧仍在实际使用/提交处校验实时身份及完整性。

## 已实现的路由

除公开探针、登录及独立 bootstrap 入口外，业务路由都需要当前唯一管理员认证；操作还须满足存储准入与业务约束。

| 路由 | 方法与作用 |
| --- | --- |
| `/healthz`、`/readyz`、`/api/capabilities` | GET，存活、就绪、实际能力 |
| `/api/auth/login`、`/api/auth/me`、`/api/auth/logout` | POST 登录、GET 当前会话、POST 撤销 |
| `/api/auth/password` | POST 校验当前密码并更新本账号密码，撤销原有会话 |
| `/api/file-manager/disks`、`/disks/{slot}/entries` | GET 独立磁盘目录、文件列表与冻结目录分支组成 |
| `/api/file-manager/disks/{slot}/write-sessions`、`/namespace` | POST 当前文件上传会话、仅创建目录（重命名/删除使用带身份和子树前置条件的 namespace-batch） |
| `/api/file-manager/disks/{slot}/directory-branches`、`/directory-select` | POST 登记或选择目录分支的完整冻结子树 |
| `/api/file-manager/disks/{slot}/namespace-batch` | POST 显式源/目标目录分支映射，原子移动、复制、删除、重命名 |
| `/api/repositories/{repo}/directory-branches`、`/directory-branches/{id}` | GET 全部或指定路径作用域与冻结条目；PATCH 分支标签 |
| `/api/repositories/{repo}/default-branch` | GET/PUT Web/SMB 共用默认根目录分支 |
| `/api/file-manager/files/{file}/nodes/{node}` | PATCH 文件路径节点独立备注，最多128 UTF-8字节 |
| `/api/file-manager/files/{file}/branches/{branch}` | PATCH 公共文件分支标签，元数据修改，不生成版本或目录子树 |
| `/api/file-manager/files/{file}/history`、`/branches`、`/select`、`/metadata` | GET 文件版本/分支/备注；POST 创建、选择文件分支或更新文件备注 |
| `/api/file-manager/versions/{version}/metadata`、`/api/file-manager/history` | POST 版本备注；GET 最近实际文件操作 |
| `/api/system/status` | GET 管理员 CPU、内存与磁盘真实容量采样 |
| `/api/repositories`、`/api/repositories/{repo}` | GET 列表/详情，POST 建仓 |
| `/api/repositories/{repo}/smb-binding` | GET/PUT 显式 SMB 分支绑定 |
| `/api/repositories/{repo}/branches` | GET 分支、POST 登记独立待写入分支 |
| `/api/branches/{branch}`、`/tree`、`/resolve` | GET 元信息/目录树、POST 路径解析 |
| `/api/branches/{branch}/write-sessions`、`/archive` | POST 工作区写会话或归档 |
| `/api/workspaces`、`/api/workspaces/{workspace}` | POST 创建、GET 状态；可绑定历史提交 |
| `/api/workspaces/{workspace}/files/{item}/content` | PUT 完整上传、GET/HEAD 私有工作内容 |
| `/api/workspaces/{workspace}/files/{item}/upload-status` | GET 持久范围、代际、封存状态 |
| `/api/workspaces/{workspace}/namespace-operations` | POST mkdir/rename/delete/delete_tree |
| `/api/workspaces/{workspace}/seal`、`/commit`、`/abort` | POST 封存、原子发布、显式放弃 |
| `/api/versions/{version}`、`/placement`、`/checksums`、`/content` | GET 元信息/路由/校验表/内容，内容支持 HEAD 与单段 Range |
| `/api/files/{file}/history`、`/history/versions/{version}/sources`、`/api/commits/{commit}` | GET 有界正文版本页、版本来源分片或提交 |
| `/api/history-trees/{history}/graph`、`/merge-links` | GET 图、POST 仅关联的合并链接 |
| `/api/history-trees/{history}/purge-plans`、`/split-plans`、`/historical-save-plans` | POST 预览，尚不执行变更 |
| `/api/plans/{plan}`、`/commit`、`/cancel` | GET 已保存预览、POST 按摘要执行或取消 |
| `/api/policies/{policy}`、`/api/primary-placement-rules/{rule}` | GET 保护策略／主盘规则；PUT 按 generation 更新，保护策略仅接受无稳定 scope 的现行路径规则 |
| `/api/admin/disks`、`/api/backup-topologies/default`、`/api/admin/check` | GET 盘状态、拓扑、既有目录诊断 |
| `/api/admin/scrub`、`/api/admin/scrub/{job_id}/cancel` | GET 完整巡检状态、POST 启动、POST 请求取消 |
| `/api/operations/{operation}`、`/api/events`、`/api/compare` | GET 结果/分页事件、POST 两次提交比较 |

表中同一行的缩写路径共享资源前缀，例如 `/api/branches/{branch}/tree` 和 `/api/branches/{branch}/resolve`。请求字段以代码及 [HTTP 契约](../15-api-and-errors.md) 为准；64 位整数使用规范十进制字符串。重复 JSON 键、浮点数、未声明的变更字段会被拒绝。失败响应包含 `error.code/message/retryable/request_id`，不会把未实现操作返回为成功。

## 锁外完整巡检

正常 daemon 在存储准入后自动运行完整巡检；默认每轮结束后 24 小时再调度，实际读取限速 64 MiB/s，成功报告从扫描开始起 48 小时内有效。主机参数 `--scrub-interval-seconds`、`--scrub-max-age-seconds`、`--scrub-bytes-per-second` 分别默认 `86400`、`172800`、`67108864`；必须为正，最大年龄不能短于间隔。间隔不是扫描完成 SLA；长扫描可能完成时已过期。

- `POST /api/admin/scrub` 接受空对象，返回 202 和字符串 `job_id`；同时只能一个 queued/running job，重复请求返回 409。请求不能覆盖主机限速/周期。
- `GET /api/admin/scrub` 返回 `phase`、`latest`、`last_complete_success_at`、`freshness`。运行状态包括 `baseline.sequence/snapshot_hash`、`started_at`、字符串 `bytes_read` 与 `expected_payload_bytes`。后者仅为正文期望字节，不是总 I/O，不能据此显示总完成百分比。
- `POST /api/admin/scrub/{job_id}/cancel` 接受空对象，返回 202 表示取消已请求；实际 I/O 结束后才能出现终态。旧 job ID 不会取消新任务。

接口都要求当前管理员认证；排队启动还会在实际 Engine 准入处再次核验撤权。status/cancel 不等扫描的 Engine 锁。状态与写成功和 `/readyz` 分离：只有完整无故障扫描更新成功时间；取消、过期、部分故障、重开和 worker 失败不产生绿色结论。HTTP 时间使用 UTC `YYYY-MM-DDTHH:mm:ssZ`；无成功记录为 null，freshness 为 `not_checked/fresh/expired/unavailable`。

`/api/admin/check` 保留原目录诊断语义，没有暗换成全池读验。嵌入 `ManagementOptions::default()` 仅手动启用；无托管 owner 的 `EngineBackend` 返回 Unsupported，无 core 构建不提供新路由。所有实际后台 I/O、lease 和 Engine owner 结束后服务关闭才返回；详细资源、基线及验证边界见[巡检生命周期](../design/integrity-scrub-lifecycle.md)。

### 周期 owner 与内部复用资格

托管 native worker 在实际周期策略下持有核心签发的 `IntegrityScrubSchedule`；注册本身不提供资格。它通过 `prepare_scheduled_integrity_scrub` 执行真实扫描，只有私有 Outcome 回到同一 writer/config/撤销 epoch 的 finish 才建立内部共享元数据覆盖。资格窗口不超过 `min(扫描开始 + max_age, 接纳完成 + interval)`；普通写入和公开报告不能刷新，错过下一轮自动失效。此资格不等于完整正文健康，也尚未启用普通发布跳过旧依赖检查。

资格依据真实周期 owner，而不是手动/周期触发标签：daemon 的同一周期 worker 可以接纳手动启动的真实扫描，仍在 Engine 锁内核验该用户；`ManagementOptions::default()` 的纯手动嵌入模式不创建 owner，永不据此签发内部复用资格。stop 经短控制状态立即撤销 owner，不等待 Engine；真实 I/O 仍须随后 drain/join。worker 退出或 panic 丢弃 lease，重开后必须替换 lease 并重新实际扫描；空闲 owner 只弱引用 backend，不增加 Engine/AuthStore 强计数，也不强留旧 State。

共享图故障由核心源归因撤销资格；版本本地 proof/正文失败仍使完整健康报告失败，但不能仅因 `metadata_failures` 总数就撤销全池共享图覆盖或封锁无关写入。服务层只保留这两个结论的区别，不从 public report、布尔配置或心跳伪造 proof。

## HTTP 上传与发布

以下为服务端 HTTP 内容接口的行为，不代表 Web 具有文件上传、下载或正文预览功能；Web 只提供文件元数据与管理操作，日常文件内容访问使用 SMB。

上传 URL 使用 `?path=业务相对路径`，必需声明 `X-ZXS-Logical-Size`、`X-ZXS-Content-Hash`（完整 BLAKE3）；`X-ZXS-Upload-Generation` 为正的十进制代际，省略时为 1。服务流式写入私有暂存目录并周期 fsync。中断和失败内容保留：同代际重新发送完整文件时必须匹配已持久前缀、长度和哈希；内容变更须显式提高代际。旧代际不会被新上传静默删除。当前不支持按偏移 PATCH/Range 上传。

上传进度使用 `AtomicFile::replace` 在同一持有的私有目录 fd 内写临时文件、同步文件、rename 并同步父目录；成功后不再额外同步同一父目录。正文检查点先同步数据，再替换进度，任一错误继续阻止新准入并保留已有材料供盘点。源码预算中，每次进度替换由三次同步降为两次（加前置正文同步为三次），不含首次目录、预算记录和其他发布 I/O；此计数不是系统调用实测或吞吐承诺。

HTTP 正文只合并已经就绪的帧：每批至多 256 KiB、64 帧；遇到 Pending 立即写出，不等待凑满。单个超出此上限的原始帧直接处理，不另外复制成大缓冲。批次在原有每累计 1 MiB 的检查点触发帧处停止，同代重试先核对实际暂存前缀（含未宣告持久的尾部）。数据 `sync_all`、进度记录原子替换和目录同步顺序不变；合并减少的是阻塞任务、写锁和写前空间检查次数，不减少持久性屏障。每个阻塞任务持有上传准入及暂存所有权至实际结束。写前余量不足时整批拒绝，实际留存尾部可能比逐帧写入短；所有实际落盘字节仍计费、留存并在重试时核对。可运行 `cargo test -p storage-server --lib upload_batching_ready_and_paced_measurement -- --ignored --nocapture` 测量就绪/间隔帧的路由及暂存路径（测试后端）；它不代表真实网络或存储引擎发布的吞吐提升。

初始化和恢复沿用统一引擎错误分类：确定冲突与结果不明分别表达，I/O 或不确定发布不能一律返回 409；内部路径与凭据信息继续遮蔽。HTTP/SMB 暂存锁和记录通过非阻塞、安全描述符读取，FIFO、链接或损坏记录不得令管理启动无限等待。

HTTP 断开不会取消已运行的阻塞发布或进度持久化。每个任务共享同一上传并发名额和文件写入占用，直到最终状态持久化完成；这期间同文件重试返回 `FILE_WRITE_BUSY`。请求体/空闲超时返回 408；已接纳不可取消发布后的等待超时返回 503，调用方须先查询原上传状态，不能推断尚未发布。Range 错误统一返回 `416` 和 `Content-Range: bytes */大小`，HEAD 响应不含正文；读租约随下载正文的消费或丢弃释放。

先调用 `seal` 冻结工作内容并取得提交模板，填入固定的 operation_id/message，再按 `storage_contract::request_digest` 计算完整请求摘要。原样保存请求后调用 commit；结果不确定时查询 operation 或原样重试。普通内容保存返回契约中的 SaveOperation/SaveResult，成功结果保留发布时保护证据，当前健康度单独计算。历史保存计划在单次核心事务中登记独立分支，只为实际写入的文件生成 FULL 版本，其余文件保留冻结父引用，原分支 HEAD 不变。

## 分支登记与首次写入

创建文件分支只持久化分支记录和冻结父引用，不复制正文、不创建占位文件、不增加文件版本。`copied_files`、`placeholder_files` 均为 `"0"`。目录分支直接引用父目录的完整子树，不提前登记后代文件分支；首次写入继承文件时才创建公共文件分支。创建不会自动选择新分支。

命名目录分支保持稳定身份；内容保存、子项分支切换及命名空间变化追加不可变目录子树。`namespace-batch` 显式列出每个作用分支及依据子树，整批检查并发布；移动保留文件身份并移交跟踪，复制产生独立文件，删除只移除选中分支条目，未选分支保留自己的冻结路径和节点。详细规则见[分支系统逻辑](../design/branch-system-logic.md)。

选中尚未写入的分支时，文件列表响应指向当前可见的父版本；通过保留的 HTTP 内容接口读取该 `version_id` 时返回父版本正文。响应中 `version_id` 和大小指向当前可见的父文件，同时用 `materialized:false`、`inherited:true`、`own_version_id:null`、`source_version_id` 明确表示该分支尚无自己的版本；`content_state:"PENDING"` 表示此归属状态，`visible_content_state` 是父版本实际的可用状态。首次成功写入后，`version_id` 与 `own_version_id` 指向新 FULL 版本，`materialized:true`、`inherited:false`。目录里未写入的文件继续显示各自父版本，已写入的文件显示自己的版本。

未发布的工作区、上传失败和写打开后直接关闭不会产生内容版本。`versions` 只列真实正文版本；统一历史图还列复用正文的路径节点，分支登记单独保留审计事件。底层 `tree` 继续使用严格的 `Entry` 合同，以真实可见版本 ID 和大小返回继承视图，顶层 `pending_file_versions` 标明哪些文件仍继承来源。登记时的历史快照在首次写入后仍显示其父内容，并保持继承标记，不会追填一个属于该分支的旧版本。

Samba 的只读打开取得父版本的只读描述符；写打开从当前可见父内容建立私有工作副本。实际写入或显式截断并成功发布后才生成自己的版本，包括有效的零字节内容。仅打开后关闭和放弃工作区不会将私有副本变成分支版本，父版本始终不可变。

## 静态资源与 Samba

静态目录支持 `.gz` 预压缩、WASM MIME、弱 ETag/304、CSP 和 nosniff；发布新静态文件后应重启服务以重建资源 ETag 表。目录不可读、不安全或缺少 `index.html` 时，静态页面返回 `503 WEB_ASSETS_UNAVAILABLE`，不阻止管理 HTTP API 启动；`GET /api/management/status` 的 `auxiliary.web_assets` 报告 `unavailable`。省略 `--web-dir` 时该状态为 `not_configured`，不会假定已提供 Web 页面。未知 API 不回退到网页。不要在静态目录保存认证或工作文件。

`/api` 和 `/api/` 下路径只属于 API：即便静态目录意外出现同名文件，也不返回静态正文或静态 304。未知 API 与缺失静态文件均返回 JSON `NOT_FOUND` 错误封装。

可选桥接参数为 `--smb-socket /srv/zxs/run/smb.sock --smb-peer-uid UID --smb-user SambaAccount`。`--smb-user` 只能设置一次，指定一个精确的 Samba 认证 account_name，不接受 `Samba=ZXS` 映射；该身份绑定到当前唯一 ZXS 管理员。两端用户名和密码可不同，Samba 凭据须独立配置。socket 父目录 0700、socket 0600，代理 UID 与真实 Samba 非匿名/非 guest 身份必须同时核验。详见[集成文档](smb.md)。

尚未提供 HTTP 线上拓扑迁移、换盘/排空/弹出、偏移分片上传、多段 Range 或任意服务端路径导出。`POST /api/admin/disks` 明确返回 409，不能在线修改配置草稿代替迁移。历史独立恢复包通过本机 CLI 操作。上传与 delete/rename 路径发生祖先重叠时会保留数据并拒绝提交，尚未实现任意交错的工作区事件重放。

## 验证

```sh
CARGO_TARGET_DIR=target/native CARGO_INCREMENTAL=0 \
  cargo test --locked --offline -p storage-server -p storage-client
```

Unix socket 测试需要运行环境允许本机 socket。[集成回归](../../test/server/integration/http_engine.rs) 覆盖 HTTP 准入、封存、三盘发布、幂等、Range、认证、历史计划和策略 CAS，也覆盖文件/目录分支、真实版本祖先、备注与重命名下载、上传池限额，以及改密的 CSRF、同源、字段和 JSON 长度边界。永久丢失后的降级发布与发布后损坏均保留原保护证据；可信嵌入式认证快照恢复测试仍保留，但不代表 daemon 在权威交接后允许自动恢复丢失的认证主库。管理运行时回归另外验证主库丢失时围栏拒绝旧快照、数据引擎阻塞时仍可登录，以及排队请求在注销或改密后被拒绝。两种保存结果 wire 均通过原 SaveOperation schema 验证。

模块回归还验证上传断开后的占用生命周期、Range/HEAD 及读租约、静态/API 命名空间、规范化账号限流、损坏上传状态和持久化失败清理。平台与共享认证回归使用 `cargo test --locked --offline -p authentication -p platform-io -p storage-server` 一并运行。

会话撤销回归固定“已取得身份、引擎尚未准入”的窗口，在注销或改密后才放行元数据、工作区、上传和读取计划，检查旧身份被拒绝且持久存储未发生变更；另覆盖旧账户 CRUD/ACL 路由消失、旧凭据格式拒绝及嵌入模式边界。SMB 独立验证当前唯一身份及共享绑定，不借 HTTP 会话或旧映射授予访问。

用户目录分支以稳定文件身份和精确节点组成冻结子树；登记目录分支直接引用父组成，文件首次实际写入时才登记公共文件分支。批量操作明确列出每个作用分支和基线子树，任一条件冲突整笔拒绝。已包含文件分支的历史域明确拒绝未适配的旧整树创建、历史保存和拆分。新版本正文使用真实文件容器，下载名称采用所选逻辑别名，布局与格式拒绝边界见[存储格式](../08-full-storage-and-disk-format.md)。

文件历史保留真实正文的 `versions`，统一图使用 `nodes`、`node_id` 与 `parent_node_ids`。文件或文件夹重命名为受影响文件创建路径节点并复用原正文；后续真实写入沿路径节点继续连线。路径节点备注与版本备注分开保存，分支来源可明确指向路径节点，下载仍使用真实 `version_id`。

`POST /api/file-manager/files/{file}/merge` 提交已审阅的参与分支 head、任意同文件结果节点、归属分支及 `confirmed:true`。参与 head 改变返回409；被合并分支保留冻结历史，转为只读别名。结果恰为参与 head 时复用节点，否则创建复用正文的 `fmeta_` 合并节点；独立来源记录保存全部输入，图边过滤成环关系。只推进参与引用，父目录冻结的旧子树在自己的组成上派生，不从子目录新 head 带入未参与文件。

`POST /api/file-manager/files/{file}/tags` 对显式节点与分支 head 基线应用标签增量，最多16项，每项128 UTF-8字节。变化生成不可变标签子节点，保留源标签；历史源不推进分支 head，当前精确引用才推进。标签不产生正文或完成首次正文写入。路径、标签、合并节点备注统一使用 `PATCH /files/{file}/nodes/{node}`，与正文备注独立。

`GET/PUT /api/repositories/{repo}/backup-configuration` 查询/配置稳定文件或目录 scope。写请求需要当前唯一管理员认证，绑定 `expected_scope_id` 与自身 `expected_generation`；继承策略的有效代数另以 `effective_generation` 返回。份数包含主盘；每个备份slot使用用户独立指定的逻辑路径前缀，相对所选scope根。配置回填可用历史正文且保持版本及proof ID。目录分支合并待用户确认，暂不实现，能力明确为false且无写端点。完整语义见[文件合并、标签和备份](../design/file-merge-tags-backup.md)。

保护规则必填 `inheritable`，现行规则为 `true`，文件绑定保留设置为 `false`。后者不参加 scope／祖先查找，GET 返回 `inherited:false`，但其份数和路径仍经文件绑定生效。重配只推进现行规则的原 ID／代数，将范围外绑定独立保留；父目录不能覆盖这些保留设置。通用策略 PUT 对稳定 scope 或保留设置返回 `400 BACKUP_CONFIGURATION_REQUIRED`，要求使用文件／目录备份配置，不通过通用入口绕过历史回填。

<a id="login-admission"></a>

### 反向代理登录限流

默认忽略 `X-Forwarded-For`，按实际 TCP 对端限流。若使用同机 TLS 反向代理，可显式增加 `--trusted-proxy-ip 127.0.0.1`（IPv6 代理则为 `--trusted-proxy-ip ::1`；可重复）。必须同时设置 `--external-origin https://storage.example.org`。仅接受指定的精确回环代理地址，不接受网段或非回环代理；监听地址仍必须是回环地址。这是可选部署配置，升级不会自动启用信任。

代理必须**覆盖**客户端传入的 `X-Forwarded-For`，仅写入它实际看到的客户端 IP，例如 nginx：`proxy_set_header X-Forwarded-For $remote_addr;`。禁止使用 `$proxy_add_x_forwarded_for` 或转发客户端原始头。来自受信代理的缺失、重复、列表、带端口或其他非单一 IP 值会被拒绝；未受信 TCP 对端的该头被忽略。`Forwarded`、`X-Real-IP` 和其他转发头不参与身份、安全 Origin 或 Cookie 策略。不要将来自不可信本地进程的请求混入受信代理来源；本机进程可使用同一回环源地址，信任本机是此部署模型的前提。多层代理需要在最后一跳安全解析上游来源后覆盖此头，不能直接传递头链。

登录保留每来源 IP 每 60 秒 10 次的上限；共享 NAT 的客户端仍共享来源限额。来源 IP + 用户名组合不再另建计数，因为同一来源的总上限已覆盖该组合。跨 IP 针对同一规范化用户名的尝试超过 10 次后，验证前逐步增加 250ms、500ms、1s、2s、4s 至最多 6s 的等待，而不是耗尽用户名额度后整分钟拒绝。账号压力在连续 60 秒无获准尝试后消退，持续攻击不能借固定窗口边界反复取得免费突发额度。

登录准入把三种可增长状态分开，各最多 4096 项：已尝试当前管理员名称的来源 IP、仅尝试未知名称的来源 IP、未知规范化用户名压力。唯一管理员的压力和待处理标记使用固定单例，不占未知名称容量，恢复替换名称也不重置该单例。未知名称来源表或名称表满时只拒绝该类新键，不淘汰未过期计数。某 IP 首次尝试管理员名称时，在所有容量/次数检查通过后，把已有来源记录移到管理员来源表，保留原计数和窗口起点；随后该 IP 的未知名称尝试也使用同一记录。因此交替名称不会获得两份 10 次额度，迁移不保留重复来源。来源窗口正常过期后才允许重新分类，管理员来源表本身满时仍明确返回 429。

每个规范化账号只允许一个正在等待或验证的请求，稳定持续猜测因而最多约 10 次/分钟；其他同账号并发请求立即返回繁忙 429。唯一管理员的待处理槽独立于最多 64 个未知名称待处理槽。取消等待或完成验证时释放；HTTP 在阻塞任务开始后取消，槽仍随实际工作存活。工作所有者仅通过原子标记释放槽，析构不获取准入锁；未知名称的已释放槽在后续未知名称准入时清理，保留记录仍最多 64 项，不淘汰活动槽或重置尝试计数。等待不占用全局四个密码哈希槽位，未知名称仍执行真实密码验证，但即使密码相同也不能取得会话。密码计算容量满时立即返回 429，不建立无限队列；登录与已认证改密共用此计算上限，但不共用尝试计数表或键容量。

分类与密码验证使用同一份已提交凭据挑战快照；发布会话前仍核对当前用户名、代数、密码哈希与恢复围栏。恢复或改密不能把旧的未知名称挑战变为已知账户证明，也不能让旧密码证明创建会话。无效、非 ASCII 或超过共享用户名策略长度上限的用户名，在进入待处理集合和计数表之前即返回 401，不保留原始输入作为键。准入更新在同一互斥锁下完成；容量或来源次数拒绝不增加账号压力，不重置既有计数。

这只隔离不同键空间的容量，不保证分布式恶意流量下的可用性：攻击者直接使用已知管理员名称，仍可能占用管理员待处理槽、耗尽其来源表，或造成最长 6s 等待；未知名称的真实哈希也会竞争四个计算槽。容量分区在压力下会产生可观察的已知/未知候选名拒绝差异；真实密码验证不等于整个准入流程等时或不可枚举。错误不回显当前管理员名称、密码或哈希，但不能据此声称分类不可推断。公网部署还应在代理层实施总体请求/连接限制。没有新增按用户配置，不修改 Origin/Cookie 安全策略、现有账号、凭据或数据库格式。

<a id="http-spool-budget"></a>

## HTTP 暂存预算与恢复

HTTP spool 必须由服务身份拥有且权限为 `0700`；现有目录不会被自动 chmod 或接管。整个路径以目录描述符逐级验证：允许 root 或服务身份拥有的普通只读祖先，以及可信 sticky 共享父目录（如标准 `/tmp`），拒绝其他用户所有的祖先、非 sticky 的组/全局可写祖先和符号链接。缺失的私有组件以描述符相对方式创建。上传文件与进度记录在已持有的目录下创建/发布，提交前检查路径身份；后端只读取已经打开的源 FD，并在接受工作区清单前核对实际副本的长度及 HTTP 网络流摘要，不重新按源路径读取。

`api-spool` 使用独立的 `SpoolBudget`，不把单次上传长度或 2 个并发许可当作累计容量限制。默认每账户 100 GiB、整个 HTTP spool 500 GiB，并保留所在文件系统 1 GiB 空闲恢复余量；这些额度不包含核心工作区或 SMB 暂存，不能相加后仍宣称是统一全池预算。

守护进程通过一个有所有者的一次性阻塞任务独占 `.budget.lock`，并盘点 active/retained/owner 记录和数据文件；管理 HTTP/Web 不等待该任务完成。`auxiliary.http_uploads` 在完整清点结果发布前为 `pending`，成功后为 `ready`，失败后为 `unavailable`，不在后台自动重试。`pending` 期间上传及上传状态仍先经过原有认证/CSRF 检查，再返回 `503 HTTP_UPLOADS_PENDING`；没有部分预算或无预算回退。同步嵌入入口 `router` 明确完成同一清点后返回，不改变其同步构造语义。已完成、失败、旧代际及未知孤儿文件继续计费，缺失或无法归属的记录不得获得免费容量；无法确认计费的 I/O 结果会阻止新准入，直到重启盘点。每份数据另计 64 KiB 保守元数据额度，超额的已有元数据按实际长度计入。这是文件长度和保守开销账本，不声称等于文件系统实际分配块数。

停止服务会丢弃异步清点结果发布者，不安装迟到的就绪结果；已经开始的阻塞清点仍由 Tokio 运行时退出等待。完成结果被丢弃时正常释放其锁，不删除留存材料。此行为不构成底层 I/O 的时限保证，也不把其他同步启动步骤改为后台任务。

接收 body 前原子预留账户及全局额度。所有后台写入/同步/记录发布与 reservation 共享生命期，取消等待不提前归还仍在执行的容量。同代际重发只验证已有物理前缀并续写，不复制第二份正文、不覆盖未宣告为 durable 的尾部；新代际保留旧字节。请求结束按实际仍保留文件长度结算，而不是按成功与否释放。 Reservation 持有准入时的计费上界和元数据额度，析构不再回查该不变量。若账本锁已因 panic 中毒，析构记录错误并保留中毒及占用状态，不产生二次 panic 或恢复准入；重启仍从磁盘盘点。

目前没有自动删除未决材料或按 TTL 回收历史的功能；达到预算会拒绝新增上传。已结算 API 副本的安全回收/显式放弃入口仍需单独设计，不能手动删除活动记录来规避账本。操作系统的其他写入者可能消耗同一文件系统空间，余量检查并不构成内核级独占预留。

运维入口：`zxs --http-spool-bytes-per-account ... --http-spool-bytes-total ... --http-spool-recovery-reserve-bytes ...`。单文件的 `--max-upload-bytes` 与上述累计限制独立；预算参数必须为正、总额不低于账户额、账户额至少容纳元数据额度。

<a id="http-transfer-limits"></a>

## HTTP 并发准入与传输期限

上传和读取使用两个独立的准入池，读取包含版本正文、工作区正文和校验清单。默认每池最多 2 个活动操作，唯一管理员的全部会话共享对应池；上传占满不会占用读取名额。池满时立即返回 429，不建立等待队列。账户分桶已移除，默认有效上限不变。

`--http-transfer-slots` 默认为 `2`，必须大于零；`--http-transfer-idle-seconds`（60）与 `--http-transfer-total-seconds`（3600）要求 `0 < idle <= total <= 604800`。三项映射至 `ServerConfig.http_transfer_limits`，仅影响 HTTP，不改变 SMB 预算或超时。旧 `--http-transfer-slots-total` 和 `--http-transfer-slots-per-account` 参数明确拒绝，不作为别名接受。

总期限从进入已认证的传输处理器开始，包含准入后的授权、打开、暂存、发布和流式读取。上传 idle 按接收非空 body 块重置；空块不能续期。下载 idle 按 HTTP 层取走非空正文块重置，不代表远端已确认接收；即使 consumer 停止轮询 body，独立计时器仍会终止旧流并释放空闲读租约。总期限不能被持续小流量重置。响应头发出前超时返回 408 `TRANSFER_TIMEOUT`；已经开始的二进制响应终止为流错误，不追加 JSON，旧 body 恢复轮询也不能继续读取。

期限取消的是 HTTP 等待，不是强制取消同步磁盘 I/O。已经启动的阻塞校验、回退读取、写入、同步或发布任务继续持有对应传输池许可及读租约、spool reservation、条目 claim，直至任务真正结束，因此卡死的底层 I/O 仍可能占用对应传输池的额度。不会通过超时提前释放正在使用的文件或宣称发布失败；客户端应查询上传状态后重试。空闲超时尽可能持久化已有前缀；总期限/断开可在持久化前到达，已写物理尾部及旧记录仍保留，后续重发必须验证原字节。不会删除失败上传、未决操作或未知文件，累计 spool 预算仍计费。

### 普通 HTTP 后端任务准入

普通 HTTP 元数据和文件管理后端调用使用独立的 8 槽准入，每个 router state 及其克隆共享。额度耗尽时立即返回 HTTP 429 `RESOURCE_LIMIT_EXCEEDED`，标记 `retryable:true`，不建立应用等待队列。许可归阻塞任务所有，覆盖 Tokio 队列等待及实际执行；HTTP 请求取消后仍保留，直到任务完成、报错或展开退出。这是保守的资源安全上限，不是吞吐保证，也不复用传输配置。

`/readyz` 和 `/api/capabilities` 使用独立的管理池，饱和时可返回 429；`/healthz` 不使用阻塞池准入。上传和正文读取保持独立准入，避免嵌套获取同一许可。认证、会话任务和遥测不受普通后端池控制；它不是进程级后端预算、跨 HTTP/SMB 的磁盘预算或 SMB 文件描述符路径限制。各路由维持原有 body／字段解析、获取许可和当前身份复核的先后顺序；该边界不限制所有连接或请求体，也不增加缓存。

已认证后端工作由 server 私有 `HttpBackendJob` 统一持有 Backend、AuthStore 强所有者及对应许可。消耗型执行入口在返回等待 future 前提交 blocking job；HTTP 取消、排队或超时不会提前释放任务所有权。原始 Semaphore、传输 Permit 不依赖认证；管理请求也不额外取得或保留内部 Engine。下载完成初始授权／打开后只把 read permit 转入 Content lease 或 manifest body，AuthStore 留在已完成的 worker 中释放，不随不可变响应继续保活。已验证内容块的后续 I/O、静态响应、公开 readiness/capabilities 和 spool 盘点不增加认证 owner。迁移范围与待执行验证见[HTTP 任务所有权验收](../validation/http-job-owner.md)。

### 已提交鉴权快照

HTTP token 检查、授权后端和 managed 引擎准入读取[已提交鉴权快照](../architecture/auth-committed-snapshots.md)，不等待凭据写序列的 fsync。读写请求仍在引擎锁后复核当前会话，保留注销、改密、到期及不确定权威故障关闭；密码哈希与持久写仍有各自的有界阻塞任务准入。

### 文件历史有界读边界

文件历史按 [ADR 0009](../adr/0009-bounded-file-history-windows.md) 只提供有界页：主节点页
`GET /api/file-manager/files/{file}/history` 及其 `nodes/{node}/parents`、
`nodes/{node}/fields/{field}`、`branches`、`merges`、`merges/{merge}/inputs`、
`merges/{merge}/parents`，以及高级正文版本页 `GET /api/files/{file}/history` 和
`versions/{version}/sources`。路由处理在 `http_file_history.rs`，请求类型、游标和 wire
投影在 `backend_file_history.rs` 及其 `cursors`／`wire` 子模块，引擎后端读取在
`engine_backend/file_manager/history_pages.rs`，选页由引擎 `business/file_history_pages.rs`
完成。没有无参数整包分支，也没有第二个整包 HTTP 出口。

- **请求边界**：原始查询在 percent 解码及 Map 分配前限制为 128 KiB／8 个字段，重复字段在提取时拒绝。
  每个路由只接受登记的查询字段，未知字段与越界 limit/offset 在调用 Backend 前拒绝；游标解码为对应集合的类型、解析全部字段并
  绑定路径中的文件，跨文件游标是 `400`。中间件先认证，AuthorizedBackend 和 Engine 锁内
  再检查当前授权；阻塞任务独立持有准入许可与认证所有者。
- **锁内**：解析文件归属与当前上下文、取得精确历史根并与请求观察到的根比较
  （不同为 `409 HISTORY_CHANGED`），然后按稳定键选页，只复制本页记录。授权失败先于根比较，
  不以冲突暴露未授权对象的变化。
- **锁外**：已选记录按遍历方向逐条受限编码，在写入前检查余量，按 `MAX_HISTORY_PAGE_BYTES` 预留外壳与游标空间准入，
  超额记录留给下一页；反向取最近的规范后缀。响应由已编码字节直接拼接，只有边标志改变的本页记录重新编码。字节截断后父边的
  `in_window` 按实际驻留节点重新判定（每次翻转只多一个字节，已在预留中计入）。
- **发送所有权**：同一普通 backend slot 覆盖捕获、编码、Body 与最后一个传输拥有的 `Bytes::from_owner` 块；每块最多 64 KiB，Body EOF 不提前归还许可。
  `serve_with_grace` 注入准确 socket 能力及原有 peer ConnectInfo，`DrainStream` 的实际 `poll_write`／`poll_write_vectored` 成功推进空闲时钟；租约保留既有 idle／total 期限，停滞时关闭该 socket，最后块正常释放则撤销定时器，不关闭健康 keepalive。
  此证明依赖当前产品唯一传输：Tokio TCP 支持 vectored write，默认 HTTP/1 builder 未启用 pipeline flush 或 writev=false，因此使用保留块所有者的 Queue；本地 transport 未启用 HTTP/2。未来增加 Flatten、TLS 包装或其他协议时必须重证实际 flush 所有权，不能只复用 Body 结论。无 socket 的嵌入式 Router 使用 Body 进度期限。
- **类型与根**：内部信任边界产生的请求／游标；仅重新核对动态授权、精确根及真实边界成员。父预览游标特许改变合法页大小，其余集合绑定页大小；未提供根的精确节点跳转拒绝。仅本地计划安装保留准确已发布目录绑定，任意 State 更换不能获此特许。

测试入口：`backend_file_history` 的请求、游标与 wire 单测（含最大合法记录、截断、共享
Web 窗口夹具 `test/fixtures/file-history-window.json`）、`engine_backend/file_manager`
的页投影与授权矩阵、`http_shutdown` 的实际非读取 TCP／健康 keepalive 租约回归、真实 HTTP `history_query`／`history_pages` 集成，以及
`authorized_backend` 的转发与撤权矩阵。源代码接线不等于这些测试已经执行；执行结果由
对应验证提交记录。

### 活动历史投影边界

`GET /api/file-manager/history` 直接调用 `Backend::activity_history`，不经过 JSON action 分派。HTTP 仍解析 query，但不以额外 query 字段改变业务结果；当前凭据复核在排队后的 Backend/Engine 准入发生。阻塞任务只持有 Backend、认证所有者和准入许可，取消 HTTP future 不提前释放后台工作所需资源。

快照保留数值 sequence/UTC 秒和明确的行详情类型；响应边界再将 sequence 编为十进制文本、时间编为 UTC。开放的持久 `kind`/action 仍是字符串，不按当前观察值封闭。提交删除事件仍以 commit ID、删除 ordinal 区分重复路径；同序号按 event ID 排序，最多 200 行，不增加跨事务缓存。

`OperationResult.result` 仍是持久化开放 JSON：活动投影只解析所消费的可选 timestamp、ID、复制数字段；缺失/null 保持缺失，格式错误返回保留来源的 `STATE_INVALID`，不使用默认值或 panic。此项明确收紧了畸形持久 receipt 字段的响应行为，正常 wire 不变；被最新 200 行淘汰的候选不因其字段错误使查询失败。备份 receipt 缺失必要 history identity 同样作为持久状态错误。现有合法缺省（未找到当前路径、备注/名称缺失、无操作所有者、已不存在目录分支等）保持原投影规则。快照只复制实际响应所需数据；正文状态复用已有 typed content 投影，最终 JSON 只在 HTTP 序列化发生。
### 计划查询与取消的类型边界

`GET /api/plans/{plan}` 与 `POST /api/plans/{plan}/cancel` 使用独立 typed Backend 方法，不经过通用 JSON action。GET 仍接收但不消费 query；输出保持 Plan 的 `expected_sequence` 十进制文本、原始 input 和状态。取消返回原始 `OperationResult` 永久收据，不转换为文件管理接口的 `operation_kind`／实时健康度信封。

HTTP 一次解析 body、路径绑定和取消字段；为保持原错误优先级，携带解析结果而非伪造有效命令：当前身份准入后传播未知字段错误，计划加载、owner 和历史仓库准入后才传播 operation_id 的解析错误。成功的 operation_id 是约束类型，内部不再解析。阻塞任务持有并发许可和认证所有者，HTTP 等待取消不提前释放。

计划磁盘文件仍是外部边界：受限读取、strict JSON、plan_id 与 input 摘要核验不变。当前 Plan/input 仍承担持久化表示，input 以及取消收据 result 保持不透明 Value；本次只分离实际 HTTP 投影和调用入口，不声称完成全部领域／持久化模型拆分，也不改变计划提交、cleanup 恢复或三类准备协议。

计划提交也使用独立 typed Backend 方法。其阶段与取消不同：当前身份之后先拒绝缺失／非字符串／空的 operation_id、plan_digest，再拒绝未知字段；随后加载并验证计划、核对 owner、预览 digest（split 保留独立错误码）、历史仓库，最后传播 operation_id 格式错误。HTTP 解析结果只保留原阶段顺序，核心只收到成功 OperationId；没有进程内 JSON 组装后反序列化命令。

Web 在用户明确确认预览后才构建提交请求；服务端以 plan_digest 绑定预览，并不接受 `confirmed` 字段。提交仍进入核心原流程：已发布的 purge／split receipt 不能证明物理清理完成，相同请求必须继续 cleanup；失败保留原收据和可重试的清理依据。重启恢复也执行原清理流程，不因为 typed 接口缓存计划、权限或返回已有收据而跳过磁盘检查。
