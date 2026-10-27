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

## Value 调用与最后动作退役

三类计划 prepare 和磁盘在线登记已接续迁移为具名 Backend 方法，最后四个动作不再
拥有生产调用方。`BackendAction`、`Backend::call`、HTTP 通用 call 与 handler 宏、
Engine JSON action dispatcher 及其测试转发／空调用记录均已删除，不保留空枚举。
源码集成与验证限制见[最后动作集成](../validation/final-typed-actions-integration.md)。

仍存在的 `setup_operation` 是独立管理初始化协议；`management_status`、Plan.input、
OperationResult.result 和持久化 Value 具有各自消费者，不能因 dispatcher 退役机械删除。
这不等于所有领域／持久化／wire 模型分离完成，也不代表全仓已经符合规范。

这次不改领域 `OperationResult.result`、Plan 输入、持久化编码、canonical 哈希、永久幂等收据或公开 `Commit.kind`。当前 `core.schema.json` 的 `$defs.Commit.properties.kind` 是开放 string，不可仅按内部已见字面量将它收窄为封闭枚举。后续 model/persist/wire 分离应独立建立固定向量和公开消费者证据。


## 分支写会话

HTTP `/api/branches/{branch}/write-sessions` 在 bounded strict JSON/path-body 绑定后，持有 backend_slots 的阻塞任务解析只含 branch_id 的 DTO；Backend::create_write_session 接受已解析分支文本并返回 WorkspaceWriteSession，不保留 WriteSession action 或内部 JSON 路径。当前身份复核、分支仓库可用性、工作区创建和 expires_at 捕获均在同一引擎锁内；没有调用 create_workspace 后再次读取配置的准入窗口。格式错误在当前 Backend 准入前拒绝，撤权及领域创建/预算错误顺序不变。

响应显式映射 write_session_id/workspace_id、scope 和字符串 u64；内部租期保持 u64，保留现有饱和到期时间规则，不返回任何本机路径。会话创建仍是非幂等工作区分配，不伪造永久操作收据；后续发布的永久幂等契约不变。磁盘路径写会话是不同入口，现已接续为 `create_disk_write_session`，仍不伪造永久收据。


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

Backend::ready() 不接收身份；HTTP /readyz 与后台 observe_storage_once 直接消费 typed Readiness，不再创建 health/monitor 身份或借用 generic action 例外。Managed 成功表达 `ready:true` 和当前 `history_index`；Engine 成功另携带 Mode、u64 sequence 与固定成员统计，HTTP 出口才输出字符串 sequence 和 metadata_redundancy。`history_index` 只有 `unbuilt`、`ready` 或带安全 code 的 `unavailable`，不输出内部 source。管理入口只读取同一 Engine 的类型化索引状态，不增加成员探测或文件 I/O、不另存一份健康真值；派生不可用不改变原可写 Ready 结论。Ready action 删除后，AuthorizedBackend::call 和 EngineBackend::call 的每个剩余 action 都无条件走当前身份复核，匿名能力/就绪探针只存在于各自显式方法。

引擎保留 read-only拒绝→同锁捕获固定成员/配置/sequence与磁盘观察计划→解锁真实观察→全部配置元数据成员在线校验的顺序。返回值只是一次观察，不授予未来写入或缓存磁盘健康。后台仍持有同一 engine lease完成一次样本、更新时核对当前 backend Arc身份与 admitted状态、保留错误分类/observed_at/15秒过期逻辑；无成员多数表决或刷新间隔变更。

原1–4成员及掉盘拒绝测试改为真实typed调用，固定wire/匿名HTTP/真实panic/管理槽取消矩阵同步，缺凭据矩阵现已接续为全部具名入口，继续验证字段／存储工作前的身份拒绝。management_slots仍独立于工作区backend_slots，取消请求不提前释放阻塞观察持有的许可。


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


## 路径解析结果

Resolve action 与内部 optional_identity Value 解析删除。HTTP 在原 bounded strict JSON（重复键拒绝）和路径/body branch绑定后，以明确边界构造器按 unknown字段→path非空/LookupPath→显式commit非空顺序生成 ResolveEntry；使用小型构造器而非通用derive是为保留具体消息与字段失败优先级。LookupPath只构造一次，核心直接使用缓存key，删除 validate_lookup_path 后再 path_key 的重复规范化。静态字段错误先于 Backend 当前准入/仓库存在性；中间件认证仍先执行，合法请求的当前身份→branch/history/repository→commit→entry错误顺序不变。

核心同锁返回 ResolvedEntry，不构造Value；Entry、原entry.version_id对应的Option<FileVersion>和可选文件内容归属均为owned响应快照。目录version为null且没有文件content扩展是合法缺失；不把继承可见版本偷换成原version对象，也不把缺失记录补成默认版本。HTTP ResolveResponse复用ContentDispositionResponse及穷尽VersionRecordResponse；单版本metadata仍在同一record之上额外输出content_state。版本字段和字符串整数只在wire映射，持久化/canonical不变，尚未迁移的通用接口继续列入库存。

真实HTTP覆盖当前/历史完整version投影、等价路径、缺失/非法身份、unknown/path/commit优先级和重复键；真实core目录解析验证Option缺失和具体entry错误；真实panic/取消槽占用/排队撤权装饰器走resolve_entry。没有加入泛化JSONfallback或把旧Action留作并行内部路径。


## 公开提交记录读取

这里迁移的是 GET /api/commits/{commit}，不是此前已经返回Arc的SMB snapshot_at。commit_snapshot 在同一当前身份准入锁内查找共享提交、复核history→repository归属，再返回Arc<Commit>；不存在的提交仍先返回具体commit not found，未额外要求分支存在或将历史提交限制为当前HEAD。Commit action删除，HTTP保留原query政策、backend_slots生命周期与panic隔离。

CommitResponse拥有Arc，Serialize时仅穷尽借用CommitRecord字段：entries/parent_commit_ids是slice，pending map与可选namespace_deletions借用，不clone整个Commit或条目。序列化在引擎返回并解锁后；已准入响应可在撤权后完成，但下一次commit_snapshot必须重新准入。公开core.schema的Commit.kind仍为开放string，响应保留任意kind、字符串sequence、provenance=null和namespace_deletions/pending的原省略规则；未改持久化或canonical编码。

回归同时核对真实state Arc/entries指针、DTO集合借用指针、序列化期间跨线程可取得实际引擎锁、撤权拒绝后续读取、u64::MAX与未知kind的固定字段/规范字节、完整HTTP当前/历史commit与独立Graph记录一致性。Graph已在下节接续迁移，不把本次Arc改造说成所有历史接口已去全量复制，也不改变History可见性过滤。


