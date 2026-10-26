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

## 剩余 Value 调用清单

此迁移没有覆盖所有应用接口。[BackendAction](../../src/server/src/backend_action.rs) 仍有 74 个变体；它是准确动作库存，下面按实际消费者列出后续范围，不表示每个动作存在同样的往返成本：

- **下一组：SMB 工作区生命周期及写选择。** `Session::workspace` 的 `CreateWorkspace` 仍为 Workspace → Value → Workspace；handles/publication/namespace 仍调用 `AbortWorkspace`、`UploadAuthorize`。应把创建、写选择准入和销毁串联迁移，保留工作区所有者、期限、基线、撤权和未决操作语义。
- **SMB 绑定／只读冻结版本。** transport/session 的 `Repository`、transport 的 `Capabilities`、只读句柄的 `Version` 仍经 Value；命名空间修改 `Namespace` 仍使用 JSON 请求。不是本次 typed 当前查询的后备路径。
- **HTTP 通用读取。** repositories/repository/branches/branch/tree/resolve/history/commit/version/graph/events/compare、目录分支及选择、拓扑／放置／策略／磁盘／健康查询，仍由 HTTP 通用分派调用 `Backend::call`。
- **HTTP 写入与计划。** 仓库／分支／目录分支创建和选择、SMB binding 配置、namespace、workspace/write-session/upload、policy/placement，以及 split/purge/historical-save/plan 的请求与结果仍需逐组迁移。已有 operation/commit_workspace typed 消费者保持不变，不能据此推定剩余 HTTP action 路径已经移除。
- **文件管理与管理控制面。** `FileManager*` action、setup_operation、management_status 等仍有独立 Value 协议；需要核对每个 HTTP 消费者，不能以新增一个包裹枚举代替拆除旧 JSON 路径。

这次不改领域 `OperationResult.result`、Plan 输入、持久化编码、canonical 哈希、永久幂等收据或公开 `Commit.kind`。当前 `core.schema.json` 的 `$defs.Commit.properties.kind` 是开放 string，不可仅按内部已见字面量将它收窄为封闭枚举。后续 model/persist/wire 分离应独立建立固定向量和公开消费者证据。
