# 读取投影与请求内上下文

目录树读取先在引擎锁内捕获独立响应，再解锁执行序列化。响应不保留引擎引用，后续状态变化不会修改已经捕获的树。

## 实现边界

- [Tree 投影](../../src/server/src/engine_backend/read_projection.rs) 为一次请求建立来源解析上下文，而不是为每个条目重新解析分支、提交和仓库。
- [来源解析](../../src/server/src/engine_backend/wire.rs) 对已验证的规范提交使用已知条目，避免再次扫描相同的提交条目表。
- 外来条目、重复键和不规范排序继续走检查路径，保留原有首个匹配项语义，不能将“通常唯一”当作已经验证的前提。
- [文件上下文](../../src/engine/src/business/directory.rs) 复用相同的上下文构造逻辑；文件管理列表每行只解析一次上下文。
- 这些对象只在当前请求内使用，不构成跨请求的认证或业务状态缓存，也不绕过历史、分支归属或访问检查。

## 复杂度口径

[回归测试](../../test/server/unit/engine_backend/read_projection/tests.rs) 比较完整响应和参考检查路径，覆盖不同列表规模并统计重复匹配访问。已验证条目的直接路径不重复扫描提交条目表。

这不是整个响应生成为 O(N) 的声明：规范性验证仍有线性成本，映射和历史上下文可能有对数成本，响应构造及序列化仍需要处理输出数据。不规范输入的保守检查路径也不享有同样的复杂度保证。工作量计数不等于吞吐量、延迟或内存占用的实测提升。

## 保留的行为

当前与历史提交、主分支与文件分支、继承上下文、待写入内容、缺失或重复条目、冻结响应和解锁后序列化均有回归覆盖。现有 HTTP 测试继续覆盖独立文件分支、共享目录组合、移动别名以及幂等命名空间批处理。

## SMB 类型化读取边界

SMB Stat/Open 的当前条目查询通过 `Backend::lookup_current(&Identity, branch, &LookupPath)` 返回 `Option<Entry>`；已有的 `lookup_at` 与 `snapshot_at` 分别用于冻结提交查询和共享不可变提交。`Session::find` 不再经过 `Backend::call`，也不再把 Entry 编码为 Value 后反序列化回来。Open 复用在 RPC 入口解析的 `LookupPath`，Stat 在入口解析一次；命中按已排序的规范命名空间 key 二分查找，复制的只是单个 Entry。没有缓存身份、绑定或当前 HEAD。

已打开的写句柄和目录句柄使用 `Backend::authorize_current` 返回 `Result<()>`，不构造固定 JSON 确认、不克隆条目或计算 pending 映射。RPC 响应边界仍返回 `{"authorized":true}`。只读文件句柄仍走 `Version` 授权冻结版本；工作区期限及写选择授权没有移入这个窄读取检查。

目录捕获／分页通过 `ReadDirectory` 与 `DirectorySelection::{Capture, Page}` 调用 `Backend::read_directory`，返回 `DirectorySnapshot`。目录句柄保存 `Option<PortablePath>`（`None` 是合成根），不再存裸路径并于每页重新检查路径格式。引擎仍核对目录 entry 身份、种类、提交归属，保持打开时冻结的提交；restart 重新选择当前 HEAD。游标由服务器句柄持有，外部 RPC 仅提交 handle/restart，不能指定 commit、after 或路径。会话仅在实际 RPC 输出时将 `DirectoryPage` 序列化，不再操作 Value 字段或反序列化自己的分页结果。分页的条目数和保守字节预算不变，字节预算测量仍属于此 SMB 投影的传输约束，不是领域调用。

三条旧 `BackendAction`（`SmbLookup`、`SmbAuthorizeRead`、`SmbReadDirectory`）及其 JSON 分派已删除，而非保留平行兼容路径。`AuthorizedBackend` 和 `ManagementBackend` 显式转发三种 typed 方法；引擎获取同一准入锁后刷新当前唯一身份、会话、共享启用状态、主分支、绑定代次，再检查分支／历史／仓库与提交。类型化不授予长期权限，也不改变撤权线性化。真实 SMB 使用当前唯一账户、没有 HTTP 会话令牌，不能与带会话内部调用混为同一种撤销模型。