## 历史图快照

Graph action已删除。history_graph 先同锁复核当前身份与history→repository，再捕获该history的metadata及Vec<Arc<Commit>>；SharedRecords::shared_values借用按key排列的Arc，过滤后才clone选中handle，不再次get_shared查树、不clone Commit.entries。branch/link/boundary元数据仍是输出规模的owned复制；释放引擎锁后按原sequence稳定排序commits，再由HTTP显式HistoryGraphResponse输出。其它集合保持原map顺序，跨history仅保留原筛选条件，外部boundary标识只作为审计数据，不新增遍历或授权路径。

Graph的commits复用CommitResponse/CommitRecord；history/raw branch/merge link/boundary使用穷尽wire转换。Graph中的Branch是原始记录，不混入普通BranchResponse的materialized/read_only等派生字段；namespace/pending省略、字符串generation/sequence、可空标识与boundary_marks始终为数组的形状保持。Engine::graph仍供其它现存核心调用方使用，本组不宣称其所有调用方已去全量复制，History可见性过滤也没有改变。

SharedRecords回归覆盖过滤前后handle计数、零payload clone、NonClone类型、key顺序、COW隔离及owner释放；实际Graph回归比较旧Engine::graph完整wire/规范字节、selected commit/entries指针、排除history不增引用、后续发布/引擎释放后的冻结结果、锁外序列化与撤权首错。HTTP核对同history范围、sequence顺序、raw branch及公开commit字段一致性；panic和取消容量矩阵走真实history_graph方法。


## SMB share 配置边界

HTTP GET/PUT `/api/repositories/{repo}/smb-binding` 使用 required `smb_binding_status` / `update_smb_binding`，旧两 action 删除。GET同锁复核当前身份、repository和history，捕获四字段owned状态；wire中的history_generation明确取history.generation，不把auth::SmbBinding的repo.smb_binding_generation当作同一含义。PUT在严格JSON与path/body绑定后、持有backend_slots的阻塞任务中按unknown字段→enabled→generation→branch→operation顺序解析；非法非空operation保留INVALID_ID与来源，未供应字符串保留原具体错误。静态格式拒绝现在先于Backend repository准入（HTTP认证中间件仍在前）；不伪称跨层错误优先级完全未变。

引擎同一锁内复核当前身份并使用current.principal，保留repository存在性前置检查，然后直接调用原set_smb_binding；其永久收据摘要仍由原kind/principal/typed参数构成相同canonical body，expected_generation内部键不改成public expected_history_generation。同operation重放先于变化后的generation/workspace条件；不同输入冲突不越过永久收据，missing repository仍先于core收据。核心递增history和SMB binding两个计数且重放不重复递增；SMB hello与现有会话仍校验原binding token，不把status快照当授权。OperationResult仅在HTTP最终operation_wire编码，未加内部JSONfallback。

回归覆盖真实核心旧入口共享相同收据、相同/不同输入重放、陈旧generation、Open/Sealed workspace拒绝、缺失仓库首错、撤权、HTTP wire与静态错误来源；既有SMB rebind helper直接使用typed字段，panic/取消槽占用/排队撤权矩阵均走新方法。持久化、核心事务与共享生成计数实现未改。


## 有界事件页

GET `/api/events` 的after在HTTP边界解析一次为u64；缺失明确表示0，空串/非规范整数仍拒绝，未知query继续忽略。Backend::event_page是required typed方法，Events action删除。当前身份复核后同锁检查cursor不超过已发布sequence，再保留events_after二分定位与最多1000条限制，复制该页Event到请求独有Vec；next_cursor仍为最后返回事件sequence，没有事件时保持after，不能把没有事件的sequence间隙推进至全局published值。既有持久化Vec<Event>及排序/恢复契约不改，没有新增缓存或全历史复制。

返回解锁后显式穷尽EventResponse字段编码，OperationId保持原typed值，kind/object_id移动为wire字符串，u64仅在wire转十进制文本。合法cursor的InvalidCursor判断仍在当前身份准入后；静态非法整数现在在Backend前拒绝，但HTTP认证中间件仍优先。没有逐event新增过滤，保留单管理员原可见性，不混入Graph/History范围策略。

回归覆盖100k历史尾部二分与1000/500页、空页sequence间隙、u64::MAX、固定canonical字段字节、真实核心发布后冻结和锁外序列化/owner释放、原恢复顺序、HTTP未知query/坏cursor/匿名优先级，以及真实panic/取消容量/撤权排队矩阵。remaining动作清单继续保留尚未迁移项，不把有界页所有权当作整个历史存储优化。


## 当前拓扑与版本冻结放置

真实Web消费者只请求`/api/backup-topologies/default`，核心只有当前池的全局topology_generation与已验证config.disks，没有按ID查找多个拓扑的模型。旧动态路由忽略任意topology参数并返回default，不是已确认的别名契约。本组收敛为固定default路由；其它拓扑路径自然404，没有新增单值enum、字符串状态或兼容别名。三份活动接口文档同步，来源归档保持不可变。Backend::topology_snapshot不接受闲置ID参数；同锁当前身份准入后捕获generation及配置顺序的六字段slot元数据，穷尽解构DiskConfig时明确排除本机root和capacity_bytes。只有最终wire补充固定topology_id=default及十进制代际。

Backend::placement_snapshot仍按当前身份→version→history/repository→placement存在性顺序，返回owned Placement而不是锁内Value。HTTP显式穷尽映射version_id/primary_slot_id/target_copies/route_slot_ids/topology_generation/binding_generations，保留冻结路由顺序、十进制字符串map及原缺失错误；query不能覆盖路由version。两项旧action删除，锁外编码；不更改磁盘配置、核心拓扑发布、物理盘检查、历史版本放置或持久化格式。

回归涵盖旧完整wire/规范字节、MAX代际map、metadata_member=false/upstream=null保留、六字段白名单、本机路径不泄露、真实发布后快照/owner释放/锁外序列化、缺失与撤权首错。实际HTTP对default认证入口与其它拓扑名（包括匿名）404分开断言；panic/取消槽占用/排队撤权矩阵走required typed方法，不用默认unsupported冒充入口覆盖。保护策略及放置规则已在下节独立接续迁移，未将本组只读投影当作其完整迁移。


## 保护策略与一级放置规则

Policy/PlacementRule GET和PUT四个action已删除。HTTP持有backend_slots的阻塞任务按unknown字段→repository非空→其余DTO类型/规范generation→OperationId顺序解析一次；repository从object移出后不再由DTO重复验证，private fields仅在已检查字段集合后反序列化。公共DTO不携带principal；generation与OperationId进入Backend即为类型，未再传回Value。原Application两套仅用于此分派的Deserialize命令、二次repository检查和operation文本解析删除。

