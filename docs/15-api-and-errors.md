# 15 API、操作结果与错误契约

[文档目录](README.md) · [上一章：身份、权限与运行隔离](14-security-and-permissions.md) · [下一章：Web 与内部管理命令行界面](16-client-interfaces.md)

<a id="s15"></a>

业务接口、幂等、分类型结果、错误及能力协商。

## 本章目录

- [15.1 设计原则](#s15-01)
- [15.2 拟定接口](#s15-02)
- [15.3 提交请求示例](#s15-03)
- [15.4 错误码基线](#s15-04)
- [15.5 能力声明与准入](#s15-05)
- [15.6 不同操作的结果类型](#s15-06)
- [15.7 请求摘要、幂等与结果保留](#s15-07)
- [15.8 上传代际与续传](#s15-08)
- [15.9 分页、事件和校验清单](#s15-09)
- [15.10 传输状态与契约交付](#s15-10)

<a id="s15-01"></a>

## 15.1 设计原则

Web 和保留的受控 HTTP 客户端使用统一 HTTPS 业务契约；SMB 基础操作经 Samba 及服务端桥接映射到同一核心规则。Web 管理功能调用业务 API，日常文件内容读写交给独立 SMB；Web 不再提供上传、下载和预览，两条用户入口不串接。本机 zxs 不再提供远程客户端或普通文件传输命令。内容流、元数据和管理操作可以使用不同路由或连接，但不能各自实现一套提交或业务约束。

项目中的绝对时刻统一以 UTC 为准，不按服务器、浏览器语言或用户所在地换算。业务文件历史节点与操作记录的展示时间由服务端统一输出 `YYYY-MM-DDTHH:mm:ssZ`；缺失的操作时间为 `null`。Web 原样显示 UTC 字符串，不保留数字秒与日期字符串的双格式展示分支。内部持久化 Unix 秒、会话到期值及单调时钟的超时计算保持各自明确的单位，不为展示改写原始时间事实。监测与健康检查同样显示 UTC，可保留其已有采样精度。

所有存储变更请求绑定主体、目标作用域和 operation_id。内容保存携带分支、文件版本及相应前置条件；配置变更携带对应代际。旧 `MergeLink` 只固定所选节点，不要求当前 HEAD 或文件内容；文件分支合并 `FileBranchMerge` 则固定每个参与head、结果和归属，并验证所选正文。业务文件名称通过结构化字段传输，内部对象路径不对外开放。

具名记录使用 JSON 对象，不接受按字段位置排列的数组；此规则同样适用于登录、改密和嵌套批量请求中的记录。数组字段仍可承载合法记录列表，不把列表改成对象。结构拒绝不绕过认证或 Cookie CSRF 校验，也不产生业务发布。

<a id="s15-02"></a>

## 15.2 文件管理接口与高级接口基线

路径解析 `Resolve`、目录列表、目录分支作用域查询及文件历史/下载上下文的路径参数按已有名称引用校验，允许规范等价输入，不改写已存储名称。工作区命名空间操作和核心直接命名空间操作的改名/删除源同样使用该规则；创建目录及改名目标仍严格要求 NFC。HTTP 写会话、上传和批量变更的现有严格路径准入不因此放宽。两类校验共用原始输入的字符、保留名、结构和长度限制，仅已有名称引用免除 NFC 输入要求，折叠与持久化键规则不变。

文件列表分页使用 `path/query/limit/cursor`；`limit` 默认 200、范围 1–500。`query` 是名称的 Unicode 小写子串，先在完整直接子项集合过滤，再按目录优先、小写名称、规范化名称、原名称、显示路径、身份确定排序。响应 `total_entries` 是匹配直接子项十进制字符串，`directory_file_count` 仍为未过滤后代文件数。`next_cursor/previous_cursor` 为空表示相应方向结束；`snapshot` 绑定业务代数和磁盘配置。改变盘位、目录、搜索或页大小须丢弃游标；业务状态变化返回 `409 LISTING_CHANGED`，不得静默拼接不同快照。认证发布不改变业务代数，每页仍重新校验当前管理员。游标不是授权凭据，不保持服务端大缓存。

主盘列表读取捕获同一业务代数与命名空间，锁外扫描直接子项，完成前重验当前身份。并发写入后已捕获响应仍可返回，其后续旧游标按上述规则拒绝。列表 HTTP 与图、活动、整树读取共享扫描并发槽与既有工作期限；取消信号在主盘每页、每项及元数据页读取中检查。完整响应编码上限采用现有 `MAX_RESPONSE_BYTES` = 8 MiB，超过时返回 `RESOURCE_LIMIT_EXCEEDED`，不截断条目或目录分支摘要。备份目录的全局冲突扫描仍持锁，取消在其前后检查。

成对提供 `selected_path/selected_entry_id` 时，`selected_entry` 独立于页和搜索按精确条目身份及路径的规范等价键解析当前目录中的条目，并返回已保存的显示拼写；返回 `null` 才表示不存在，不能根据条目不在当前页推断删除。同一身份的不同路径别名仍分别匹配。`selected_path` 的末级名称使用已有名称引用校验，父路径须与本次目录的键一致；显示路径可能额外包含仓库名称前缀，不能把这个前缀计入仓库内存储路径的 200 个 UTF-16 代码单元预算而误拒合法条目。每次最多增加一项投影。目录分支只返回摘要，不携带 `entries/file_contexts/file_branches/inherited_files/directory_refs`；分支数量本身尚未分页。

批量准备通过只读 `POST /api/repositories/{repo}/directory-branches` 提交 `{path,sources:[{directory_branch_id,expected_tree_id}],entries:[{entry_id,path}]}`。服务器核对捕获的作用域、子树头和直接子项，返回摘要及 `{entry_id,path,selected_path}`；`selected_path` 关联原选择，避免别名子集被误认为唯一身份。源集合为空是权威结果，不得用同路径旧作用域补齐。每个数组最多 500 项、乘积最多 10000，超限明确拒绝；目标选择用 `GET .../directory-branches?path=...&summary=true`。这不是写入授权，最终变更仍重新核对全部身份与树基线，Cookie POST 仍要求 CSRF。 同身份的 NFC / NFD 等价选择视为重复输入；别名匹配按路径键区分，返回的 `selected_path` 保留请求拼写用于关联，`path` 保留已存储拼写。

| 接口 | 用途 |
| --- | --- |
| `POST /api/auth/login` | 唯一管理员登录；请求 username/password，返回 token/csrf_token/principal/expires_at |
| `GET /api/auth/me` | 当前有效会话；返回 principal/csrf_token |
| `POST /api/auth/logout` | 持久撤销当前会话；返回 logged_out |
| `POST /api/auth/password` | 验证 current_password 并设置 new_password；返回 changed/reauthentication_required，撤销全部会话 |
| `GET /api/file-manager/disks` | 可见磁盘目录入口及只读状态 |
| `GET /api/file-manager/disks/{slot}/entries?path={path}` | 有界当前目录页、后代文件数与稳定目录分支摘要 |
| `POST /api/file-manager/disks/{slot}/namespace` | 准备并提交当前目录的创建、重命名或删除 |
| `POST /api/file-manager/disks/{slot}/namespace-batch` | 显式源分支→目标分支映射；移动、复制、删除、重命名整批检查并发布 |
| `GET /api/repositories/{repo}/directory-branches?path={path}` | 只读查询冻结分支和条目；省略 path 返回所有作用域，不切换默认分支 |
| `POST /api/repositories/{repo}/directory-branches` | 只读查询捕获分支及已选直接子项的精确身份/路径事实；不创建操作，不修改状态 |
| `GET/PUT /api/repositories/{repo}/default-branch` | 查询或选择 Web/SMB 共享默认根目录分支 |
| `PATCH /api/repositories/{repo}/directory-branches/{id}` | 修改分支标签，身份、子树和内容不变 |
| `POST /api/file-manager/disks/{slot}/write-sessions` | 取得该盘目录当前文件分支的写会话；随后通过通用工作区接口上传 |
| `POST /api/file-manager/disks/{slot}/directory-branches` | `{path,name,operation_id}`：登记命名目录分支，直接引用父子树；不提前创建文件分支、不复制正文、不创建占位文件、不自动选择 |
| `POST /api/file-manager/disks/{slot}/directory-select` | 同结构请求；选择完整冻结目录子树，恢复成员、名称、层级及精确节点；不要求后代有同名文件分支 |
| `GET /api/file-manager/files/{file}/history` | 有界节点窗口与当前上下文摘要，绑定精确 `history_root`；`limit/cursor/anchor_node_id/history_root`，可成对传入 repository_id/path 区分别名 |
| `GET /api/file-manager/files/{file}/history/nodes/{node}/parents` | 单节点完整图父关系的窗口，按父节点 ID 升序续读 |
| `GET /api/file-manager/files/{file}/history/nodes/{node}/fields/{field}` | `message`／`author` 外置字段的 UTF-8 完整片段（offset/max_bytes） |
| `GET /api/file-manager/files/{file}/history/branches` | 分支固定字段摘要页（含 `pending_count`，不复制 pending 映射），按 branch_id 升序 |
| `GET /api/file-manager/files/{file}/history/merges` | 合并来源摘要页，按 (sequence DESC, merge_id ASC) |
| `GET /api/file-manager/files/{file}/history/merges/{merge}/inputs` | 合并输入对，按原序号分片（offset/limit） |
| `GET /api/file-manager/files/{file}/history/merges/{merge}/parents` | 合并时保存的父关系，按原序号分片 |
| `POST /api/file-manager/files/{file}/branches` | 从选定版本登记公共文件分支；可传 source_node_id 固定路径节点来源，不复制正文或新增节点 |
| `PATCH /api/file-manager/files/{file}/nodes/{node}` | 设置路径节点独立备注，最多128 UTF-8字节；不改正文备注或新增节点 |
| `PATCH /api/file-manager/files/{file}/branches/{branch}` | 修改公共文件分支标签，最多128 UTF-8字节；同文件内规范化名称唯一，不增加版本或目录子树 |
| `POST /api/file-manager/files/{file}/select` | 选择此文件的分支；传入 repository_id/path 时只改变该逻辑别名，不改变其他路径 |
| `POST /api/file-manager/files/{file}/metadata` | 设置或清空文件容器备注，最多 128 UTF-8 字节，禁止换行及控制字符 |
| `POST /api/file-manager/versions/{version}/metadata` | 设置或清空版本备注，最多 128 UTF-8 字节，禁止换行及控制字符 |
| `GET /api/file-manager/history` | 获授权文件实际操作记录，当前最多最近 200 项 |
| `GET /api/system/status` | 管理员读取真实 CPU、内存、磁盘容量及 CPU/磁盘功耗；不可用值为 null，功耗另含原因代码 |
| `GET /api/system/history?range={range}` | 管理员读取后台监测历史；1min/30min/1h/24h/1month（最近30天），最多361个真实聚合点，与文件操作历史分离 |
| `GET /api/capabilities` | 协议、命名策略、上传范围、全量能力及入口限制 |
| `GET /api/repositories` | 当前系统仓库列表 |
| `GET /api/repositories/{repo}/branches` | 分支及归档状态 |
| `POST /api/repositories/{repo}/branches` | 兼容目录快照入口：从指定提交登记各文件引用，不复制来源正文 |
| `POST /api/branches/{branch}/archive` | 显式归档 |
| `GET /api/branches/{branch}/tree` | 固定提交下的目录列表 |
| `POST /api/branches/{branch}/resolve` | 把路径解析为条目与确定版本 |
| `POST /api/branches/{branch}/write-sessions` | 登记会话、文件写入占用和 fencing token，不独占整分支日常写入 |
| `POST /api/workspaces` | 创建写工作区和基础 HEAD 绑定 |
| `PUT /api/workspaces/{workspace}/files/{item}/content` | 上传完整文件内容 |
| `GET /api/workspaces/{workspace}/files/{item}/upload-status` | 查询上传代际、已持久化范围、预期长度及封存状态 |
| `GET /api/workspaces/{workspace}/files/{item}/content` | 仅授权所属会话读取覆盖后的工作文件及 Range；返回 no-store，不使用不可变版本缓存 |
| `PUT /api/workspaces/{workspace}/files/{item}/ranges/{offset}` | 可选脏范围协议，只有能力已声明才可使用 |
| `POST /api/workspaces/{workspace}/namespace-operations` | 准备创建、改名、移动、删除等操作 |
| `POST /api/workspaces/{workspace}/commit` | 发起封存与提交 |
| `GET /api/operations/{operation}/outcome` | 只查询确定结果或处理中状态，供写入后的结果核对；不观察当前健康 |
| `GET /api/operations/{operation}` | 显式管理员诊断：原操作结果与当前健康观察分开返回 |
| `POST /api/workspaces/{workspace}/abort` | 仅在安全阶段中止 |
| `GET /api/versions/{version}/content` | 受授权的固定版本内容与 Range；可传 repository_id/path 按具体逻辑别名生成下载名称 |
| `GET /api/versions/{version}` | 返回获授权版本描述及可信校验清单摘要 |
| `GET /api/versions/{version}/checksums` | 返回该版本完整二进制校验清单，按当前认证及内容状态核验 |
| `GET /api/files/{file}/history` | 正文版本摘要页，按 (version_sequence, version_id) 升序；`source_count` 代替完整来源数组 |
| `GET /api/files/{file}/history/versions/{version}/sources` | 正文版本真实 `source_version_ids`，按原序号分片 |
| `GET /api/commits/{commit}` | 提交及目录树信息 |
| `POST /api/compare` | 比较版本或提交，不产生写入 |
| `POST /api/history-trees/{history}/historical-save-plans` | 从历史元数据和工作区实际写入创建新分支计划，未写文件仍引用父版本 |
| `POST /api/history-trees/{history}/merge-links` | 直接记录 source_branch_id/source_commit_id 到 target_branch_id/target_commit_id 的合并连线，不准备文件内容或冲突决议 |
| `POST /api/history-trees/{history}/purge-plans` | 预览历史内容清除、受保护引用及实际释放量 |
| `POST /api/history-trees/{history}/split-plans` | 预览切点后代闭包、ID映射、独立性及空间预算 |
| `POST /api/plans/{plan}/commit` | 以固定计划摘要和代际执行，长任务通过operation查询 |
| `POST /api/plans/{plan}/cancel` | 仅在未进入不可撤销决定时取消 |
| `GET /api/history-trees/{history}/graph` | 分别包含版本演进父边、合并标记线、PURGED 节点及拆分来源的历史图 |
| `PUT /api/policies/{policy}` | 条件创建／更新无稳定 scope 的现行路径保护规则，仅影响后续版本；稳定 scope 与保留设置须用 `backup-configuration` |
| `GET /api/events?after={cursor}` | 可续读事件流或分页事件 |
| `GET /api/backup-topologies/default` | 当前池唯一拓扑的已发布代际、盘位直接来源和物理绑定；供已认证管理员查看，不接纳其它拓扑名别名 |
| `POST /api/backup-topology-plans` | 用户指定一级/二级/后续来源映射，预检路由和数据迁移 |
| `POST /api/storage-slots/{slot}/replacement-plans` | 用户为同一逻辑角色指定替换物理盘，保留来源映射 |
| `PUT /api/primary-placement-rules/{rule}` | 条件设置仓库/目录的一级盘规则，只影响新版本；旧版另迁移 |
| `GET /api/versions/{version}/placement` | 该版本一级归属、冻结路由、每级健康和迁移残留 |
| `GET /api/admin/disks` | 磁盘状态，仅授权管理员 |
| `POST /api/admin/disks` | 当前身份准入后以 `409 OFFLINE_CONFIGURATION_REQUIRED` 拒绝在线登记；登记需要显式离线配置和拓扑迁移 |
| `POST /api/admin/disks/{disk}/drain` | 排空前检查和迁移任务 |
| `POST /api/admin/disks/{disk}/eject` | 检查后安全卸载 |

文件历史图只使用 `nodes` 以及每项的 `node_id`、`parents.edges` 和字符串 `created_at`。正文版本页不作为缺失 `nodes` 时的兼容替代；前端不再把 `version_id`、正文来源或数字时间戳解读为旧版图协议。缺失的图不会在客户端从正文列表推测重建。

文件历史按[ADR 0009](adr/0009-bounded-file-history-windows.md)采用“精确当前根上的可替换窗口”。主路由与全部辅助读取只返回有界页，不提供 `all=true`、旧整包回退或双格式解码；未知、重复参数和非法值返回 `400 INVALID_REQUEST`，游标格式错误返回 `400 INVALID_CURSOR`。

- **历史根**：`history_root` 是目标历史在当前已准入 State 的已发布历史目录中的准确 `history-manifest` 哈希（64 位小写十六进制）。首页由服务端捕获；续页、辅助读取和精确跳转必须携带来源页的根（查询参数或游标），二者同时提供必须一致。根不同返回 `409 HISTORY_CHANGED`：本历史的任何合法写入（含备注、标签、选择）都会改变根，无关历史的写入不会。冷启动从恢复所准入候选的历史目录读取同一根，不使用全池序号、热缓存或零值回退。
- **授权次序**：每页先做当前认证／撤销及对象授权，再比较根；游标与根都不是授权凭据，也不证明当前盘健康。游标在请求边界解码、重新解析并绑定路径中的文件，跨文件或跨查询的游标为 `400`，不会以 `409` 暴露另一对象是否变化。
- **节点页**：节点按 `(sequence DESC, node_id ASC)`，`limit` 默认 50、最多 200。每个节点给出 `parents.status`：`complete` 且边为空才表示该读取范围内没有父关系；`more` 必带绑定该节点的续读游标。父边标注 `in_window`，`false` 表示父节点存在但未驻留本页，不是根、丢失或损坏。`current` 只标记上下文当前节点。`anchor_node_id` 与 `cursor` 互斥，定位到使该节点成为页首的窗口。主响应不再重复正文 `versions` 全集。
- **字段与关系**：`message`、`author` 没有写入长度上限，仅在不超过 256 UTF-8 字节时内联，否则经字段分片完整读取；分支页不复制 `pending_file_versions`，合并页只给计数，输入对、保存的父关系和正文来源按原序号分片，单节点高入度父集合可持续续读。
- **字节预算**：每页完整编码（含外壳、上下文、游标与转义）不超过 `MAX_HISTORY_PAGE_BYTES` = 4 MiB，低于客户端 8 MiB 接收上限。服务端逐条编码已选记录，超出剩余预算的记录留给下一页，响应由已编码字节直接拼接；非末页必有后续游标，末页游标为 `null`，不存在空页携同一游标的循环。字节截断后的 `in_window` 按实际驻留节点重新判定。

原始历史查询在解码前受 128 KiB／8 个字段的解析预算约束；之后仍严格拒绝未知或重复字段。节点、分支、合并和正文页的游标绑定页大小；父关系游标允许改变合法 `limit`，以便从节点预览继续展开，但始终绑定同一根、文件和节点并检查边界确实是该节点的父项。前后方向均可续读；字节不足时正向保留规范顺序前缀，反向保留离边界最近的后缀，不能跳过未返回项。

字段 `offset` 是 UTF-8 字节边界，落在字符内部或 `max_bytes` 放不下下一个完整字符时返回 `400 INVALID_REQUEST`，不静默改写偏移或返回无进展片段。请求 `max_bytes` 默认 64 KiB、最多 1 MiB；服务端在复制前再以六倍最坏 JSON 转义及固定外壳预留缩小片段，故实际片段可以更短，客户端使用返回的 `next_offset`。准备／取消仅本地计划不改变已发布历史根；实际历史发布仍须重新读取最新根。

共享 JSON 客户端协议的响应上限仍为 8 MiB；Web 在 `Content-Length` 预检、流式读取及 JSON 解码处执行检查，超限报告 `RESPONSE_TOO_LARGE`。历史页的服务端预算是独立且更小的编码上限，不以提高客户端上限换取容量。永久历史、节点、父关系、合并来源与操作收据不因分页裁剪，窗口驱逐只释放请求与浏览器工作集。

文件分支登记回执固定创建时的可见父 ID、`inherited:true` 与 `own_version_id:null`；目录分支回执固定目录分支身份和父子树引用，不表示预先创建文件分支。之后首写不会改写原回执。当前内容状态通过读取接口查询，不能以当前状态回填旧操作结果。

上述文件管理接口已实现，具体字段以服务端路由及机器契约为准。其后的高级接口表包含设计基线，不能推断每项均已开放。旧整树创建、历史保存与拆分接口在已包含文件分支的历史域中明确拒绝，避免把文件快照当作完整目录树。目录分支具有稳定的 directory_branch_id，其每次实际组成变化形成新的 head_tree_id；这两个身份不能与文件分支和文件版本混用。保留的受控 HTTP 正文下载接口通过 Content-Disposition 使用所选视图中的逻辑名称，Web 管理界面不提供文件上传、下载或预览入口；内部不可变 storage_name 不随下载或容器改名而改变。

普通 workspace commit 只针对明确当前分支，SMB 的目标由共享默认根目录分支及其冻结子目录组成决定；历史节点保存通过 historical-save-plans 登记新分支，仅实际 staged 的文件产生 FULL 版本，不能沿用普通提交直接覆盖主分支。普通内容提交最多一个父提交，不能由用户任填父列表；这不限制统一文件节点图的多父 `parent_node_ids`。旧 `MergeLink` 不改写提交、正文或HEAD，新文件分支合并按独立请求推进参与引用并保存来源，原正文证明不变。树拆分使用独立结构迁移接口，不能把 rebase/强制重置作为隐藏入口。

<a id="s15-03"></a>

## 15.3 提交请求示例

```json
{
  "operation_id": "op-example-0007",
  "workspace_id": "ws-example-0007",
  "repository_id": "repo-example",
  "branch_id": "br-main",
  "expected_head": "commit-example-0006",
  "fencing_token": "17",
  "message": "更新设计文档",
  "requested_protection": "strict",
  "file_placement": {"item-report": {"primary_slot_id": "P01", "target_copies": 3}},
  "expected_topology_generation": "1",
  "sealed_manifest_hash": "<已封存工作清单的哈希>",
  "request_digest": "<请求规范表示的哈希>"
}
```

请求先通过验证并登记操作，长任务可返回 `202` 和操作 ID，明确表示尚未完成。内容提交的最终成功结果包含 `commit_id`、分支新代际、逐版本r(v)、一级归属、required_slots及每级确认状态、拓扑/绑定代际、成员代际和业务序号。即时变更回执与 `GET /api/operations/{operation}/outcome` 只返回原操作的发布结果，不带 `current_health` 或 `checked_at`，不会为了确认发布而枚举历史版本、读取内容副本或取得当前时刻。

显式 `GET /api/operations/{operation}` 保留真实管理员诊断能力：相同的固定结果之外，必填 `current_health` 为 `null` 或 `{state, checked_at}`。`null` 明确表示 NotObserved（操作尚未完成或没有可观察作用域），不表示健康，也不伪造观察时间；对象表示 Checked，`state` 为 `HEALTHY`、`DEGRADED`、`UNAVAILABLE` 或 `UNKNOWN`，`checked_at` 是实际观察的 UTC 时刻。已成功提交后来掉盘时，原发布结果与保护证据不变，只有独立健康观察变化。两条 GET 均保留当前认证、主体绑定、原 operation ID 和错误来源，不通过切换端点绕过准入。

收到超时后只能查询或用**同一个 operation_id 和相同请求**重试。相同 ID 携带不同请求必须返回 `IDEMPOTENCY_KEY_REUSED`，不能覆盖原操作。

<a id="s15-04"></a>

## 15.4 错误码基线

进入应用路由的 HTTP API 请求在响应开始前发现的错误统一返回 `{"error":{"code":"…","message":"…","retryable":false,"request_id":"req_…"}}`，HTTP 状态码不在错误对象中重复。错误封套涵盖业务拒绝、查询/路径参数解析、请求体读取、路由及后台任务失败，不以框架默认的纯文本错误替代；`request_id` 供诊断关联，不是业务 `operation_id`。

查询提取器拒绝的缺失字段、重复字段或非法参数值返回 400 `INVALID_REQUEST`；未知路由返回 404 `NOT_FOUND`，`/api` 与 `/api/` 命名空间不能落入同名静态文件。已知路由使用不支持的方法返回 405 `METHOD_NOT_ALLOWED` 并保留 `Allow`。这些错误不绕过原有认证与权限检查。

有界 JSON 请求体超限返回 413 `PAYLOAD_TOO_LARGE`，包括未提供或不可信 `Content-Length` 的流式输入；登录、修改密码和一般业务 JSON 的上限分别为 8 KiB、16 KiB 和 1 MiB。登录与改密额度容纳合法密码的 JSON Unicode 转义膨胀，解码后的密码规则不变。未超限但发生读取错误的请求体返回 400 `INVALID_REQUEST`。普通 JSON 请求体（包括登录和改密）须在 30 秒内读完，期限到达返回 408 `REQUEST_TIMEOUT`；初始化/恢复 JSON 另有更严格的十秒外层期限。文件上传继续使用独立的流式长度、空闲/总期限及暂存准入规则，不套用 JSON 请求体上限或普通请求体期限。

阻塞请求任务异常退出或取消属于 500 `INTERNAL_ERROR`，不能误报为客户端输入错误或将 panic、内部路径等细节发给客户端；原有受控业务错误及监测服务的 503 语义保持不变。`retryable` 表示错误类别允许重试，不承诺原操作未执行；变更请求仍须按相同 `operation_id` 查询或重试。内容响应头已发送后发生的校验或读取失败只能中断响应流，不能再改写 HTTP 状态或伪装成 JSON 错误封套；HEAD 也不发送错误响应体。

| 错误码 | 含义 | 客户端必须做什么 |
| --- | --- | --- |
| `INVALID_REQUEST` | 查询、路径、JSON 或请求体输入无效，HTTP 400 | 修正请求，不将框架拒绝视为服务不可用 |
| `NOT_FOUND` / `METHOD_NOT_ALLOWED` | 路由不存在或方法不受支持，HTTP 404 / 405 | 检查目标及 `Allow`，不回退为静态页面 |
| `PLAN_NOT_FOUND` | 经安全读取确认本地准备计划不存在，HTTP 404，不可重试 | 核对计划 ID；已发布操作按原 operation_id 查询，不把计划缺失当作磁盘 I/O 故障 |
| `PAYLOAD_TOO_LARGE` | 有界请求体超过入口上限，HTTP 413 | 缩小请求或使用对应的流式上传入口 |
| `REQUEST_TIMEOUT` | 有界 JSON 请求体未在读取期限内完成，HTTP 408 | 检查连接与请求大小；与正文传输的 `TRANSFER_TIMEOUT` 区分，不据此推断其他请求的操作结果 |
| `INTERNAL_ERROR` | 请求任务异常或内部状态失败，HTTP 500 | 保留请求 ID，变更请求先查询原操作结果 |
| `NAME_INVALID_ENCODING` | 非法 Unicode 输入 | 显示错误，不替换乱码继续 |
| `NAME_INVALID_CHARACTER` | 禁止字符或不在允许集 | 展示原名称和原因 |
| `NAME_RESERVED` | 保留设备名或系统保留名 | 要求用户明确选择新名称 |
| `NAME_NOT_NORMALIZED` | 新名称不是规定 NFC 形式 | 展示候选，确认后另发规范请求 |
| `NAME_COLLISION` | 大小写/规范等价重名 | 不自动加后缀 |
| `NAME_TOO_LONG` / `PATH_TOO_LONG` | 名称或路径超限 | 展示超限组件和预算 |
| `DIRECTORY_BRANCH_CONFLICT` | 依据的目录子树或写会话中的选择已过期 | 整批不发布；保留暂存内容，刷新视图后由用户重新决定 |
| `BATCH_MAPPING_CONFLICT` / `BATCH_SCOPE_CONFLICT` | 分支映射、物理归属或作用域不一致 | 修正明确映射，不自动覆盖或跳过 |
| `DIRECTORY_HISTORY_SPLIT_UNSUPPORTED` / `FILE_NODE_HISTORY_SPLIT_UNSUPPORTED` | 当前拆分协议不能保留目录组成或文件路径节点闭包 | HTTP 400 受控拒绝，不执行会丢失引用的拆分 |
| `DIRECTORY_SCOPE_REQUIRED` | 旧盘根包含多个独立命名空间，无法组成单事务 | 进入一个具体文件夹后操作 |
| `FILE_BRANCH_CONFLICT` | 工作区打开后文件选择已改变，含两个引用分支共用来源的情况，HTTP 409 | 保留修改并重新选择目标、获取工作区，不按相同来源ID继续写入 |
| `HEAD_CONFLICT` | 分支基础头已变化 | 保留工作区，不覆盖或自动合并 |
| `BRANCH_WRITE_BUSY` | 分支正在拆分或维护 | 保留工作区，等待维护完成 |
| `FILE_WRITE_BUSY` / `FILE_CONFLICT` | 同文件写入占用或基础内容已变化 | 保留修改，不静默覆盖 |
| `BRANCH_SWITCH_BUSY` | 有脏工作区或不安全句柄 | 不强制切换 |
| `WORKSPACE_DEPENDENCY_BUSY` | 屏障依赖其他尚未封存的文件或目录事件 | 保留脏数据，先处理所指依赖或显式批量提交 |
| `LEASE_EXPIRED` / `FENCED` | 写会话已失效 | 停止提交，重新获取授权 |
| `REPLICA_TARGET_UNAVAILABLE` | 固定路由中某级缺少健康完整副本 | 定位slot/source/level，不偷降、不换无关目标 |
| `NO_SPACE` / `QUOTA_EXCEEDED` | 空间或配额不足 | 保留待处理状态和已知错误 |
| `CONTENT_UNAVAILABLE` | 请求版本无可读健康副本 | 不替换成其他版本 |
| `CHECKSUM_MISMATCH` | 内容校验失败 | 隔离并切换相同版本副本 |
| `COMMIT_IN_DOUBT` | 无法判定提交传播结果 | 查询原操作，禁止自动清理 |
| `METADATA_UNAVAILABLE` | 元数据保护或恢复条件不足 | 阻止危险写入 |
| `RECOVERY_REQUIRED` | 可信成员、日志或配置证据不足 | 只开放可验证读取与恢复，不自动重置池 |
| `POLICY_CHANGED` | 准备期间保护策略代际改变 | 按新条件重新准备，不能修改已有候选 |
| `BACKUP_CONFIGURATION_REQUIRED` | 通用策略更新试图修改稳定 scope 规则或仅供文件绑定使用的保留设置，HTTP 400 | 改用对应文件／目录的 `backup-configuration`，重新读取稳定身份和自身代数 |
| `FORMAT_UNSUPPORTED` | 必需盘格式或协议特性未知 | 停止写入/自动重放，使用受支持读取器 |
| `SAVE_MODE_UNSUPPORTED` | 请求不支持的保存或分支类型 | 明确拒绝；实际内容只接受 FULL，分支创建先记录与来源引用 |
| `HISTORY_NODE_NOT_FOUND` | 所选连线端点不存在或不在请求历史域中 | 重新选择有效节点，不以分支当前 HEAD 偷换端点 |
| `CONTENT_PURGE_PENDING` | 清除屏障已建立，新的内容读取或固定暂时被阻止 | 等待计划完成或安全取消，不改用其他版本，不提前视为已清除 |
| `CONTENT_PURGED` | 请求版本payload已按规则清除 | 显示节点和清除事件；内容GET可映射HTTP 410 |
| `PURGE_BLOCKED_BY_REFERENCE` / `CONTENT_PINNED` | 活跃引用、工作区或租约阻止删除 | 展示保护来源，不强删仍需内容 |
| `TREE_MOVED` / `SPLIT_PLAN_STALE` | 拆分已迁移绑定或计划代际过期 | 重新授权共享或工作区，重新预览 |
| `CROSS_TREE_DEPENDENCY_FORBIDDEN` | 请求将两独立树建立活动依赖 | 改为独立全量导入，不静默连接历史 |
| `PRIMARY_PLACEMENT_REQUIRED` | 无用户指定/继承的一级盘规则 | 选择一级盘，不随机存入其他盘 |
| `BACKUP_ROUTE_INCOMPLETE` / `ROUTE_TARGET_UNAVAILABLE` | 指定链深不足或所需级别盘位不可用 | 补全映射、恢复/显式替换目标；不跳级或自由回退 |
| `PLACEMENT_CHANGED` / `TOPOLOGY_INVALID` | 路由、来源、绑定代际变化或映射不合法 | 重新预览，拒绝环、跨级、重复后继和同盘多级 |
| `UNSUPPORTED_FILE_SEMANTICS` | mmap、链接等未支持操作 | 显式报错，不空实现成功 |
| `IDEMPOTENCY_KEY_REUSED` / `REQUEST_DIGEST_MISMATCH` | 操作 ID 对应不同输入或摘要不匹配 | 查询原操作，不能修改原输入后继续使用同一 ID |
| `RESOURCE_LIMIT_EXCEEDED` | 工作区、上传、句柄或队列达到准入上限 | 停止新增工作，查询或处理已有工作，不删除脏数据腾空间 |
| `AUTH_STATE_INVALID`（改密） | 管理员凭据代数已达 UInt64 上限，无法推进，HTTP 409、`retryable:false` | 保留凭据、会话和磁盘状态；等待或重试不能解决，不回绕、重置或自动恢复 |
| `UPLOAD_GENERATION_CONFLICT` / `UPLOAD_RANGE_CONFLICT` | 上传代际变化或重叠范围内容不同 | 查询原上传状态，保留本地内容，显式创建新代际 |
| `TRANSFER_TIMEOUT` | HTTP 正文空闲或总期限到达；响应头前为 408，已开始的正文终止为流错误 | 查询上传状态后重试；已有字节/操作记录保留，阻塞 I/O 可能仍在完成 |
| `UPLOAD_INCOMPLETE` | 长度或有效范围未满足封存条件 | 补齐缺失范围，不能补零冒充完整上传 |
| `LISTING_CHANGED` | 当前目录分页的业务快照或配置已改变，HTTP 409 | 丢弃旧游标并重新加载首页，不拼接不同快照 |
| `HISTORY_CHANGED` | 文件历史页所绑定的精确历史根已不是当前根（本历史发生了合法写入，包括备注／标签／选择变化），HTTP 409 | 丢弃该根下的所有页与游标，重新读取首页或按节点 ID 精确跳转；已选动作输入保留但须重新确认，不自动更新 expected head |
| `INVALID_CURSOR` | 事件游标超过当前发布序号，HTTP 400 | 核对游标所属服务及发布状态，再重新轮询；不按时间过期处理 |
| `CONFIG_CONFLICT` / `CONFIG_INVARIANT_VIOLATION` | 配置与权威状态或协议常量冲突 | 进入配置核查，不自动改盘上状态或降低保护 |

业务 API 向 Web 返回结构化错误，Web 展示错误码、请求 ID、原因和待处理状态。SMB 入口单独返回对应协议状态，详细原因记录在服务器活动中心，供已认证管理员查询。

<a id="s15-05"></a>

## 15.5 能力声明与准入

客户端通过 `/api/capabilities` 读取 `name_policy`、`path_policy`、`unicode_data_version`、支持的操作、`max_upload_bytes` 和平台能力。项目采用单一当前协议与格式，不声明数字协议版本或可读旧格式列表；Unicode 版本用于标识固定的外部字符数据，不是项目协议版本。拓扑代际、备份路由和具体资源状态通过各自查询接口读取。

遇到不能正确执行的规则或操作应拒绝相关写入并说明原因，不用其他规则静默近似。服务器不能因为客户端缺少某个按钮就省去认证及业务约束检查。

当前能力声明 `full_only:true`、`full_branches:false`、`branch_creation:"record-until-write"`、`branch_content:"inherit-parent-until-first-write"`，`directory_branch_creation:"inherit-directory-tree"`、`namespace_batch:true`，区分内容必须 FULL 与登记分支只保存冻结引用。历史另存、合并关系标记、清除、独立树拆分和用户备份拓扑按各自能力及已开放入口报告。SMB 暴露用户所选默认根目录分支的基础操作，管理 API 能力不能被解释为 SMB 也支持。画线请求固定节点，不要求固定当前 HEAD、解决冲突或提供文件内容。客户端请求不支持的保存方式或操作时明确拒绝，不能近似执行。

当前另开放文件合并、节点标签与scope备份配置，具体字段见[机器契约](contracts/openapi.json)和[合并、标签、备份规则](design/file-merge-tags-backup.md)。`merge_behavior:"explicit-file-node-adoption"` 区分文件结果采用与旧 `merge-links` 纯关系登记；`directory_branch_merge:false`、`directory_merge_status:"design-pending"` 明确目录分支合并待用户确认，暂不实现且无执行入口。

| 已实现入口 | 写入基线与结果 |
| --- | --- |
| `POST /api/file-manager/files/{file}/merge` | 每个公共文件分支 `expected_node_id`、结果 `chosen_node_id`、归属 `surviving_branch_id` 与 `confirmed:true`；只推进参与引用，被合并分支成为只读历史别名 |
| `POST /api/file-manager/files/{file}/tags` | 每个显式 `source_node_id` 与所关联分支当前 `expected_node_id`，按 `add/remove` 增量创建继承标签的元数据子节点；最多16标签、每项128 UTF-8字节；历史源不推进分支head |
| `PATCH /api/file-manager/files/{file}/nodes/{node}` | 路径、标签与合并节点的独立128字节备注；不修改正文备注或正文proof |
| `GET/PUT /api/repositories/{repo}/backup-configuration` | GET返回自身与有效策略代数、scope身份及实际备份路线，不返回账户权限投影；PUT固定 `expected_scope_id`/`expected_generation`、含主盘总份数和每个备份slot的用户逻辑前缀，并回填可用历史版本 |

保护规则必填 `inheritable`：`true` 表示当前可按 scope／路径继承的规则，`false` 表示仅经文件绑定生效的保留设置，不允许缺省或旧格式推断。重配沿用现行 `policy_id` 并推进 `generation`，仅把范围外仍绑定旧规则的文件转到共享的旧设置快照，保留其份数和路径。保留设置不参加精确 scope／祖先查找，不受父目录配置覆盖；显式文件配置可以重新绑定。GET 对保留设置返回 `inherited:false`，`effective_policy_id`／`effective_generation` 描述绑定设置；自身 `policy_id`／`generation` 仍只描述精确现行规则，不存在时为 `null`／`"0"`。没有绑定的保留设置清理，空 scope 的现行规则继续保留。

节点标签和合并复用已有正文，`versions`仍只包含真实正文版本。统一 `nodes` 增加 `tags`/`merge` 类型与 `tags` 快照；独立 `merges` 保存全部输入来源，图边略去会形成环的关系。只读别名可通过历史节点下载和派生新分支，不能直接选择为可写分支。条件冲突返回409，标签限制/无效prefix返回400；不能因客户端先筛选了分支就省略服务端校验。

主盘文件列表把命名仓库的虚拟目录与磁盘根仓库的顶层条目放在同一显示命名空间。
若二者名称按同一命名策略规范化后相同，根列表及该首级路径下的读取、建目录、写会话和目录分支操作返回
`409 NAME_COLLISION`，不会优先选择某个仓库或创建工作区。其他无冲突路径仍可使用；显式仓库、分支或条目身份
不受这项显示路径检查影响，可据此改名解决冲突。检查不修改权威 State，也不自动改名或隐藏其中一方。

备份盘的 `entries` 按用户为该slot配置的逻辑路径浏览，`backup:true`、只读且不提供目录分支操作。条目的 `path` 是备份路径，`repository_path` 是真实主命名空间路径；主目录改名不改变既定备份路径。该槽没有当前正文时显示其最新可用旧正文，预览和下载必须固定条目的 `version_id`；已无live alias时返回 `historical_only:true`、`repository_path:null`，历史查询省略路径上下文。规范化路径冲突返回 `BACKUP_PATH_CONFLICT`，授权过滤先于冲突检查。

备份配置查询以 `backup_routes` 列出该scope所有实际历史来源链，`backup_slot_ids` 为各链备份slot的并集，`max_copies` 取链长度最小值。选择n份时应对每条链前n项取备份并集并收集独立prefix，不能只按当前默认链填写；旧 `available_route_slot_ids` 仅保留默认来源链作兼容展示。

对移动端、桌面及浏览器分别报告可用能力，不能发一个笼统的 `all_platforms_supported=true` 代替真实能力矩阵。

<a id="s15-06"></a>

## 15.6 不同操作的结果类型

`operation_id` 标识用户请求，`commit_id` 标识目录状态提交，两者不得等同。所有操作查询至少返回操作类型、处理状态、确定结果或明确的结果未知状态；只有会产生目录提交的操作才返回相应 commit_id。

| 操作类型 | 结果必须包含 | 不得隐含执行的动作 |
| --- | --- | --- |
| 内容保存 | commit_id、目标分支、新版本和发布时保护情况；当前健康由显式诊断另行观察 | 覆盖旧版本或把本地暂存当成已保护 |
| 分支创建 | 文件/目录分支稳定 ID、冻结来源节点、`copied_files:"0"`、`placeholder_files:"0"`及待写入状态 | 复制来源正文、虚增版本、将冻结来源引用报告为新增内容已保护或改写原主分支 |
| 历史保存 | 新 branch_id、起点、实际写入的 FULL 版本及未写入来源引用清单 | 隐式复制工作区未写文件，或改写原主分支 |
| 旧关系标记 `MergeLink` | merge_link_id、固定的来源与目标节点、关系记录保存结果 | 新建内容版本、目录快照、额外副本或推进 HEAD |
| 文件分支合并 `FileBranchMerge` | merge_id、全部输入head、chosen/result节点、归属分支、复用标志及受影响目录分支 | 自动拼接内容、复制正文或修改未参与引用 |
| 历史内容清除 | 逻辑墓碑结果、物理清理进度、离线残留及实际释放量 | 将墓碑发布或任务受理视为所有介质已清理 |
| 树拆分 | 原树与新树 ID、迁移映射、各树活动状态、主分支及共享处理结果 | 只发布一侧，或保持活动跨树内容依赖 |
| 拓扑或盘位变更 | 已生效代际、受影响路由、迁移结果和残留状态 | 只改配置标签便显示全部数据合规 |

HTTP 202 或任务已受理只表示操作登记，不表示正常保存成功。对取消请求，结果必须区分“已安全取消”和“已进入不可撤销决定、继续完成”；不得将客户端停止等待视为服务端已回滚。

<a id="s15-07"></a>

## 15.7 请求摘要、幂等与结果保留

核心在池内按 `operation_id` 索引操作，并固定绑定认证主体、请求摘要和操作结果；已存在的 ID 不能被其他主体或不同输入复用。幂等命中不绕过认证与已授权读取入口；进入核心回放后，完整匹配的已发布请求返回原操作结果，不重复执行业务变更。HTTP 提供按 ID 查询操作，不设独立登记接口；响应未知时查询原操作或以完整相同输入重试，受理与发布状态以回执字段为准，不能仅凭 HTTP 状态推断已发布。

仓库创建与整树分支创建的回执 `result` 分别保存 `created_repository`、`created_branch`，固定首次发布时的完整对象。创建接口的外部对象形状不变；后续写入、归档或 SMB 绑定变更不能改写原创建响应。回放只解码已验证请求和主体的原回执，不读取当前对象替代，也不为缺少这些字段的旧回执提供兼容回退。

文件管理磁盘根的 `create_disk_root` 同时承担确保根存在的职责：全新、未使用的 operation_id 可直接取得已有根的当前对象，不发布新创建回执；若该 operation_id 已有发布记录，仍须验证原主体和请求并回放原回执，不能由此快捷路径跳过幂等校验。

需要显式 `request_digest` 的 HTTP 内容提交使用 `BLAKE3("zxs:request" + NUL + JCS(envelope))`。envelope 固定包含 `method`、`route`、`path_parameters`、`query` 和 `body`；method 大写，route 使用契约中的路径模板，路径参数单独传入，query 仅包含 schema 定义的有业务意义参数。body 排除唯一的顶层 `request_digest`；operation_id、目标、前置条件、fencing_token、计划摘要、工作清单摘要及全部已接纳扩展都参与计算。身份由已认证通道绑定；认证令牌、请求跟踪 ID、网络超时和重试次数不进入摘要。拒绝未知字段和重复字段后再计算，不自行填充会随环境变化的默认值。

其他已实现的元数据命令由核心使用相同域前缀，对 `{kind, principal, body}` 计算内部摘要；`operation_id` 是账本索引身份。文件标签与分支合并使用单一的[类型化核心请求](../src/engine/src/business/file_requests.rs)：`location` 明确为 `null` 或完整的 `repository_id/path` 对象，不能遗漏位置约束。完整相同的请求可在文件改名后回放原结果；改变、添加或省略位置，以及改变正文或主体，均不得匹配原回执。当前位置只在新变更执行前校验，不以变化后的路径替代原请求。不会为缺少位置证据的摘要提供兼容回退；查询已有回执不要求重新构造请求。

内容提交必须绑定不可变 `sealed_manifest_hash`，覆盖每项最终长度、内容哈希、上传代际和命名空间操作顺序。相同工作区 ID 下字节变化必须产生新清单和新操作，不能沿用已登记请求指向可变工作区。流式内容的摘要绑定目标、上传代际、偏移、长度和内容哈希，不将二进制流强行表示成 JSON。

操作账本保存受理、阶段进展和固定终态。未决操作保留全部输入及恢复依据；已结束操作可把过程日志压缩进检查点，但必须在池生命周期内保留 ID、账户、请求摘要、终态及结果引用。内容清除不删除操作去重记录。配额耗尽时拒绝新登记，不通过遗忘旧 ID 恢复容量。工作区查询、取消和恢复通道须保留处理额度；销毁整个池是独立高权限流程。

<a id="s15-08"></a>

## 15.8 上传代际与续传

上传状态 GET 明确区分 `RECEIVING`（尚未完成发布）、`CHECKSUM_MISMATCH`（完整接收但实际哈希不符）、`COMPLETE` 与 `SEALED`。`durable_ranges` 描述已持久化字节及其实际哈希；顶层 `content_hash` 是声明的预期哈希，校验失败时两者不能互换。查询只投影已有记录，不重新读取或全量哈希正文；不返回 owner、逻辑路径或暂存文件路径。

`CHECKSUM_MISMATCH` 不表示工作区被终止或数据可丢弃。重启后保留该状态和坏源；同代际仍按既有重放规则校验持久化前缀，相同错误字节再次校验失败，修改字节返回 `UPLOAD_RANGE_CONFLICT`，修改长度或预期哈希返回 `UPLOAD_GENERATION_CONFLICT`。修正内容须显式启动更高代际，保留旧代际恢复证据。未完成上传可同代际完整重放并补齐；引擎已有同代际或更高代际的发布结果仍优先于暂存状态。

完整上传与可选范围上传都绑定 workspace、item、upload_generation、最终逻辑长度和预期内容哈希。第一版可以只公布完整上传能力；未实现范围协议时返回不支持，不近似接收。

| 情况 | 规定行为 |
| --- | --- |
| 完整 PUT 中断 | 保留已接收工作数据并标注未完成；完整上传模式的重试在同代际以相同内容重新传输，不把部分文件当作完成 |
| 范围上传重试 | 同代际、同偏移、同长度、同哈希返回原接收状态；其他重叠范围一律拒绝，变更内容须显式建立新代际 |
| 截短或改变最终长度 | 创建新上传代际并使旧写 token 失效；旧唯一脏副本在新内容可恢复或明确放弃前保留 |
| 查询已接收范围 | 区分内存接收与已持久化范围，只有后者可用于崩溃后的续传依据；两者都不表示已完成业务提交 |
| 新文件封存 | 范围必须完整覆盖 `[0, logical_size)`；空文件长度零且无范围，仍校验整文件哈希 |
| 基于旧版本的范围修改 | 清单固定 base_version_id 及基线哈希；未上传范围只从获授权且受引用保护的该版本补齐，再验证最终完整文件 |
| 封存与继续编辑 | 已封存清单不可变；后续写入进入新代际，旧操作继续引用旧清单 |

范围使用无符号 64 位偏移与长度，检查相加溢出、越界及请求上限。接收前申请逐盘和账户暂存预算；额度不足返回明确错误并保留已接收数据。最终 ACCEPT 前仍重新核对主体与业务前置条件。

<a id="s15-09"></a>

## 15.9 分页、事件和校验清单

当前文件管理目录分页使用[第 15.2 节](#s15-02)的 `snapshot`、`next_cursor` 和 `previous_cursor`，不是固定旧 `commit_id` 的历史快照续读。游标绑定盘位、路径、搜索、页大小、排序及业务快照；业务状态或配置变化返回 `409 LISTING_CHANGED`，查询条件不匹配返回 `400 INVALID_REQUEST`。客户端须重新加载首页，不能拼接不同快照。

目录游标只是无服务端缓存的翻页位置，不是授权凭据；当前没有账户绑定、签名或有效期字段，不提供防篡改令牌或 `PAGE_SNAPSHOT_EXPIRED` 语义。每页仍重新验证当前管理员及业务状态，不能依靠游标授权。搜索 `query` 最多 1024 个 UTF-8 字节，游标最多 32768 个 ASCII 字符；未知查询字段、重复字段、非法值和游标编码均明确拒绝。文件历史游标另按第 15.2 节绑定精确历史根，不与目录游标互换。

当前 `GET /api/events?after={sequence}` 是已认证的事件轮询，`after` 为规范 UInt64 十进制字符串，省略时为 `"0"`。服务端按已发布 `sequence` 顺序返回其后的最多 1000 项；`next_cursor` 是最后返回的序号，没有新事件时保持输入值。超过当前发布序号的游标返回 `400 INVALID_CURSOR`。事件包含 `sequence/operation_id/kind/object_id`，没有事务内 `ordinal`、订阅连接、可配置页大小、事件窗口到期或快照续订协议。客户端可按序号去重并继续轮询；事件用于提示刷新，不能代替当前读取的认证和状态核验。

独立校验流程为：取得获授权版本描述 → 获取完整 checksums.bin → 验证清单哈希、头部、段数与长度 → 按段获取内容并核对段哈希。清单接口与内容接口执行相同的授权、PURGE_PENDING/PURGED 屏障，不由公开静态目录提供。HTTP Range 仅定义单范围基线；多范围支持须单独声明，未支持时返回明确错误。HEAD、304、206 也先授权；416 应返回当前表示长度。示例与测试需覆盖空文件、尾段、越界及条件失败。

<a id="s15-10"></a>

## 15.10 传输状态与契约交付

| 结果类别 | HTTP 基线 | SMB 桥接要求 |
| --- | --- | --- |
| 任务受理 / 查询成功 | 202 / 200，查询响应体仍可能是处理中或失败 | 不映射成 FLUSH 已持久化成功 |
| 输入、编码或摘要错误 | 400；明确不支持的语义用 422 | 映射相应无效参数或不支持状态 |
| 未知路由 / 不支持的方法 / 请求体超限 | 404 / 405（含 Allow）/ 413，仍使用统一错误封套 | 不以 HTTP 路由语义替代协议状态 |
| 未认证 / 无权限 | 401 / 403；隐藏对象存在性时统一使用 404 策略 | 使用协议认证或访问拒绝状态，不返回空目录 |
| 条件、代际或幂等冲突 | 409；HTTP If-Match 失败用 412 | 返回占用、冲突或前置条件对应状态 |
| 内容已清除 / 范围越界 | 410 / 416 | 返回相应不可访问或范围错误，并保留活动中心原因 |
| 准入限流 / 工作资源上限 | 429，可给 Retry-After | 新请求返回资源不足；已受理请求保持原操作关联 |
| 空间或配额不足 | 507 | 映射磁盘满或配额状态，不自动降低保护 |
| 服务或元数据不可用 | 503 | 返回服务不可用；已发 ACCEPT 的操作保留未知状态，不改写成确定中止 |

HTTP 状态描述本次调用，operation 的终态描述原业务请求，二者不可互换。SMB 的精确 NTSTATUS 值与回调必须在锁定版本的映射表和样机记录中冻结；以上分类不代替真实协议错误测试。

准确的发布后工作区终态镜像失败、显式降级恢复的发布后索引重建／工作区核对失败，返回现有错误封套的 `503 RECOVERY_REQUIRED`、`retryable:false`，消息保留原 operation ID 并明确已发布；内部保留原收据、阶段、主体绑定和具体 source。这不是确定拒绝，也不是自动重新提交的许可。当前客户端仍保留原操作为待查询状态，不从错误消息解析新的收据；使用 outcome 查询核对固定发布事实，当前健康只在显式诊断查询中观察。只有这几个已知发布后上下文改变分类，普通输入／路径拒绝仍保持原状态码。严格重开可能先因同一故障失败，管理离线核对与普通操作查询的可达性尚未闭环，详见[事务恢复](10-transactions-and-recovery.md#s10-10)。

[契约附件](contracts/README.md)提供核心保存请求、操作结果、版本描述及配置约束的机器可读草案和固定样例。OpenAPI 明确列出当前已描述端点；本章其他拟定接口在补齐 schema、错误映射与验收前不能标记为已冻结。规范、schema 和样例出现差异时阻止发布并修订，不由某个客户端自行选择宽松解释。

---

[文档目录](README.md) · [上一章：身份、权限与运行隔离](14-security-and-permissions.md) · [下一章：Web 与内部管理命令行界面](16-client-interfaces.md)

### 工作区保留证据容量诊断

`GET /api/workspaces/capacity` 要求当前有效会话，返回 `WorkspaceCapacity`：

- `principal` 仅统计当前授权身份的保留记录和元数据预留量；`records` 与 `metadata_bytes` 各包含 `used`、`limit`、`remaining`，全部为规范 UInt64 十进制字符串。
- `global` 与 `principal` 均必填且不得为 null，分别返回全池汇总和唯一管理员的主体预算。现有响应字段、每主体及全池上限均保留，不扩大容量；请求参数不能指定另一主体。
- `warning` 为 `ok`、`near_limit`（全池或主体的任一预算达到 80%）或 `exhausted`。`remaining` 下限为零。
- `automatic_retirement` 固定为 `false`。提交、终止及过期记录仍占用保留证据预算；本接口不删除证据、不修改预算，也不承诺下一次写入一定获准。后端不支持时返回 501。

### 引擎错误到 HTTP 的现行映射登记

服务器集中登记现有内部错误别名与公开 HTTP 状态，并测试别名唯一性、目标状态登记、重试语义和 500 诊断遮蔽。`RESOURCE_LIMIT → RESOURCE_LIMIT_EXCEEDED` 等转换是边界适配，不意味着每个内部错误码均为公共契约。未列入显式状态表的码仍保留原有 400 兼容行为，`NOT_FOUND` 与 `_NOT_FOUND` 后缀仍映射 404；这不是全量内部错误码白名单，也尚未形成全仓新增错误码穷尽门禁。

HTTP Bearer 的认证方案名按 ASCII 不区分大小写；令牌保持原始大小写和字节。重复 Authorization、空令牌及额外空白字段仍被拒绝，不改变 Cookie/CSRF 或原生 SMB 凭据策略。
