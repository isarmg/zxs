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

## 剩余 Value 调用清单

此迁移没有覆盖所有应用接口。[BackendAction](../../src/server/src/backend_action.rs) 仍有 68 个变体；它是准确动作库存，下面按实际消费者列出后续范围，不表示每个动作存在同样的往返成本：

- **下一组：工作区 seal 与发布入口。** `SealWorkspace` 仍通过 HTTP JSON action 返回 seal 数据；`CommitWorkspace` action 与已有 typed commit_workspace 的实际消费者和永久幂等错误顺序需要继续核对。上传 publication 的结果、write-session 与 namespace 仍有 Value，不能据当前查询迁移宣称全部工作区接口已经分层。
- **SMB 绑定／只读冻结版本。** transport/session 的 `Repository`、transport 的 `Capabilities`、只读句柄的 `Version` 仍经 Value；命名空间修改 `Namespace` 仍使用 JSON 请求。不是本次 typed 当前查询的后备路径。
- **HTTP 通用读取。** repositories/repository/branches/branch/tree/resolve/history/commit/version/graph/events/compare、目录分支及选择、拓扑／放置／策略／磁盘／健康查询，仍由 HTTP 通用分派调用 `Backend::call`。
- **HTTP 写入与计划。** 仓库／分支／目录分支创建和选择、SMB binding 配置、namespace、workspace/write-session/upload、policy/placement，以及 split/purge/historical-save/plan 的请求与结果仍需逐组迁移。已有 operation/commit_workspace typed 消费者保持不变，不能据此推定剩余 HTTP action 路径已经移除。
- **文件管理与管理控制面。** `FileManager*` action、setup_operation、management_status 等仍有独立 Value 协议；需要核对每个 HTTP 消费者，不能以新增一个包裹枚举代替拆除旧 JSON 路径。

这次不改领域 `OperationResult.result`、Plan 输入、持久化编码、canonical 哈希、永久幂等收据或公开 `Commit.kind`。当前 `core.schema.json` 的 `$defs.Commit.properties.kind` 是开放 string，不可仅按内部已见字面量将它收窄为封闭枚举。后续 model/persist/wire 分离应独立建立固定向量和公开消费者证据。