PUT在同锁current身份复核后检查repository一次，直接构造既有core ProtectionPolicyUpdate/PlacementRuleUpdate并返回OperationResult；鉴权使用current.principal。core公共Engine入口仍有真实直接消费者，原rule/policy ID及path_prefix检查保留，且不在HTTP重复检查：其先于永久receipt的错误顺序、同输入重放先于generation/作用域检查、canonical kind/principal/body、原scope/retained规则拒绝均未改变。静态DTO格式错误现在先于Backend repository/当前准入（HTTP认证中间件仍在前）；没有声称所有公共core字符串都已变成受约束类型。

GET仍按当前身份→指定policy/rule存在→所属repository存在顺序捕获owned记录，锁外显式穷尽wire；policy的scope_directory_id/scope_file_id与空backup_prefixes省略、inheritable和字符串generation原样保留，rule保留完整五字段。mutation结果只在HTTP最终operation_wire编码；core持久化结果与摘要格式不动。

原形状矩阵迁到真实HTTP parser，原core-replay测试改用typed方法并保留永久摘要、result、sequence、变更输入冲突、撤权/未准入身份与陈旧generation断言；原malformed/unknown请求的认证优先级另由真实HTTP覆盖，不把不能构造的坏DTO继续塞进内部接口。固定wire覆盖scope/保留字段及MAX，实际HTTP覆盖GET与操作result一致、重放、非法operation/整数、core路径错误先于同op冲突及更新后generation。panic/取消/排队撤权测试调用新required方法。公共core ID/path受约束类型改造仍是明确后续边界工作，不能凭本批删检查或声称全部parse已迁出core。


## 不复制条目的提交比较

POST `/api/compare` 只在HTTP解析unknown/left/right字段，保持unknown优先与left先于right的错误；严格JSON/重复键、认证中间件、backend_slots继续承担原边界责任。Backend::compare_commits同锁当前准入后依原顺序捕获left共享commit并复核history/repository，再捕获right并复核；缺失仍为comparison commit not found。随后释放引擎锁再运行纯比较，不再授权时查一次、core里重新查一次；不同history/repository的已授权提交仍可比较，没有暗加同history限制。

portable CommitComparison拥有两个Arc<Commit>及私有变更索引，不存Value或自引用，公开changes仅返回借用Entry的视图。索引完全由这些private不可变owner的entries枚举生成，读取直接依赖内部不变量；不为打印ID加无用校验。两个临时BTree索引借用cached lookup_key，保留左entries顺序的removed/modified后接右entries顺序的added；modified仍只比较version_id和display path，不擅自扩大到size/file_id/其它元数据，重复key的旧last-match行为也由差分测试锁定。没有Entry clone、额外历史缓存或生命周期trait；core公开Engine::compare_commits也返回同一typed结果，不保留JSON旁路。

HTTP ComparisonResponse按借用change视图流式SerializeSeq，显式三个wire变体及原before/after省略，不再分配Entry副本或中间Value。原Entry/model持久化与wire尚未全分离，仍沿用既有Entry序列化格式并以完整字段/规范字节回归，不借本批声称全模型分层完成。当前公开Commit.kind保持开放字符串。

回归覆盖旧比较算法差分矩阵、顺序/casefold/重复key、未参与modified的元数据、Entry指针与Arc持有数、COW与owner释放、空比较固定字节、实际engine原回归和HTTP当前/历史/跨repository/同提交/格式/重复键。实际backend测试在后续发布及撤权后仍序列化已准入快照并证明锁可取，下次调用重新拒绝；panic/取消/排队撤权矩阵走required typed方法。纯比较保持原O((L+R)log(L+R))索引成本，主要变化是CPU/编码移出引擎锁及移除payload复制，不宣称未经测量的吞吐倍数。


## 文件历史的类型化读取

HTTP `/api/files/{file}/history` 与实际 SMB 测试消费者直接调用 required `Backend::file_history`，History action 删除。路径绑定和 query 规则保持不变；同一引擎锁内复核当前身份、调用既有核心 file_history。核心的选择性 SQLite 索引、回退与 version_sequence 排序不变；不把索引 I/O 宣称为锁外操作。返回 owned FileVersion 列表后，HTTP 才用已有 VersionRecordResponse 显式输出原始版本记录，不额外注入 content_state。

原 is_ok 筛选把内部缺父记录默默隐藏；不能把它重写成具名 missing 分支后当业务语义。现直接信任 Engine 已准入版本，不再逐版本重查 history/repository。磁盘边界 journal::resolve_admitted_snapshot 经 RawState→AdmittedState，history manifest 验证 repository 双向归属，HistoryCoverage::finish 明确覆盖全部 versions；普通/降级恢复及 checkpoint 使用 verified_snapshot，同样不接纳孤儿。SQLite 历史索引仅选候选，实际记录仍取自当前 State。真实写入从现有 branch/repository 建版本；split 在同一候选里补齐新父链；package import 先验证闭包，生产没有删除 history/repository 后保留版本的路径。未发布的候选不会成为并发读取状态。

保留外部按 history 查找的两条 NotFound 和未知 file 的 file history not found，不添加 active/content-state/跨仓库筛选。原手造损坏 State 的服务器“不可见”测试已删除；真实磁盘 store/load/verified_snapshot 负例覆盖 MissingHistory、MissingRepository，并新增仅版本孤儿触发 versions coverage 拒绝的断言。合法状态回归覆盖真实核心查询与原 wire 规范字节一致、后续发布/撤权与已准入快照生命周期、锁外序列化、HTTP 路径不可被 query 覆盖、原始版本 wire 对齐，以及真实 typed 方法的授权、撤权、panic 和取消预算矩阵。领域/持久化 FileVersion 分离并未因这次 wire 投影而全面完成。


## 内部动作枚举的退役

原 BackendAction 不是 HTTP/RPC 字符串协议；随着最后四个实际消费者完成类型化，
枚举及其仅测试 ALL 清单、generic call 记录器均已退役。当前撤权与无凭据验证使用
具名 Request 的完整集合，不为历史测试保留不可调用分派路径。真实 RPC、准备任务
execute、持久化 Value 和独立初始化协议没有因此删除。


## 管理员磁盘状态与完整性检查