[读取回归](../../test/server/unit/engine_backend/read_projection/tests.rs) 覆盖 Unicode 等价 key、持久化重开、当前与冻结查询、目录字节／数量上限、分页／restart、迁移分支和身份拒绝。[授权回归](../../test/server/unit/engine_backend/read_projection/authorization_tests.rs) 保留旧 Tree 元数据判定的独立参考；[撤权矩阵](../../test/server/unit/engine_backend/revocation_tests.rs) 覆盖外层认证返回后、引擎准入前完成撤销的交错。JSON 缺字段／多字段测试不再用于已经删除的纯内部请求，实际 RPC 请求解析仍在协议边界。复杂度计数不是吞吐、延迟或总分配量测量，真实 AF_UNIX / SDK 验证与这些直接后端测试分别记录。

## 写选择准入的接续迁移

SMB PrepareWrite / Dirty 进一步使用 `Backend::authorize_workspace_selection`，直接借用写句柄已经验证的 `PortablePath`，返回 `Result<()>`；`WorkspaceSelectionAuthorize` 的 JSON action 已删除。引擎同一锁内仍刷新当前身份、绑定、工作区所有者和分支，再检查当前目录／文件选择是否与工作区捕获的目标一致，包括已成功发布的工作区收据。`Engine::validate_workspace_file_selection` 接收类型化路径并复用其 key，借用 `workspace_ref` 而非克隆整个工作区；不改变选择代次、首次发布后的目标判断、失败时保留脏数据或永久幂等行为。

## HTTP / SMB 工作区创建与销毁

创建通过 `Backend::create_workspace(CreateWorkspace)` 直接返回领域 Workspace，销毁通过 `abort_workspace(workspace_id)` 返回 `()`。HTTP 与 SMB 不再走 `CreateWorkspace` / `AbortWorkspace` actions，两个变体与引擎 JSON 分派已删除。SMB 的创建结果没有 Workspace → Value → Workspace 往返，准备失败、命名空间失败及 Close 的清理仍调用同一销毁语义，并保持原先“不覆盖首要错误、未决记录保留”的处理。

[HTTP 入口](../../src/server/src/http_workspace.rs) 在认证中间件之后执行有界 strict JSON 解码，拒绝重复键、非对象、未知字段、空身份和显式 null 的可选 source；仅字段缺省才选当前 HEAD。Abort 的 path/body 身份必须一致。DTO 解析发生在预算槽和引擎准入之前；格式错误在此阶段拒绝，解析后请求才参与当前授权与业务错误判定。`backend_slots` permit 随阻塞任务持有，HTTP future 被取消不会提前释放预算；任务完成或错误后才归还。

引擎同一锁内刷新账户／会话／绑定。创建继续先核对分支归属，再由原核心执行只读状态、工作区数量、分支状态、源提交归属和持久化；历史与当前创建使用原核心各自的预算及错误顺序。Abort 继续先核对当前主体和工作区所有者／分支，再进入原核心未决决定检查及落盘。两操作原本没有 operation_id，此迁移不新增幂等承诺，也不改重复 abort 的 fencing 行为、永久操作收据或不确定提交决策。

[WorkspaceResponse / StagedFileResponse](../../src/server/src/backend_workspace.rs) 是显式 HTTP wire DTO：穷尽解构领域记录，直接移动公开字段，StagedFile 的本机 `source_path` 不存在于响应类型中；保留当前 u64 文本编码、空 map 省略规则和 state 字符串。响应不再先编码领域 Workspace 后递归删除路径字段。新增字段使穷尽解构报编译错误，需显式决定是否进入 wire。这里只拆开这些 HTTP 响应，未改持久化 Workspace 或宣称完整 model/persist/wire 分离。