GET `/api/admin/disks` 和 `/api/admin/check` 的真实管理 Web 消费者改走 required `disk_status`/`integrity_check`，删除 Disks/Check action 以及不再有消费者的 list_handler 宏。HTTP 继续解析并忽略原 query，持有 backend_slots；同锁 current 准入之后，Disks 捕获原 DiskObservationPlan 再解锁采样，Check 保持原核心离线完整性检查的锁内 I/O 与逐盘问题聚合语义。不借本次类型迁移改变检查时序、后台缓存或磁盘完整性策略。

Backend 返回既有 Vec<DiskStatus>/CheckReport，HTTP 才做穷尽 wire 投影：磁盘十字段、nullable upstream、字符串 u64 bytes、原始 physical_device 不变；Check 的三个 usize 计数仍是 JSON number、issues 原顺序保留，不机械改成字符串。真实掉盘继续输出观察/检查报告而不是用新的默认状态掩盖错误。MonitorDisks 和 FileManagerDisks 具有不同采样、字段与资源策略，未被这组假称完成；私有监控采样已在下节独立接续，FileManagerDisks 也在后节复用同一已类型化采样方法。

测试覆盖原 wire 及固定规范字节、真实核心检查与掉盘报告、当前身份/撤权/排队准入、已准入 owned 响应的锁外序列化与 owner 释放、HTTP 查询及匿名拒绝、真实 panic 和取消预算矩阵。没有新增副本、fsync、权威状态或永久幂等格式。


## 私有监控采样、缓存与整数统计

真实路径 EngineBackend::observe_disks→management 的 15 秒新鲜度缓存→Telemetry::collect 直接传 Vec<DiskSample>，不再构造 Value、克隆 disks JSON 再 from_value。DiskSample 表达非权威物理观察，不能授权写入；InternalDiskObservation 私有能力与 AuthorizedBackend 默认拒绝仍保留，未伪造系统用户。原 MonitorDisks generic 没有生产调用方，已删除，原权限测试改验证真实公开 disk_status 不因私有采样而放行。management 单后台观察者、过期/失败清缓存、锁外 I/O 与 owner 释放不变。

采样内存/磁盘字节保持 Option<u64>，Statistics 直接做 u128 累计，不再 string→parse().ok 丢失坏值；每指标独立有效计数和多次聚合精度保留。cache 的 Sample 解码在真实磁盘边界用 optional_u64_string 严格接收当前 canonical decimal/null，随后沿原 valid_at/valid_for 验证统计范围，再进入缓存和平均；非法字符串不是“无传感器”，无旧格式迁移。均值转换依赖已准入样本与 sum≤u64::MAX×count 的既有统计不变量，不为每次读取重复校验。

既有 Snapshot/cache/HTTP 的字段和字符串 bytes 格式不变，Snapshot 尚承担共同投影，不宣称所有 model/persist/wire 已全面拆分。status 返回 Json<Snapshot>，history 返回具名 HistoryResponse，删除内部 to_value/序列化 expect；采样前后当前 session 检查、独立 2 槽、取消持有许可、31 日保留/文件/内存预算、后台生命周期与时钟策略不变。回归覆盖 exact wire、真实 HTTP 在线/掉盘/匿名/范围拒绝、物理 bytes 不冒充逻辑计数、混合 null 与两次聚合/restart、u128大数、损坏 bytes/统计在真实缓存恢复边界被拒绝，以及 capability/缓存owner/撤权/取消矩阵。


## 文件管理器磁盘投影复用真实准入

`/api/file-manager/disks` 的实际 Web 消费者位于 files/actions/browse.rs。该入口复用 required Backend::disk_status，不新增并行包装或重复引擎准入；仍在 backend_slots 内同锁当前身份→捕获 DiskObservationPlan→解锁物理采样，HTTP 最终穷尽映射原七字段，排除 disk_id/upstream/metadata_member/physical_device。

原 disk_access 只是同一 config.disks 的 slot→level==1 映射；观察计划捕获同一配置，filter_map 查询缺槽不是合法业务分支。映射、二次查表/静默过滤、FileManagerDisks action 已删除，writable 直接使用捕获的 level==1。它描述一级盘策略，不是假装当前 online；掉盘一级盘仍 writable=true、online=false，已有实际写入口的可用性/只读授权不变。配置顺序和全部槽保持，不创建 lazy 根仓库、不新增账户过滤，未知 query 沿原规则忽略。

回归验证两级/掉盘/最大bytes/七字段白名单，真实HTTP调用不创建根仓库、匿名拒绝和旧多槽场景；同一真实disk_status方法的公开panic和取消许可矩阵扩至文件管理路由。删除的是每次请求额外O(n log n)映射及查表，不改变核心采样I/O或声明未经测量的吞吐提升。


## 默认目录分支的轻量摘要

GET `/api/repositories/{repo}/default-branch` 与 SMB 目录选择回归的实际读取直接调用 required default_directory_branch，删除 DefaultBranch action。当前身份与 repository 一次同锁查找后捕获 owned 选择和目录摘要，HTTP 显式输出原五字段/每分支八字段，file_count 只在出口转十进制字符串；query 仍不能覆盖路径身份。

portable directory_branch_summaries 只对指定 repository/directory 的借用 DirectoryBranch 排序并穷尽投影小字段，不解析输入ID、不复制entries；顺序保持 sequence 逆序再ID、数量保持 file_branches.len（不混用另一路径按entries统计的定义）、path保持查询展示拼写。核心公开 Engine::directory_branches 也改为该typed结果，两个真实 package rename/restore 消费者使用字段；原scope解析拆成借用 DirectoryScopeView，只有实际需要 namespace payload 的原scope调用方才复制entries，元数据读不再为取directory ID clone整棵树。没有新增JSON旁路、索引或历史缓存。

未初始化选择的None由真实新仓库/HTTP回归证明合法，公开name=main保持；Some选择指向缺失branch不是业务缺失，manifest目录选择闭包已在磁盘准入保证，读取使用明确内部不变量而不再and_then→main掩盖。外部未知repo仍原NotFound，撤权仍先于资源查找。真实mutation后已准入摘要保持冻结、序列化可另线程取引擎锁，owner释放后仍可输出；纯排序/作用域/字段、固定wire、空/初始化HTTP、包重命名恢复、panic/取消/排队撤权矩阵覆盖实际typed路径。DirectoryBranchScopes的full/summary与有界选择在后节独立接续，其他目录写入仍待迁移，不借摘要迁移扩大其权限/筛选。


## 默认目录选择写入

Web files/actions/mutation.rs 的 PUT default-branch 以及真实 SMB 目录切换回归改为 required select_default_directory_branch。HTTP 绑定路径/严格JSON后，在持有 backend_slots 的阻塞任务一次解析 SelectDefaultDirectoryBranch；OperationId 保持类型并保留非法非空ID的 INVALID_ID/source。未知字段、空字段和路径身份冲突在 HTTP 拒绝，资源ID仍按原查找契约处理，不新增格式门槛。静态格式错误现在先于排队后Backend当前身份复核；HTTP中间件认证仍先于body，合法命令的撤权仍先于资源查找。

EngineBackend 同锁当前身份→既有repository准入→核心 select_default_directory_branch，直接返回 OperationResult，HTTP才 operation_wire；删除 UpdateDefaultBranch action/JSON arm。SMB回归直接生成typed OperationId，不用字符串绕行。repository缺失先于永久receipt，同operation不同branch摘要冲突先于动态scope检查；核心公开入口自己的repository/scope检查仍服务直接调用方，未把它们移动到receipt前。canonical directory.default.select 的kind/principal/body与永久幂等语义不变，无新增锁、I/O、发布阶段或格式迁移。

回归覆盖独立摘要计算、核心直接调用共享receipt、重放、冲突/作用域/未知repo/撤权次序、真实HTTP回复与路径绑定、SMB目录切换对已打开writer的fencing，以及实际方法的授权参数/排队撤权/panic/取消许可矩阵。


## 目录范围 GET 的完整记录与轻量选择

`/api/repositories/{repo}/directory-branches` 的 GET full/summary 是不同的真实响应：Web batch/actions.rs:257 读取12字段摘要，namespace HTTP回归读取完整entries/maps；SMB目录移动回归只读head_tree_id，现也直接消费typed Summary。HTTP将省略path、显式空path、具名LookupPath分别表达All/Root/Directory，summary只接收字符串true，未知query仍拒绝。静态query/path解析移到HTTP阻塞任务，不再穿Value到引擎；因此静态坏格式在Backend当前身份/资源准入前拒绝，中间件认证仍最先。POST bounded selection在后节独立接续，不混入GET投影。

required directory_scopes 在同锁current/repository准入后使用共享借用筛选器，原排序sequence逆序再ID、NFD/casefold路径查询、当前目录身份优先和当前目录不存在时按历史branch.path回退均保留。manifest验证已保证repository→main branch→head commit（manifest.rs主分支及live commit验证，加state_read完整coverage；真实磁盘state_read_tests已有MissingMain/MissingHead负例）；原链式Option不能把缺父链伪装成历史path回退，现明确内部不变量。合法的未找到当前目录Entry仍是Option业务缺失，外部未知repository仍NotFound；不新增每行父链验证。

State.directory_branches仍是TrackedMap而非SharedRecords：Full为实际锁外序列化捕获一次owned DirectoryBranch，file_count从该冻结记录在锁外计算，出口move各字段、entries/maps不再复制；Summary只复制展示字段，不复制entries或上下文map。两者file_count仍按entries中的File计数，不混用default-branch摘要的file_branches.len。模型/持久化的共享表示不在此批擅改；当前完整响应原本需要O(payload)的JSON树分配，现为typed所有权再直接编码，不宣称零复制或未经测量的吞吐提升。嵌套Entry/FileContext等仍沿既有公开字段编码，未宣称全模型已完成wire分离。

回归覆盖原full wire/可选map省略/12字段摘要、NFD/root/all过滤、移动目录身份回归、稳定排序、后续mutation不影响已捕获所有权、锁外序列化/owner释放、实际HTTP摘要与完整响应、未知query/非法路径/匿名拒绝，以及required方法的授权参数/撤权/panic/取消许可矩阵。没有新增缓存、状态索引、fsync或永久幂等格式。


## 有界目录选择事实

Web batch/actions.rs 的 POST directory-branches 现传入私有已验证 ReadDirectorySelection，删除 DirectoryBranchSelection action 和原 scope_query Value路径。HTTP严格JSON/path-body绑定并拒绝顶层/嵌套未知字段、缺失数组和错误类型；公开构造器一次接收边界字符串，保持 sources≤500、entries≤500、乘积≤10000、entry ID≤256、LookupPath/NFD及直接子项/去重规则。短路上界证明乘法不会溢出；sources保持输入顺序，entries保持原按ID/请求展示路径排序。scope的None只表达显式空path的根范围，缺path在wire拒绝。静态解析/预算现在在HTTP阻塞任务内、Backend当前身份/资源/动态source判断之前；中间件认证及backend_slots取消所有权不变，合法请求仍同锁current→repository→当前source immutable tree复核。

核心借用已缓存lookup key，不再重新validate/path_key或构造Value。SelectedEntryFact仅捕获请求的身份、实际展示path和原selected_path；单身份唯一匹配优先、同身份多个alias按exact key、合法缺失省略保持。Unique/Ambiguous表达算法真实所需状态，替代永不需要完整数量的饱和计数和Option组合；键/索引借用记录与请求，不复制Unicode键串。Summary复用既有12字段typed选择投影，最终HTTP加entries并编码一次，不把结果快照当作后续mutation授权/基线证明。

回归覆盖边界预算与静态错误、严格HTTP、NFD与10k alias事实、来源顺序/请求排序、当前tree冲突/未知repo/撤权次序、真实只读状态不变、捕获后修改分支不影响回复、锁外序列化与owner释放、公开panic/取消容量及真实排队撤权矩阵。永久幂等、磁盘格式和写入发布阶段未变。


## 目录分支重命名的已验证名称

Web files/actions/mutation.rs 的 PATCH directory-branches/{id} 绑定repository/branch路径后，HTTP解析 RenameDirectoryBranch 并调用 required rename_directory_branch，删除同名action/JSON arm。portable BranchName私有保存通过原128 UTF-8字节/PortableName/NFC政策的展示文本及固定Unicode碰撞key，不承担serde/磁盘编码。核心rename直接接受&BranchName，不再次校验或在每个候选中重复fold同一新名称；原公开validate_branch_name仍由manifest及其他真实原始边界消费，委托同一contract校验规则，不为保留API制造未使用wrapper。该规则自身仍可能为保留名stem分配，不宣称校验零分配；公开模型名字String与持久化格式未在此批全面重构。

HTTP保留精确静态优先级：缺失/空name字段先拒绝；非空name先读OperationId、再验证name政策，所以bad operation赢过非法名称，已存在operation遇非法名称仍先报告NAME*/METADATA_TOO_LONG。错误保留contract source及原public code/400，不被invalid_source降成INVALID_REQUEST。静态边界失败先于排队后的Backend current/repository复核；合法命令仍同锁current→repository→原mutate/receipt→动态branch/碰撞。canonical directory.branch.rename的kind/principal/body仍绑定原NFC展示字符串，不使用折叠key替代摘要输入，永久receipt/replay/冲突次序不变。