[wire 固定投影测试](../../test/server/unit/backend_workspace.rs) 对含本机暂存路径的完整工作区比较当前字段形状和大整数文本；[HTTP 回归](../../test/server/integration/http_engine/optional_identities.rs) 覆盖严格请求、历史源、预算释放及 abort 响应；[任务准入测试](../../test/server/unit/backend_admission.rs) 验证两个 typed handler 在取消后仍持有槽位。撤权矩阵与 SMB 故障代理已迁到真实 typed 方法，不通过旧 action 兼容层维持测试。

## HTTP / SMB 上传准入

HTTP 流式上传与 SMB Dirty 改为 `Backend::authorize_upload(identity, workspace, Option<u64>) -> Result<()>`；`UploadAuthorize` action 及 JSON 分派已删除。HTTP 直接传入已经解析的长度，不再格式化十进制字符串后在后端重解析；SMB 没有声明长度时传 `None`。授权包装器显式转发，引擎在原准入锁内刷新身份与绑定，再依次检查工作区存在／所有权、Open 状态、租期和声明大小，保留原有错误优先级。后续实际发布仍执行自己的当前准入与容量检查，这不是可长期复用的授权票据。

原 `item_id` 在该 action 中只做格式验证，没有参与领域对象查找或授权，所以不保留为无消费者的 typed 参数。HTTP 在原路径、长度／generation／hash 头校验之后验证 entry ID，且在读取 body、创建 spool 或调用后端之前拒绝非法值。外层身份认证、上传预算槽和逐 item claim 仍在上述解析之前；因此资源忙错误仍先于格式错误，而非法 entry ID 现在先于工作区领域错误。SMB writer 的 item ID 来自核心创建的记录，不重复解析自己的值。`UploadAdmission` 仍随阻塞授权任务持有，取消 HTTP future 不提前释放槽位或 claim；上传重试、永久操作收据和 canonical 编码没有改变。

[准入回归](../../test/server/unit/engine_backend/authorization_tests.rs) 用持久化重开后的过期工作区验证 owner/state/lease/size 错误顺序；[HTTP 流式回归](../../test/server/unit/tests/spool_budget.rs) 验证非法 entry ID 不读取 body、不发布 spool，且失败后预算可复用。已有上传重试、超时、HTTP 引擎、SMB 和撤权矩阵覆盖真实 typed 方法。

## 工作区查询与上传状态快照

HTTP Workspace GET 使用 `Backend::workspace` 捕获当前已授权领域快照，再复用 `WorkspaceResponse` 显式屏蔽本机路径；仍执行原 query 边界解析和 backend_slots 准入，取消 future 后阻塞任务保留槽位。捕获复制的 Workspace 不是共享可变授权状态。

上传状态使用一次 `published_upload_status` 调用代替 `WorkspaceAuthorize` / `UploadStatus` 两次准入。引擎在同一锁内刷新身份／绑定、检查工作区所有者并捕获 `PublishedUploadStatus`：工作区不存在／拒绝访问返回原错误，只有已授权工作区缺少指定 item 返回 `Ok(None)`。generation/size 直接使用 u64，Complete 与携带 manifest 的 Sealed 是互斥内部状态；HTTP 不再对自己的 Value 做 get/as_str/parse/.ok，也不把解析失败伪装成缺失。HTTP 最后才构建原 wire 字段、大整数文本、durable_ranges 和可空 manifest。

原两次相邻准入的第二次状态捕获合为一次锁内授权与捕获；撤权先完成时仍拒绝，不缓存权限，也不把锁扩到本机 spool I/O。HTTP 的 spool 可用性、read_slots、backend_slots、阻塞任务持有配额及本机 spool 所有者检查顺序不变。本机代次高于引擎或引擎 item 缺失时选择本机状态，否则选择引擎（相等也优先引擎）；两边都缺失才在 HTTP 返回原 item NotFound。不存在的工作区不得被本机文件掩盖，其他引擎错误不进入本机 fallback。

三条旧 actions 已删除。授权／撤权矩阵覆盖新方法；预算取消测试覆盖 GET 与状态读取；现有真实 HTTP 回归覆盖新代次未完成、相等代次完成／sealed 以及无本机 spool 的跨入口上传。typed wire 测试覆盖 u64 最大值、零长度和 sealed manifest；这些测试不等于真实 AF_UNIX / SDK 验证。