已同candidate解析branch后没有移除操作，后续get_mut必存在，改明确内部不变量而不伪造StateInvalid外部错误。回归覆盖名称边界/固定Unicode key、UTF-8上限、NFD/保留名、HTTP错误优先级/双路径绑定、独立canonical摘要/核心直接replay、同名fold碰撞、缺repo/branch与撤权、完整公开panic/取消/排队矩阵。没有更改Commit.kind开放string、发布阶段、授权范围或增加旧格式迁移。


## 显式归档与原始操作收据

文档声明的 POST branches/{branch}/archive 与创建后归档/重放回归改为 required archive_branch，删除Archive action以及Application中仅被此入口消费的String operation/Deserialize薄转发。HTTP在bounded strict JSON/path-body绑定后一次解析ArchiveBranch(OperationId)，字段非空→未知字段→operation格式的原静态次序保留；静态格式错误现先于排队后current/资源检查，中间件认证仍先于body，backend_slots仍由阻塞任务持有。

原该路由输出raw OperationResult，而非operation_wire观察包。显式ArchiveResponse穷尽移动原11字段，保留kind、sequence十进制字符串、nullable commit/history/branch、current_health枚举（不把RECOVERING变UNKNOWN或加checked_at）、result载荷，不错误切换为operation_kind/publication_sequence格式。内部没有encode→Value路径，也没有为归档引入时钟观察；公共Commit.kind与OperationResult.kind仍开放String。

同锁current→repo_for_branch（含migrated拒绝）→原core archive保持资源准入先于永久receipt，同operation变更既有branch先报摘要冲突，未知branch仍先NotFound；主分支保护、选中文件分支/工作区拒绝与generation溢出检查不变。回归覆盖固定canonical raw字节/最大sequence/null/Recovering、独立原请求digest与核心直接重放、只增一次generation、实际HTTP活动工作区拒绝/归档/replay、未知branch和匿名拒绝、真实panic/取消/撤权矩阵；generic授权probe移到仍存在MergeLink且ALL遍历保留。没有持久化迁移或发布阶段改变。


## 完整分支创建的类型化输入与永久响应

POST repositories/{repository}/branches 绑定路径身份后，在 bounded strict JSON 与 backend_slots 阻塞任务中一次解析 CreateBranch：repository/source/name/operation 非空字段次序、未知字段、OperationId、BranchName。BranchName 沿原 128 UTF-8 字节/NFC/PortableName 规则并缓存碰撞 key；非法 operation 仍先于名称政策失败。静态解析现在早于 Backend 排队后的 current/repository/source 动态检查；认证中间件仍先于 body。required create_branch 不再经 CreateBranch action、Deserialize 或 encode→Value；Application 保留实际 repository/source-history 归属判断，但接收 typed 命令且不重复解析 operation。

同锁 current→repository→source 属于 repository→核心 mutation planner/永久 receipt 的顺序保持；未知或跨仓库 source 仍 HistoryNodeNotFound，先于已存在 operation 的摘要冲突。核心公开 create_branch 接受 &BranchName，所有真实直接调用方已迁移；canonical branch.create 的 source_commit_id/name 仍使用原展示字符串，碰撞循环不再为每个现有分支重复 fold 同一新名称。旧记录名称仍是 String，按各自已有边界准入，并非全模型名称重构。

首次创建从同一未发布候选 next 捕获已构造的 Branch，原 persist 成功后才返回，不为取得返回值复制并反序列化 created_branch JSON；原 change 闭包、摘要、receipt 内容与发布顺序保持。既有 operation 仍只从不可变永久 receipt 的 created_branch 解码，不读取后来修改的当前分支代替重放，缺失或损坏仍保留 StateInvalid 与具体来源。OperationResult.result 持久化契约未改，本批不宣称消除全部动态 receipt 载荷。捕获函数支持 owned 创建记录 move 字段/map，借用 GET 仍仅 clone 一次所需记录；同锁捕获原 head 的 pending facts，HTTP 复用 BranchResponse 在锁外编码。归档、后续写入、共享绑定变化、重新打开后的响应保持原创建 state/generation/head；没有新增缓存、锁、发布阶段、fsync 或格式迁移。

回归覆盖静态错误优先级、独立 canonical digest、核心直接重放、折叠名称冲突、跨仓库/缺 source 在 receipt 前拒绝、归档后冻结响应、核心重启重放、真实 HTTP wire/path 绑定/匿名拒绝，以及实际 required 方法的授权、排队撤权、panic 和取消许可矩阵。剩余其他创建和文件管理动作继续列于动作库存，不以通用 Backend 方法包装旧 JSON 冒充完成。


## 仓库创建与动态固定路由

POST repositories 使用 required create_repository 和已有 RepositoryResponse，删除 CreateRepository action/通用 JSON arm。HTTP保留未知字段→target_copies 的 u64/u32 表示→name/primary/operation 非空→OperationId→名称政策的次序；strict JSON/bounded body、认证中间件和 backend_slots 任务所有权保留。target_copies=0 不在表示边界偷偷变为新的错误，而仍在原核心 route 报 PlacementInvalid；动态 slot/level/copies 的 route 校验仍先于永久 receipt，不能用已解析命令作为未来路由有效证明。

RepositoryName 私有保存 PortableName 的原始 NFC 展示名与固定 Unicode key，保留原240 UTF-8字节/UTF16限制，不套用 BranchName 的128字节上限。核心 create_repository 与私有 create_repository_kind 接收此类型，不再次validate_name或对每个候选重算同一新名称key。首次 disk-root创建在原名称验证位置解析新生成的完整标签；已存在root的返回路径、actor检查、slot查找与永久operation优先级不变。真实直接调用方（核心/服务/救援测试、浏览器fixture、perf工具）同步改为明确构造类型，没有增加裸字符串后备API。

当前身份同锁复核后调用原核心：合法动态route→mutate/receipt→名称碰撞/创建。canonical repository.create 的name、primary_slot_id展示输入、数值target_copies与disk-root私有file_manager_root标志不变；响应移交创建receipt内的原Repository，不读取后来的SMB绑定代替重放。核心 OperationResult.result 的 created_repository 解码仍是现有持久化契约，本批不重构其格式或宣称所有内部Value消失。

回归覆盖200/240字节名称、NFC/保留名与来源、copies表示拒绝/zero策略、独立digest、same-operation动态route错误优先于冲突、名称fold碰撞、核心直接replay、绑定变化/重启后原创建wire，以及实际HTTP授权/撤权/panic/取消容量。CSRF单元fixture改用合法创建请求和typed Repository返回，不再以空对象+fake accepted响应制造公开路由已调用backend的假证明。没有新增发布阶段、同步预算、真实SDK/浏览器运行或持久化迁移。


## 纯合并连线登记与拆分后的原始重放

Web admin/request.rs 的 Kind::Link 和 admin/response.rs 是 merge-links 真实消费者；此入口仅记录关系，不承担文件结果采用或目录分支合并。HTTP绑定history路径、bounded strict JSON后按原history/source branch/source commit/target branch/target commit/operation非空字段次序解析，再拒绝未知字段并生成OperationId，调用 required create_merge_link。删除MergeLink action/Deserialize/内部JSON编码；静态格式错误前移到Backend current之前，认证中间件与阻塞任务持有backend_slots不变。

EngineBackend同锁current→Application的history/repository准入→原split重放资格→核心add_merge_link。现有receipt的kind/history/actor全匹配才允许跳过已迁移端点的当前history检查，核心仍重新匹配永久请求摘要；它不是跨锁缓存，也不是旧身份永久授权。原owner/history在split后可重放；不同digest仍冲突，wrong history拒绝，新operation对迁移端点仍TreeMoved。history不写入原core canonical请求body，仍只绑定四个endpoint字段与kind/principal，不能在迁移中悄悄重建不同摘要。

返回原MergeLink记录，复用Graph已有MergeLinkRecord的八字段穷尽move投影；仅为新的真实HTTP消费者扩到pub(crate)，不维护第二份字段表。核心仍按已匹配receipt重建原端点/author/history；永久记录中merge_link_id的既有动态载荷不在此批伪装成已经完成持久化类型分离。没有内容I/O、时钟观察、额外锁或发布阶段。

原Application split/reopen回归已提升为真实required Backend入口，保留原冲突/错history/TreeMoved与全state不变断言；新增独立digest、同op缺端点先冲突/新op缺端点404、缺history、撤权、真实HTTP与Graph wire一致性/replay/路径绑定、完整授权/panic/取消/排队撤权矩阵。通用授权probe只改为仍存在Plan的测试期Debug记录，ALL仍遍历所有剩余动作，不恢复生产动作字符串协议。

### 目录范围初始化：解析路径与快照源复用

POST directory-scopes 的 HTTP 边界把空 path 转为根范围 None，非空 path 按原创建拼写规则解析 PortablePath 后无重复验证地转为 LookupPath，operation_id 解析为 OperationId。HTTP 仍拒绝 NFD 创建拼写、未知字段、非字符串与 path/body 身份冲突；核心公开初始化接口只接受已经验证的 LookupPath，因此直接库调用仍可引用规范等价的既有目录。原始请求展示拼写仍进入永久摘要，不把等价拼写错误地合并成同一幂等请求。

required Backend::initialize_directory_scope 取代 JSON action；AuthorizedBackend、ManagedBackend、EngineBackend 保留当前身份与仓库的动态准入，核心事务内再做当前目录存在性与选择检查。返回 OperationResult，HTTP 才应用原 operation_wire，包括即时 current_health 时间，不改成归档接口的原始收据格式。撤权、取消后的槽位归属与 panic 边界测试走真实新方法。

初始化现在只取得借用 scope view；已有选择不再为了取得目录 ID 复制 entries。首次初始化捕获一次选中子树和当前 Commit 的共享句柄，保留同一快照的规范展示路径，避免再次路径验证、再次规范化和整份 Commit 深复制。其他实际需要 entries 的 scope 调用方仍得到自有条目。没有新缓存、后台任务、锁、持久化阶段或格式迁移；首次子树筛选仍为 O(当前 namespace 条目数)，不声称全链路增量化。

### 文件管理器目录分支创建与选择

磁盘路径的 directory-branches / directory-select 使用同一真实命令 MutateDiskDirectory 与封闭 Create / Select 动作，移除两个 BackendAction 的 JSON 分派。bounded HTTP 解析 slot/path/name/operation：省略 path 或空串都保持原磁盘根语义，显式非字符串仍拒绝；创建拼写 PortablePath 转为 LookupPath，BranchName 缓存名称 key，OperationId 按该路由原 INVALID_REQUEST 映射保留来源。原名称失败先于 operation 格式失败的顺序不变，未知字段仍拒绝。静态格式解析现位于 HTTP 阻塞任务内、先于排队后的 current 再验证；中间件认证仍先于 body，合法命令仍先验证当前身份再接触资源。

EngineBackend 在同锁 current 身份后进入真实目录命令。虚拟仓库目录需显式范围、备份盘只读、合并命名空间歧义与惰性根准入保持；只有 Create 且根路径才允许建磁盘根，已用 operation 和 main 基线名称碰撞在建根前失败。共享磁盘解析器返回输入路径的借用后缀，不先分配裸 String。命名仓库去掉一个已验证组件得到 LookupPath 后缀，直接复用 display/key；磁盘根路径直接移动原解析结果，没有内部重新 parse 或 re-hash。

核心 create_directory_branches / select_directory_branches 接收 LookupPath 和 BranchName，只重查当前资源/选择/配额/正文可读性等动态条件。创建时仅需要 history ID 而不复制整个 Repository；已有目录选择和按名称选择不为取得目录 ID 复制 entries，首次物化复用已捕获共享 Commit。新名称 key 在碰撞/查找循环外已缓存，现有名称记录仍按当前存储模型计算 key。原 directory.branches.create/select 的 kind/principal/repository/path/name 展示字符串摘要、永久 receipt、scope 生成、同步顺序、操作健康观察包与文件正文检查不变；不承诺持久化全增量或吞吐提升。

无效裸 JSON 的原惰性根回归迁到真实 parser 和 HTTP 路由，逐次比较数据盘字节及仓库列表；合法内核/SMB夹具调用 typed backend，授权、两种动作的排队撤权、取消容量与 panic 边界均有对应调用。既有目录选择预算、部分选择、冲突、损坏正文和重启回归保留，不使用只改测试期望来放行不完整选择。

### 文件分支公开名称修改

PATCH file-manager/files/{file}/branches/{branch} 在 HTTP 边界绑定路径身份、拒绝未知字段并解析 RenameFileBranch；BranchName 保存原展示名称与碰撞 key，OperationId 只解析一次。空 name 仍先拒绝，非空 name 的 operation 格式失败仍先于名称政策错误；保留该路由 operation 的 INVALID_REQUEST/source 与名称的具体错误码。移除 FileManagerRenameBranch action，所有真实 Backend 实现提供 required rename_file_branch，不再在核心分发 JSON。