## 上传发布结果

`Backend::upload` 与 `upload_generation` 直接返回已完成的 `StagedFile`，`finish_authorized_upload` 不再先序列化、递归删除 source_path 再返回 Value。SMB 新建文件 Open 直接取得 `staged.item_id`，Flush 不为丢弃结果生成 JSON；HTTP 最后投影为现有显式 `StagedFileResponse`，公开结构不含本机 source_path，保留 size/dirty_generation 文本及可空 base_version_id。该类型也用于 WorkspaceResponse 内嵌文件，两处 wire 规则不再重复。

prepare 锁内所有者／代次检查、锁外完整副本验证、finish 回锁撤权与工作区检查均未改。HTTP 阻塞发布仍持有上传 admission/claim，结果返回前先持久写入 complete 状态；取消、超时未知结果、同代次重试和永久操作收据行为不变。`upload_generation` 不再有忽略 generation 的 trait 默认实现，production 原来已有的三个显式实现继续转发精确代次；测试代理也必须显式选择支持或拒绝。此组消除真实结果往返，但没有删除 BackendAction 变体，库存数字不因此虚减。

独立 wire 测试覆盖本机路径脱敏与 u64::MAX；真实 HTTP 上传辅助函数逐次断言完整字段集和整数编码。现有 SMB 句柄身份稳定、重试／丢回复和上传后回锁撤权测试继续覆盖真实方法。

## 工作区命名空间暂存

`Backend::stage_namespace(StageNamespace)` 消费已经区分创建名／查找名的 `NamespaceOperation`，返回领域 Workspace；旧 `Namespace` action 与引擎 JSON 分派删除。HTTP bounded strict JSON／path-body 绑定后，在持有 backend_slots 的阻塞任务中解析 kind/path/可空 destination，再进入当前授权；未知字段、重复键、非字符串 destination 与命名错误仍拒绝并保留具体来源。格式错误在 Backend 当前准入前，工作区所有权、分支归属与业务检查顺序不变。HTTP 复用显式 WorkspaceResponse，不重新序列化领域记录再删本机字段。

SMB RPC 入口一次解析路径并直接构造同一类型，替换目标删除也由已有 PortablePath 转 LookupPath，不再 JSON 编码后重新解析。原 source→destination 格式检查顺序、operation 保留／recovery record、预算／路径锁、冻结工作区基线、replace 先 delete 再 rename、提交标记及失败 abort 清理顺序不变；永久幂等提交仍走已有 typed commit_workspace，不把 namespace 暂存伪称为可重放操作。SMB 丢弃 Workspace 结果时不构造 JSON。

故障代理在真实 stage_namespace 方法注入，不再拦截已经删除的 action。授权／撤权、取消预算、公开路由 panic 矩阵同步扩展到真实方法；HTTP 新回归验证非法 envelope 不暂存、重复键拒绝及 Unicode 创建／查找拼写，SMB 原 rename/replace/丢回复/预提交失败矩阵继续适用。

## 剩余 Value 调用清单

此迁移没有覆盖所有应用接口。[BackendAction](../../src/server/src/backend_action.rs) 仍有 53 个变体；它是准确动作库存，下面按实际消费者列出后续范围，不表示每个动作存在同样的往返成本：