合法命令在同一 Engine 锁内按 current 身份、file_repo 当前资源准入、原 mutate/永久 receipt 的顺序执行。静态格式失败已移到 HTTP 阻塞任务内，先于排队后的当前身份和资源检查；中间件认证仍先于 body。核心只接收 BranchName，不重复验证或每个候选重复折叠新名称；既有存储名称仍按原模型计算 key。当前分支属于文件、未迁移、活动且非合并别名以及同文件名称碰撞仍为动态检查。canonical file.branch.rename 的展示字符串摘要、no_change、原历史树标识与 operation_wire 健康观察包不变。

不新增锁、索引、缓存、fsync 或格式迁移；碰撞查找仍扫描分支，元数据修改不创建内容节点。HTTP 回归覆盖独立摘要、同 ID 冲突、重启后重放旧 receipt 不覆盖新名称、格式拒绝不改变文件历史、128 字节边界与原内容/目录节点；动态撤权、授权参数透传、取消后槽位归属和 panic 隔离走新方法。直接核心的非法字符串用例迁到真实 HTTP parser/路由，核心合法夹具先构造约束类型。

### 版本来源与显式节点来源的文件分支创建

POST file-manager/files/{file}/branches 解析 CreateFileBranch：已绑定的 file ID、必填 source_version_id、可选但显式提供时必须非空字符串的 source_node_id、BranchName 和 OperationId。两个真实来源调用分别保留 file.branch.create 摘要中的 source_node_id 省略/存在形状，不变成统一 null；核心只接收已验证名称，pending冻结来源、未物化/未选中的结果、source node归属、main/casefold碰撞和当前file/repository准入不变。静态字段验证在bounded HTTP任务，合法命令仍同锁current→file_repo→mutate/receipt。新请求与原receipt不合并，operation_wire保持原健康观察包。

共享核心创建实现也由目录首次实际保存的惰性fork调用。该内部路径从已准入的目录分支名称生成UTF-8前缀和十进制后缀，不重新parse内部生成名称。实际借用 FileBranchCreation 带file/version/display/key，两个外部入口复用BranchName缓存key，内部候选每次只计算一次key并供碰撞及创建使用，不增加持久格式。

内部生成条件：原目录分支名在创建/改名的BranchName边界以及history manifest读取时受同一名称政策约束（含NFC与128 UTF-8字节）。零后缀保留完整原名；非零u64十进制后缀最多21 ASCII字节，保留至少107字节预算的字符边界前缀，因此非空且总长不超过128。前缀不引入新字符，连字符/数字合法，末尾数字消除截断处空格/点的尾部问题；原首字符仍不为空格。若前缀保留首个点，保留原合法设备名stem；否则尾部连字符数字使整体不等于设备保留名。NFC字符串的码点前缀保持NFC，ASCII连字符阻断跨边界组合。main属于业务保留名，由候选循环和创建碰撞检查排除，不由名称字符规则宣称排除。独立生成测试覆盖128字节、中文截断、重音字符、内嵌空格/点、COM10、main和u64最大后缀；这不是把任意裸结构视为外部输入证明。

### 文件、版本与节点备注

三个真实备注目标共用 Note 的128 UTF-8字节/无control规则：空串是明确清除，空格和NFD文本仍是合法原样文本，不应用BranchName的NFC或非空要求。HTTP file/version metadata使用原POST，node使用原PATCH；缺失note、null和非字符串一律拒绝，不能默认空串。读取note字符串后先解析operation，再验证内容政策，保留原静态错误顺序和来源。解析直接消费外部String，核心只接收&Note，不重复validate；持久化读取仍使用原validate_note。

UpdateNote的真实三变体NoteTarget排除file/version/node可选字段的无效组合，不合并三个map或operation kind。EngineBackend同锁current后，file和node走file_repo，version先查当前version并从记录取得file ID、再file_repo。file.metadata摘要仍含version_id:null，version.metadata仍绑定file/version，file.path.metadata仍绑定file/node；空串分别移除各自map的key。节点备注仍支持路径节点和metadata节点。OperationResult在HTTP应用原operation_wire；精确旧清除receipt的重放不得清除后来的新备注。

只有三个核心测试使用的旧set_file_path_note转发入口已迁到set_file_node_note并删除；Rust client同名方法是实际HTTP客户端接口，保持不变。没有新增persist格式、锁、同步阶段、通用DTO框架或缓存。HTTP回归逐目标验证缺失/null/格式/path身份失败保持历史不变、清除后重启三个map确实无key、旧清除receipt重放不删除later备注及撤权拒绝重放。全量验证结果独立记录，不把类型迁移视作全仓达标。

### 带位置和无位置的文件分支选择

SelectFileBranch真实命令用Option<FileSelectionLocation>区分无位置选择和明确repository/path；HTTP必须同时提供两个字段，缺一、null、空字段拒绝，文件路径按原PortablePath创建拼写规则保持NFC/格式错误码。先读取branch/operation再做路径内容政策解析，原非空非法operation优先于非法path。已绑定file ID不重复校验。合法命令同锁current→file_repo；有位置时保持仓库身份比较，再调用对应核心方法，不在两个入口之间JSON自调用。

核心select_file_branch_at接收&PortablePath，复用key查找当前file/path；publish_selected_file_at的另一个真实workspace调用方原本就持有PortablePath，因而共享helper直接接收该类型。选择key来自显式path或当前Entry缓存key，不再复制selected_path后重算key；file_locations中的持久display引用仍在需要匹配时计算一次key。保留原缺省位置优先级与实际错误，不新增缓存或索引。

两个file.branch.select摘要仍分别为file/branch与repository/path/file/branch，缺省位置不编码成null。永久receipt先于当前path、活动branch与正文使用检查，HTTP current身份和file/repository准入仍先于receipt；改名、merge、重启后原请求仍重放，但新请求仍检查当前规则。非pending分支继续valid_replica，pending分支继续冻结来源例外，不因为本次typing增加正文读取；原目录/上下文发布、世代、预算与同步顺序不变。

新增真实pending回归先移除旧source的全部实际副本，分别执行带位置和无位置选择，再重启重放，证明冻结pending来源未被强行要求新副本；既有非pending不可读正文测试同时验证两入口失败且State不变。HTTP覆盖成对字段、格式/身份失败不改历史、独立两种摘要及形状冲突、merge/rename/reopen后的重放和新请求只读拒绝；授权、两个位置形状的排队撤权、取消容量与panic隔离走新方法。没有改变正文丢失后的读取失败事实，也不宣称pending引用本身提供可读正文。