- **工作区 seal／公开发布已接续迁移。** 见[公开提交边界](detached-workspace-commits.md#public-commit-envelope-and-seal)；HTTP 与 SMB 保留各自摘要契约。上传 publication 结果也已接续类型化；namespace 暂存也已接续迁移；write-session 已接续迁移，通用读取仍有 Value，不能据当前迁移宣称全部工作区接口已经分层。
- **SMB 绑定／只读冻结版本。** Capabilities、Repository 与冻结版本已使用 typed snapshot/准入，生产 Session::call 已删除。不是本次 typed 当前查询的后备路径。
- **HTTP 通用读取。** resolve/history/commit/graph/events/compare、目录分支及选择、拓扑／放置／策略／磁盘／健康查询，仍由 HTTP 通用分派调用 `Backend::call`。
- **HTTP 写入与计划。** 仓库／分支／目录分支创建和选择、SMB binding 配置、policy/placement，以及 split/purge/historical-save/plan 的请求与结果仍需逐组迁移。operation HTTP 入口已接续到既有 typed 方法；其余 action 仍需逐项迁移。
- **文件管理与管理控制面。** `FileManager*` action、setup_operation 等仍有独立 Value 协议；management_status/retry_storage 已接续类型化；需要核对每个 HTTP 消费者，不能以新增一个包裹枚举代替拆除旧 JSON 路径。

这次不改领域 `OperationResult.result`、Plan 输入、持久化编码、canonical 哈希、永久幂等收据或公开 `Commit.kind`。当前 `core.schema.json` 的 `$defs.Commit.properties.kind` 是开放 string，不可仅按内部已见字面量将它收窄为封闭枚举。后续 model/persist/wire 分离应独立建立固定向量和公开消费者证据。


## 分支写会话

HTTP `/api/branches/{branch}/write-sessions` 在 bounded strict JSON/path-body 绑定后，持有 backend_slots 的阻塞任务解析只含 branch_id 的 DTO；Backend::create_write_session 接受已解析分支文本并返回 WorkspaceWriteSession，不保留 WriteSession action 或内部 JSON 路径。当前身份复核、分支仓库可用性、工作区创建和 expires_at 捕获均在同一引擎锁内；没有调用 create_workspace 后再次读取配置的准入窗口。格式错误在当前 Backend 准入前拒绝，撤权及领域创建/预算错误顺序不变。

响应显式映射 write_session_id/workspace_id、scope 和字符串 u64；内部租期保持 u64，保留现有饱和到期时间规则，不返回任何本机路径。会话创建仍是非幂等工作区分配，不伪造永久操作收据；后续发布的永久幂等契约不变。FileManagerWriteSession 是不同入口，仍在剩余清单。


## 冻结文件版本元数据与读句柄准入

HTTP `GET /api/versions/{version}` 通过 required `Backend::version_snapshot` 获取同一锁内的 FileVersion 与 ContentState。响应字段由 VersionResponse 穷尽解构，保留原字符串 u64、所有公开版本字段与 AVAILABLE/PURGED/MIGRATED token；不把本机路径混入元数据。query 继续由 ApiQuery<BTreeMap<String,String>> 在入口解析并允许原有未使用参数。稀疏 content_states 中不存在的记录明确表示 Available（与核心读及原 wire 一致），不是授权失败或缺失版本的默认值。

SMB 冻结只读句柄走 required `authorize_version`，同锁重新验证身份／绑定、版本存在与历史仓库归属后返回 unit；不复制 FileVersion，也不构造随后丢弃的 JSON。写句柄与目录仍走 authorize_current；不存在的 handle 不调用任一后台准入。Version action 删除，Placement 使用相同的借用版本归属解析，保持版本不存在→历史/仓库→placement不存在的错误顺序。版本元数据只捕获一次领域记录、HTTP 出口序列化一次，存储与永久幂等格式不变。

公开路由 panic、取消预算、授权装饰器、引擎排队撤权及 SMB handle 三种类型的调度日志均覆盖真实 typed 方法；未把默认 unsupported 或 generic call 当作迁移后的验证。固定 wire 数值/状态向量和真实 HTTP 对照磁盘 version.json 验证当前字段契约。


## 仓库快照与 SMB 绑定

HTTP 单仓库读取通过 repository_snapshot 返回同锁捕获的领域 Repository，显式 RepositoryResponse 穷尽投影，保留 binding generation 字符串 u64 及 file_manager_root=false 省略规则。原 ApiPath/ApiQuery 行为、backend_slots 持有至阻塞任务完成、当前账户与绑定复核及仓库不存在错误不变；Repository action 删除。

SMB hello 保留 sole_identity→SessionPermit→仓库快照→enabled检查→捕获 branch/generation→Capabilities 的顺序，直接消费 bool/u64/String，不再 Value.get 后解析内部响应。之后 synthetic root Stat/Open 使用 authorize_repository：同一引擎锁内复核当前身份/绑定 generation 与仓库存在，只返回 unit。hello 快照不授予后续永久访问；显式 disable/rebind 仍使已连接句柄在下次准入拒绝。生产 Session::call 已无调用者并删除，不留下旧 JSON 后备路径。

Capabilities 的匿名例外、管理层未就绪缓存/运行状态及 max_upload_bytes 已在下一组接续类型化，见下节。SMB 的会话预算失败释放、绑定变更、冻结只读历史与当前写句柄矩阵继续执行；新增完整调度日志证明 hello 只取一次快照，后续根访问每次调用真实 typed 准入且不走 generic call。


## 能力公告与管理状态

required Backend::capabilities() 不接收 Identity，保留原匿名例外，不伪造 anonymous/management 用户。Capabilities 私有字段区分初始未取得引擎配置的 profile 与完整引擎 profile；max_upload_bytes 始终是 u64，冷态为明确的 0，不再依靠字段缺失、Value.get 或字符串解析回退到 HTTP 上限。Management 缓存 typed 快照，先释放 cache 锁再捕获管理状态，保留原观测时序；恢复后离线仍保留已取得的能力描述，不把状态降级误作能力格式重置。

HTTP 在独立 management_slots 的阻塞任务中读取公告并计算 min(server,core)，最后才输出当前 wire；SMB hello 直接计算 min(core,SMB上限)。固定能力字段来自一个只读静态描述，响应借用它序列化；冷态精确保留 features:[]/max_upload_bytes:"0"，不偷渡完整 profile。Capabilities action 与最后的 SMB 内部 JSON field/decimal 辅助删除。

StorageState 移到无 core 依赖的声明并由 management 重导出，原枚举与 15 秒 stale 观测规则不变。management_status 与 retry_storage 返回 StorageStatus：EmbeddedReady 保留省略 recovering 的 wire，Managed 保留真实 recovering bool；retryable 只在出口按状态派生。HTTP storage/status/retry 和外层 management JSON 均在实际 HTTP 边界投影，Management Ready 分派直接比较 typed state。重试仍在受预算阻塞任务中复核当前账号再启动恢复，没有改变后台恢复持有者、撤权或持久化顺序。

回归覆盖完整固定能力字段、冷态、全部状态/可选字段、公开路由真实 panic、注销后的匿名能力例外、独立管理预算及取消后的 permit 持有；HTTP 实测管理未就绪与引擎直接托管两种 wire。Ready 已在下一组接续迁移；此处不宣称 setup_operation 或所有 FileManager action 已迁移。


## HTTP 永久操作查询

这里复用既有 Backend::operation(OperationId)，不重做已经完成的 SMB typed operation，也不把核心 OperationResult.result 或公开 kind 封闭化。HTTP ApiPath/原 query map 在入口接收；backend_slots 内一次解析 OperationId，保持非法ID的 404/OPERATION_NOT_FOUND/原source映射，再进入 typed 方法，最终只在 HTTP 出口调用原 operation_wire。Operation action 与重复引擎分派删除。

身份中间件仍先执行；同其他已迁移入口，格式解析在阻塞任务中的当前 Backend 准入前完成，排队期间撤权与非法ID同时出现时先给格式404，不读取任何收据或正文。有效ID仍严格执行 current admission→永久owner检查→记录授权→锁外health IO→回锁current/记录授权；不缓存旧健康、不裁剪收据、不跳过撤权或绑定。

核心 owner/未知/待决发现失败及 logout/password 回锁矩阵改为真实typed调用；需要检查wire的测试显式在测试客户端投影。原两项 degraded/permanent-loss与publication后损坏证据测试升级为真正HTTP请求，而非公开一个测试专用JSON后门；响应capture与固定publication事实断言保留。公开panic、预算取消、非法ID来源和现有HTTP严格操作ID矩阵继续覆盖真实路径。


## Readiness 匿名入口与固定成员观察

Backend::ready() 不接收身份；HTTP /readyz 与后台 observe_storage_once 直接消费 typed Readiness，不再创建 health/monitor 身份或借用 generic action 例外。Managed 成功只表达原 {ready:true}；Engine 成功携带 Mode、u64 sequence 与固定成员统计，HTTP 出口才输出字符串 sequence 和 metadata_redundancy。Ready action 删除后，AuthorizedBackend::call 和 EngineBackend::call 的每个剩余 action 都无条件走当前身份复核，匿名能力/就绪探针只存在于各自显式方法。

引擎保留 read-only拒绝→同锁捕获固定成员/配置/sequence与磁盘观察计划→解锁真实观察→全部配置元数据成员在线校验的顺序。返回值只是一次观察，不授予未来写入或缓存磁盘健康。后台仍持有同一 engine lease完成一次样本、更新时核对当前 backend Arc身份与 admitted状态、保留错误分类/observed_at/15秒过期逻辑；无成员多数表决或刷新间隔变更。

原1–4成员及掉盘拒绝测试改为真实typed调用，固定wire/匿名HTTP/真实panic/管理槽取消矩阵同步，另以全部 BackendAction::ALL 验证缺凭据时每个generic action在读字段前拒绝。management_slots仍独立于工作区backend_slots，取消请求不提前释放阻塞观察持有的许可。


## 保留工作区容量投影

contract::WorkspaceCapacity 本来就是只读容量响应契约而非持久化记录；现在 Backend::workspace_capacity 返回该投影，engine同锁复核当前身份后使用 current.principal 捕获预算，不经 Value encode。HTTP 只在真实响应边界序列化一次，保留原 management_slots、不接纳query身份选择、全局/当前主体字段、u64文本及 automatic_retirement=false；没有更改核心计数、near-limit规则、永久证据保留或启动配置。

WorkspaceCapacity action 删除，原 management_call JSON桥已无消费者并删除。授权装饰器、排队撤权、forged principal、真实panic、独立管理槽取消和原live-capacity/多session/logout/不可通过query隐藏全局预算测试均走真实typed方法；其它 FileManager/计划/通用读取仍按剩余库存继续，不据此宣称所有动作完成。


## 带逻辑路径的正文下载

HTTP 下载把成对 repository_id/path 查询一次解析为 DownloadContext（LookupPath 缓存查找键），与 version 组成 ReadContent；必需的 Backend::range_content 方法完整传递上下文，不允许默认实现静默丢弃它。FileManagerDownloadContext action 与 HTTP 内部 name 字段 JSON 提取删除。剩余生产 Backend::call 只用于 HTTP 通用分派及其授权/管理转发，不再有下载或 SMB 内部消费者。

引擎在同一锁内按当前身份、上下文仓库、版本存在、history 归属、当前路径文件身份顺序准入，然后捕获下载名与正文读取计划。上下文路径名直接使用当前条目，不再先构造默认名称再覆盖；无上下文时保留现有 file_locations、条目、file_names、creation_name 的合法回退规则。上下文缺失版本仍是 VERSION_NOT_FOUND，无上下文缺失版本仍是 NOT_FOUND；跨 history 或路径文件不匹配仍为 FILE_NOT_FOUND。路径静态格式在 HTTP read_slots 阻塞任务中、Backend 当前准入前解析：同时存在格式错误与缺失仓库/排队撤权时格式错误优先；中间件身份检查仍先执行。

上下文检查与正文计划从两次引擎准入合为一次，避免并发改名/选择在两次观察之间产生混合名称。计划建立后仍解锁执行原读取 I/O，保留已准入读取租约语义，不把下载上下文作为后续请求的授权缓存，也不增加发布式回锁阶段。read_slots 的租约、总超时、Range/HEAD、内容校验和正文句柄生命周期不变。回归覆盖成对/空/非法查询、规范化查找、历史正文与当前文件名、跨 history、GET/HEAD 范围响应、真实 panic 和排队撤权。


## 完整树快照的 HTTP 边界

HTTP Tree 用 ReadTree（branch、可选 commit）直接进入 tree_snapshot，返回既有四字段 owned TreeSnapshot；Tree action、引擎内 query Value 解析及 Value 输出删除。HTTP 保持 ApiQuery<BTreeMap> 的原解析政策、忽略其它查询参数、路径 branch 不被 query 覆盖，显式空 commit 不降级到当前 HEAD。当前身份、branch/history 与所选 commit 的归属检查继续由同一引擎锁内的 branch_snapshot 负责，PendingSources 和 list_tree_at 保持原合并与错误顺序；离锁响应投影不成为后续授权证明。HTTP backend_slots 从排队到阻塞结束持有，序列化在实际 HTTP 出口进行。

先前 frozen projection、授权差分、旧实现 pending oracle、SMB 测试客户端和排队撤权测试已迁到实际 typed 方法，不保留旧生产 JSON 后备路径；测试输入向量可在 test-only 客户端转换为 ReadTree，生产无需重建 JSON。没有改变 Entry 持久化/canonical 表示或历史选择规则；响应仍需完整树复制，本组不声称优化为分页或零拷贝。


## 仓库列表

Repositories action 已移除，repositories_snapshot 在当前身份准入的同一引擎锁内按原 repositories 映射顺序捕获 Vec<Repository>。HTTP 列表复用单仓库的穷尽 RepositoryResponse，锁外投影与序列化，保留包裹字段、字符串 generation、file_manager_root=false 省略规则和原 query 忽略政策；查询参数不能选择身份或过滤权威列表。现有管理容量/取消、真实 panic、撤权和发布准备期间读锁释放测试都改走实际 typed 方法。SMB 测试客户端直接读取 Repository 字段，不再 Value 取值/重解析。

这是输出规模 O(N) 的 owned snapshot，不声称无需复制或已增加分页；领域/磁盘格式、授权例外及排序政策不变。固定多仓库 HTTP 回归以创建接口的完整对象为参考，核对列表排序、全部字段、单对象一致性与未授权拒绝。


## 分支元数据与列表

Branch/Branches 两个 HTTP 读取动作已删除。branch_snapshot 在当前身份准入后按原 repo_for_branch 检查迁移状态/history/仓库；branches_snapshot 先按 repository 准入，再按原映射顺序捕获同 history 的全部分支（不新增逐分支过滤）。两者返回 BranchSnapshot，不从 branch_wire JSON 反解析。捕获函数只借用同一锁内 State，穷尽解构 Branch，合并 head pending 与覆盖优先的 branch pending；不修改 Branch 持久化模型。

显式 BranchResponse 在 HTTP 边界输出字符串 generation、可省略 file_id/file_node_id 和始终存在的合并 pending。文件分支独立捕获可见/源版本、存储 disposition、别名及 node_id，保留旧 LIVE（合法 disposition 缺失）、PENDING（继承）与已有 ContentState 的差异；显式空 source 不降级到 visible；alias_of 在文件分支无别名时是 null，在非文件分支省略。归档/迁移状态和别名决定 read_only，node_id 保留显式节点优先与可见版本回退。

内容归属字段由共享 ContentDispositionSnapshot/Response 生成，旧 content_wire 也仅在原 generic 输出点序列化该类型，不维护第二份继承/状态派生逻辑。显式空 source 的选择/过滤仅在 capture_content 中做一次，响应只消费已捕获选择。CreateBranch 和 FileManager 文件历史仍使用通用 JSON action，其原输出点复用同一 typed 捕获及显式 BranchResponse；这不是已迁移读方法的 JSON 后备路径，也不宣称这些写入口已完全类型化。独立冻结旧 wire oracle 覆盖普通/文件分支、pending覆盖、空/缺失版本、全部ContentState、状态/别名/显式node组合及 u64::MAX；HTTP核对历史fork、列表排序/字段、query不可覆盖路径与具体缺失错误。实际panic/取消准入/撤权矩阵随方法迁移。
