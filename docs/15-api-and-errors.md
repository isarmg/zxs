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

成对提供 `selected_path/selected_entry_id` 时，`selected_entry` 独立于页和搜索按精确条目身份及路径的规范等价键解析当前目录中的条目，并返回已保存的显示拼写；返回 `null` 才表示不存在，不能根据条目不在当前页推断删除。同一身份的不同路径别名仍分别匹配。`selected_path` 的末级名称使用已有名称引用校验，父路径须与本次目录的键一致；显示路径可能额外包含仓库名称前缀，不能把这个前缀计入仓库内存储路径的 200 个 UTF-16 代码单元预算而误拒合法条目。每次最多增加一项投影。目录分支只返回摘要，不携带 `entries/file_contexts/file_branches/inherited_files/directory_refs`；分支数量本身尚未分页。

批量准备通过只读 `POST /api/repositories/{repo}/directory-branches` 提交 `{path,sources:[{directory_branch_id,expected_tree_id}],entries:[{entry_id,path}]}`。服务器核对捕获的作用域、子树头和直接子项，返回摘要及 `{entry_id,path,selected_path}`；`selected_path` 关联原选择，避免别名子集被误认为唯一身份。源集合为空是权威结果，不得用同路径旧作用域补齐。每个数组最多 500 项、乘积最多 10000，超限明确拒绝；目标选择用 `GET .../directory-branches?path=...&summary=true`。这不是写入授权，最终变更仍重新核对全部身份与树基线，Cookie POST 仍要求 CSRF。 同身份的 NFC / NFD 等价选择视为重复输入；别名匹配按路径键区分，返回的 `selected_path` 保留请求拼写用于关联，`path` 保留已存储拼写。

| 接口 | 用途 |
| --- | --- |
| `GET /api/setup/status` | 公开最小 state，不返回目录或授权材料 |
| `GET /api/setup/recovery-status` | 仅灾备 bootstrap 授权状态与 CSRF |
| `POST /api/setup/initialize` | 无密钥一次性创建管理员，保留同源／Fetch Metadata／JSON／限流；请求 username/password，返回 initialized=true、storage_pending=true |
| `GET /api/admin/storage/status` | 当前管理员查询 pending/active/recovery_required/unavailable、disk_ids、completion_available、allow_development |
| `GET /api/admin/storage/discovery-root` | 只读返回 systemd `--storage-discovery-root`，默认 /zxs/data/disks；不提供 HTTP 修改 |
| `GET /api/admin/storage/discovered-disks` | 无路径参数，固定父目录的直接候选及空目录/已识别ZXS/未支持/不可用分类；仅空目录携带初始化观察，识别不等于接纳 |
| `POST /api/admin/storage/conversion/preview` | 活动系统的普通来源清点与另一空目标的一级盘添加预览；只读，不支持外来ZXS历史迁入 |
| `POST /api/admin/storage/conversion/commit` | 明确确认 contents-only 范围后执行新一级接纳和根内容导入；两个持久阶段，来源不变 |
| `GET /api/admin/storage/conversion/status` | 查询最新或指定 operation_id；只按准确原工作区永久回执报告整体完成 |
| `POST /api/admin/storage/conversion/resume` | 显式继续原 operation_id；未知导入结果保留工作区及恢复状态，不生成替代工作区 |
| `POST /api/admin/storage/conversion/stop` | 注册已完成后明确停止原内容导入；核心关闭保留工作区并发布独立停止回执，不删除来源、已接纳磁盘或暂存字节 |
| `POST /api/admin/storage/foreign/preview` | 显式只读验证发现候选的本盘发布闭包，返回来源分组、历史计数及正文暂不可用版本；`execution=unavailable`，不执行迁入或剪裁 |
| `POST /api/admin/storage/foreign/plans/preview` | 绑定完整来源复核与已登记一级盘的空父目录/逐历史名称映射，只读且不预留目标 |
| `POST /api/admin/storage/foreign/plans/save` | 明确仅保存计划后，重验并在系统盘不可覆盖保存原复核记录；不接纳或写入数据盘 |
| `GET /api/admin/storage/foreign/plans/status?operation_id=…` | 只读取当前主体的已保存计划，`validation=required`，不扫描源盘或目标盘 |
| `POST /api/admin/storage/foreign/plans/revalidate` | 重验原物理来源、全部闭包和空目标映射；匹配仍不可执行迁入 |
| `POST /api/admin/storage/foreign/plans/execute` | 明确返回 `FORMAT_UNSUPPORTED`；保存计划不能构造外盘执行能力 |
| `POST /api/admin/storage/foreign/execution-intents/preview` | 选择原复核计划及独立意图 ID，重新检查完整来源/目标；效果仅为保存目标盘意图 |
| `POST /api/admin/storage/foreign/execution-intents/save` | 原样 preview 加 `confirm_save_intent=true`，保存目标盘局部不可变意图；仅返回 `intent_saved`，不复制、预留或发布 |
| `POST /api/admin/storage/foreign/execution-intents/status` | 按完整目标 registration、原 ID 和摘要读取目标记录，不探测旧源路径；不完整残留明确要求恢复 |
| `GET /api/admin/storage/directories?path=…` | 只能列举发现父目录本身，子目录请求拒绝；返回直接空目录及已识别ZXS目录和截断计数，不递归发现 |
| `POST /api/admin/storage/directories/inspect` | 请求 mount_path/root_path，检查已存在的空私有目录并返回绑定身份的 CandidateSelection |
| `POST /api/admin/storage/activate` | 首次激活：selections 与 production/development 模式；不提供已初始化池的热增盘 |
| `POST /api/admin/storage/complete-activation` | 空对象，仅补全服务端证明可恢复的原激活意图；不重选目录或重置管理员 |
| `POST /api/auth/login` | 唯一管理员登录；请求 username/password，返回 token/csrf_token/principal/expires_at |
| `GET /api/auth/me` | 当前有效会话；返回 principal/csrf_token |
| `POST /api/auth/logout` | 持久撤销当前会话；返回 logged_out |
| `POST /api/auth/password` | 验证 current_password 并设置 new_password；返回 changed/reauthentication_required，撤销全部会话 |
| `GET /api/smb/account` | 返回 state=disabled，或 state=configured、username、generation（十进制字符串）、candidates、password_change（available/client_unavailable/unsupported_account_name/requires_unprivileged_service）；只描述配置与客户端前置条件，不承诺 smbd 正在运行 |
| `POST /api/smb/account` | 严格 username/expected_generation/administrator_password 请求；只选服务器预置候选，验证当前 Web 管理密码后持久化新绑定并关闭旧 RPC 连接；返回当前账号状态，过时代际拒绝，不创建或改名系统账户 |
| `POST /api/smb/password` | 严格 expected_generation/current_password/new_password 请求，不接受账号或目标主机；仅明确成功返回 changed=true；不撤销 Web 会话，不创建/改名 Samba 账户 |
| `GET /api/file-manager/disks` | 可见磁盘目录入口及只读状态 |
| `GET /api/file-manager/disks/{slot}/entries?path={path}` | 有界当前目录页、后代文件数与稳定目录分支摘要 |
| `POST /api/file-manager/disks/{slot}/namespace` | 准备并提交当前目录的创建、重命名或删除 |
| `POST /api/file-manager/disks/{slot}/namespace-batch` | 显式源分支→目标分支映射；移动、复制、删除、重命名整批检查并发布 |
| `GET /api/repositories/{repo}/directory-branches?path={path}` | 只读查询冻结分支和条目；省略 path 返回所有作用域，不切换默认分支 |
| `POST /api/repositories/{repo}/directory-branches` | 只读查询捕获分支及已选直接子项的精确身份/路径事实；不创建操作，不修改状态 |
| `GET/PUT /api/repositories/{repo}/default-branch` | 查询默认根目录分支；PUT 以 expected_tree_id/expected_head 显式发布根目录或子目录默认组合 |
| `PATCH /api/repositories/{repo}/directory-branches/{id}` | 修改分支标签，身份、子树和内容不变 |
| `POST /api/file-manager/disks/{slot}/write-sessions` | 取得该盘目录当前文件分支的写会话；随后通过通用工作区接口上传 |
| `GET /api/repositories/{repo}/directory-git?path={path}` | 只读准确目录仓库、所属祖先、工作树、HEAD、index 代数和直接独立子仓库状态，不隐式初始化 |
| `POST /api/repositories/{repo}/directory-git/initialize` | `{path,expected_namespace_head,operation_id}`：显式初始化此目录仓库，不递归初始化、不自动提交 |
| `POST /api/repositories/{repo}/directory-git/stage` | 准确目录、分支、工作树和 index 代数：暂存自身工作内容，不自动纳入孩子的新 HEAD |
| `POST /api/repositories/{repo}/directory-git/children/stage` | 准确父 index 代数和直接独立孩子已提交 HEAD：只暂存该引用，不替孩子提交 |
| `POST /api/repositories/{repo}/directory-branches/{id}/commits` | `expected_tree_id`、必填十进制字符串 `expected_index_generation`、`message`、`operation_id`：只提交此仓库的暂存内容和明确子仓库引用 |
| `POST /api/file-manager/disks/{slot}/directory-branches` | `{path,name,operation_id}`：登记命名目录分支，直接引用父子树；不提前创建文件分支、不复制正文、不创建占位文件、不自动选择 |
| `GET /api/file-manager/files/{file}/history` | 当前根上的统一节点窗口及有界父边预览；可成对传入 repository_id/path 区分别名，支持 limit/cursor 或精确 anchor_node_id |
| `POST /api/file-manager/files/{file}/branches` | 从选定版本登记公共文件分支；可传 source_node_id 固定路径节点来源，不复制正文或新增节点 |
| `PATCH /api/file-manager/files/{file}/nodes/{node}` | 设置路径、标签或合并节点的独立备注，最多128 UTF-8字节；空字符串清空，不改正文备注或新增节点 |
| `PATCH /api/file-manager/files/{file}/branches/{branch}` | 修改公共文件分支标签，最多128 UTF-8字节；同文件内规范化名称唯一，不增加版本或目录子树 |
| `PUT /api/file-manager/files/{file}/default-branch` | 显式设置已浏览文件分支为当前逻辑路径默认；必须提供 repository_id/path、expected_node_id/expected_head 和 operation_id |
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
| `GET /api/files/{file}/history` | 当前根上的正文版本摘要窗口，默认只读首页 |
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
| `GET /api/backup-topologies/default` | 当前池唯一拓扑的已发布代际、盘位直接来源和目录绑定；供已认证管理员查看，不接纳其它拓扑名别名 |
| `POST /api/backup-topology-plans` | 用户指定一级/二级/后续来源映射，预检路由和数据迁移 |
| `POST /api/storage-slots/{slot}/replacement-plans` | 用户为同一逻辑角色指定替换物理盘，保留来源映射 |
| `PUT /api/primary-placement-rules/{rule}` | 按规则 ID 和预期代数设置现行路径的主盘，只影响后续新版本，不搬移旧版本 |
| `GET /api/repositories/{repository}/placement-configuration?path=…` | 读取当前仓库根或目录的真实身份、主盘规则基线与可选主盘根路径；不创建规则 |
| `PUT /api/repositories/{repository}/placement-configuration` | 自动创建或更新当前路径的主盘规则；同时校验目录身份、规则 ID 和代数，保留历史正文及独立子规则 |
| `GET /api/versions/{version}/placement` | 该版本一级归属、冻结路由、每级健康和迁移残留 |
| `GET /api/admin/disks` | 磁盘状态，仅授权管理员 |
| `POST /api/admin/disks/preview` | 以 operation_id、已检查目录、槽位、层级、上游和 metadata_member 生成当前池的纯添加预览 |
| `POST /api/admin/disks` | 提交严格 `{preview}`，保留原 operation_id，真实运行在线纯添加协调器 |
| `GET /api/admin/disks/addition` | 查询最新持久添加；可传 operation_id 查询原操作，不探测主机磁盘 |
| `POST /api/admin/disks/addition/resume` | 严格 `{operation_id, continuation:add\|cancel}`，保留原目录、角色和目标配置，明确继续添加或取消 |
| `POST /api/admin/disks/addition/cancel` | 严格 `{operation_id, preview_digest}`，请求原 pure-add 的固定旧多数认证取消；已发布结果不改写 |
| `POST /api/admin/disks/exit/preview` | 对当前准确登记身份预览 `planned_offline` 或 `deregister`，绑定已发布源快照、配置和原 operation_id |
| `POST /api/admin/disks/exit` | 严格 `{preview}`，实际执行退出控制准备、持久 stop、I/O 关闭与 D3 发布 |
| `GET /api/admin/disks/exit` | 查询最新退出；可传 `operation_id`，准备阶段不等于完成 |
| `POST /api/admin/disks/exit/resume` | 严格 `{operation_id}`，按引擎真实退出阶段继续原动作，不重新访问已停止目标 |


文件管理历史返回 `kind:nodes` 的 `HistoryWindowPage`，正文历史返回 `kind:versions`；不再把版本集合、分支和合并全集夹带在主节点页中，也不从正文来源或时间相邻推断图边。统一节点按 `(sequence DESC,node_id ASC)`，正文按 `(version_sequence ASC,version_id ASC)`，分支按 ID，合并按 `(sequence DESC,merge_id ASC)`；每页替换上一个窗口。`identity.context` 保留准确当前节点／分支及实际路径，即使它们不在本页；`identity.action_max_entries` 是实际 `Config.max_entries` 的十进制文本，不替代完整动作编码的 1 MiB 上限。

两个历史入口只接受 `history_root/cursor/limit/repository_id/path/anchor_node_id` 中适用于该集合的参数；repository/path 必须成对。未知、重复参数、`all`、不支持的游标格式和跨类型 ID 明确拒绝。无 cursor 默认只读首页。节点 anchor 与 cursor 互斥，且必须提供来源页根。续页可由 cursor 携带根；显式根与 cursor 同时提供时，在当前身份和对象归属核验后检查一致性。根是目标 history 已发布且与当前 State 绑定的准确 history-manifest hash，不使用全池序号或 HistoryTree.generation；变化返回 `409 HISTORY_CHANGED`，没有跨写旧根库存。

辅助读取均为 GET，必须提供来源页 `history_root` 或对应 cursor：

| 路径（文件管理项均位于 `/api/file-manager/files/{file}/history`） | 集合 |
| --- | --- |
| `/branches`、`/merges` | 独立分支和合并摘要页；不复制 pending 映射或合并输入数组 |
| `/nodes/{node}/parents` | 同一获授权文件的合法图父关系，按父节点 ID 排序 |
| `/merges/{merge}/inputs` | 按原序号读取成对 branch/node 输入 |
| `/merges/{merge}/parents` | 按原序号读取保存的合并父关系 |
| `/nodes/{node}/fields/{field}` | `message/author/created_at` 枚举字段的 UTF-8 字节片段 |
| `/api/files/{file}/history/versions/{version}/sources` | 按原序号读取真实正文来源；与图父关系分开 |

主页父集合使用 `complete` 或带继续读取 cursor 的 `more`；即使没有分配到预览边，也不能声称父集合为空。父边显式标记 `in_window/outside_window`，后者仅表示未加载，不伪造节点。字段片段返回 total_bytes、byte_offset、text、end_of_field 及下一 cursor；尾片不表示客户端持有全部先前片段，空字段明确返回零长度和空串。未有小字段上界证明的作者、消息和创建时间不内联、不截断；不为读取增加写入长度限制。

节点页默认 32/最大 64 项，正文／分支／合并页默认 64/最大 128，关系页默认 256/最大 512，字段默认 32 KiB/最大 64 KiB，整页父边预览最多 256；cursor 最多 4096 ASCII 字符。它们限制请求工作集，不裁剪永久历史。全部窗口响应在写入缓冲前执行不超过既有 8 MiB 的硬编码上限，JSON 转义、外壳、当前摘要、cursor 与父边均计费；已选有界 DTO 只最终编码一次。共享客户端的 Content-Length、流式读取和解码上限仍保留。真实 CPU 扫描、State 驻留、发布与浏览器峰值预算按 ADR 0009 分别验证，不据此宣称 O(page) 或吞吐达标。

文件分支登记回执固定创建时的可见父 ID、`inherited:true` 与 `own_version_id:null`；目录分支回执固定目录分支身份和父子树引用，不表示预先创建文件分支。之后首写不会改写原回执。当前内容状态通过读取接口查询，不能以当前状态回填旧操作结果。

上述文件管理接口已实现，具体字段以服务端路由及机器契约为准。其后的高级接口表包含设计基线，不能推断每项均已开放。旧整树创建、历史保存与拆分接口在已包含文件分支的历史域中明确拒绝，避免把文件快照当作完整目录树。目录分支具有稳定的 directory_branch_id，其每次实际组成变化形成新的 head_tree_id；这两个身份不能与文件分支和文件版本混用。保留的受控 HTTP 正文下载接口通过 Content-Disposition 使用所选视图中的逻辑名称，Web 管理界面不提供文件上传、下载或预览入口；内部不可变 storage_name 不随下载或容器改名而改变。

普通 workspace commit 只针对明确当前分支，SMB 的目标由共享默认根目录分支及其冻结子目录组成决定；历史节点保存通过 historical-save-plans 登记新分支，仅实际 staged 的文件产生 FULL 版本，不能沿用普通提交直接覆盖主分支。普通内容提交最多一个父提交，不能由用户任填父列表；这不限制统一文件节点图的多父 `parent_node_ids`。旧 `MergeLink` 不改写提交、正文或HEAD，新文件分支合并按独立请求推进参与引用并保存来源，原正文证明不变。树拆分使用独立结构迁移接口，不能把 rebase/强制重置作为隐藏入口。

管理初始化不建立零成员池；`storage_pending:true` 只表示管理员已创建、存储尚未配置。目录选择分别携带挂载路径和实际存储根，可使用挂载点内嵌套目录；必须既存、空、0700 且属于服务账号，不执行挂载、格式化或权限修改。身份在使用时重新校验，选择 DTO 不代替授权。

首次激活／补全回执为 `{activated:true,restart_required:bool,recovering:bool}`，后两者恰一为真。客户端断开不取消已接管的有界激活任务；未知结果先查询状态，仅凭 `completion_available` 补全原意图。公开预设初始化及旧初始化补全路由已移除；恢复系统接口不变。已有池纯添加另有持久协议和 Web 控件；首次回执不是增盘完成证明。添加状态区分 `adding/recovery_required/completed`，包含原预览、当前阶段与具体失败。只有引擎确认同一 operation_id 的 `membership-add` 已发布结果并同步控制记录后才返回完成。移出和排空没有产品入口，见[在线目录成员管理](design/online-disk-management.md)。

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

外来历史执行意图使用独立的目标盘局部名称空间：地址必须同时保留完整 target registration 与原意图 ID/摘要，不是全池业务 operation 预留或发布回执。错误的查询/确认摘要返回 `PLAN_STALE`；已有不同意图内容不得覆盖。源拔除后仍可读取原目标事实；原样保存重试只同步同一记录，不宣称原 namespace 基线仍空。实际复制与 namespace 发布不可用，详见 [执行意图边界](design/foreign-import-writer-first-slice.md)。

<a id="s15-04"></a>

## 15.4 错误码基线

进入应用路由的 HTTP API 请求在响应开始前发现的错误统一返回 `{"error":{"code":"…","message":"…","retryable":false,"request_id":"req_…"}}`，HTTP 状态码不在错误对象中重复。错误封套涵盖业务拒绝、查询/路径参数解析、请求体读取、路由及后台任务失败，不以框架默认的纯文本错误替代；`request_id` 供诊断关联，不是业务 `operation_id`。它保持 `req_` 加 32 位小写十六进制，来自服务进程内的独立单调计数，仅保证该进程内唯一；不承诺跨重启／跨服务唯一，不用于授权、幂等或安全随机。错误封套构造不访问系统熵源，因而熵失败仍能返回并关联原始错误。

业务 ID 和 operation ID 仍由安全系统熵生成 UUID v4；生成失败以带原始 source 的具名错误传播，不 panic、不使用计数或可预测随机替代。HTTP 映射为 500 `INTERNAL_ERROR`，不误报为输入错误。决定前的失败不发布业务状态；如既有持久化决定后的恢复／容器改名遇到失败，遵守原 `COMMIT_IN_DOUBT` 围栏和查询恢复协议，不声称已回滚。

查询提取器拒绝的缺失字段、重复字段或非法参数值返回 400 `INVALID_REQUEST`；未知路由返回 404 `NOT_FOUND`，`/api` 与 `/api/` 命名空间不能落入同名静态文件。已知路由使用不支持的方法返回 405 `METHOD_NOT_ALLOWED` 并保留 `Allow`。这些错误不绕过原有认证与权限检查。

有界 JSON 请求体超限返回 413 `PAYLOAD_TOO_LARGE`，包括未提供或不可信 `Content-Length` 的流式输入；登录、修改密码和一般业务 JSON 的上限分别为 8 KiB、16 KiB 和 1 MiB。登录与改密额度容纳合法密码的 JSON Unicode 转义膨胀，解码后的密码规则不变。未超限但发生读取错误的请求体返回 400 `INVALID_REQUEST`。普通 JSON 请求体（包括登录和改密）须在 30 秒内读完，期限到达返回 408 `REQUEST_TIMEOUT`；初始化/恢复 JSON 另有更严格的十秒外层期限。首次账户和恢复为16 KiB，目录检查为64 KiB，首次存储激活为4 MiB，覆盖32项目录身份及路径JSON转义。文件上传继续使用独立的流式长度、空闲/总期限及暂存准入规则，不套用 JSON 请求体上限或普通请求体期限。

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
| `HISTORY_CHANGED` | 获授权目标历史的准确根或查询上下文改变 | 刷新窗口；草稿可保留，但提交前必须按当前状态重新审阅 |
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
| `POST /api/file-manager/files/{file}/merge-history` | 历史来源 `inputs:[{branch_id,source_node_id,expected_node_id}]`、结果 `chosen_node_id`、新结果分支名 `name` 与 `confirmed:true`；单次原子派生并合并，保留全部原分支和默认引用 |
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

目录游标只是无服务端缓存的翻页位置，不是授权凭据；当前没有账户绑定、签名或有效期字段，不提供防篡改令牌或 `PAGE_SNAPSHOT_EXPIRED` 语义。每页仍重新验证当前管理员及业务状态，不能依靠游标授权。搜索 `query` 最多 1024 个 UTF-8 字节，目录游标最多 32768 个 ASCII 字符；未知查询字段、重复字段、非法值和游标编码均明确拒绝。文件历史使用第 15.2 节的独立准确根窗口协议及 4096 字符游标，不复用目录摘要或提高客户端 8 MiB 上限。

当前 `GET /api/events?after={sequence}` 是已认证的事件轮询，`after` 为规范 UInt64 十进制字符串，省略时为 `"0"`。服务端按已发布 `sequence` 顺序返回其后的最多 1000 项；`next_cursor` 是最后返回的序号，没有新事件时保持输入值。超过当前发布序号的游标返回 `400 INVALID_CURSOR`。事件包含 `sequence/operation_id/kind/object_id`，没有事务内 `ordinal`、订阅连接、可配置页大小、事件窗口到期或快照续订协议。客户端可按序号去重并继续轮询；事件用于提示刷新，不能代替当前读取的认证和状态核验。

独立校验流程为：取得获授权版本描述 → 获取完整 checksums.bin → 验证清单哈希、头部、段数与长度 → 按段获取内容并核对段哈希。清单接口与内容接口执行相同的授权、PURGE_PENDING/PURGED 屏障，不由公开静态目录提供。HTTP Range 仅定义单范围基线；多范围支持须单独声明，未支持时返回明确错误。HEAD、304、206 也先授权；If-Match 强比较先于 If-None-Match 弱比较，失败为 412，命中后者为 304，两者先于 Range。304 不含响应体；GET 的 412 保持 `HTTP_PRECONDITION_FAILED` 统一 JSON 错误封套，HEAD 不含响应体。416 应返回当前表示长度。示例与测试需覆盖空文件、尾段、越界及条件失败。

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

- `principal` 仅统计当前授权身份的预算；两个 scope 的 `records` 只计未冷存的驻留记录，`cold_records` 为冷存记录数，`metadata_bytes` 包含冷热清单及生命周期预留。`records` 与 `metadata_bytes` 各包含 `used`、`limit`、`remaining`，这些值与 `cold_records` 全部为规范 UInt64 十进制字符串。
- `global` 与 `principal` 均必填且不得为 null，分别返回全池汇总和唯一管理员的主体预算。现有响应字段、每主体及全池上限均保留，不扩大容量；请求参数不能指定另一主体。
- `warning` 为 `ok`、`near_limit`（全池或主体的任一预算达到 80%）或 `exhausted`。`remaining` 下限为零。
- 当前 Engine 的 `automatic_retirement` 为 `true`，仅表示[工作区生命周期合同](19-capacity-and-performance.md)规定的受限冷存能力，不表示自动删除证据；冷存记录仍计入元数据字节预算。本接口只读，不修改预算，也不承诺下一次写入一定获准。后端不支持时返回 501。

### 引擎错误到 HTTP 的现行映射登记

服务器以共享的封闭 `ErrorCode` 对内部错误别名与公开 HTTP 状态作穷尽匹配，并测试别名唯一性、目标状态登记、重试语义和 500 诊断遮蔽。`RESOURCE_LIMIT → RESOURCE_LIMIT_EXCEEDED` 等转换是边界适配，不意味着每个内部错误码都会原样成为公开响应。既有 400 与各类 NotFound 的 404 策略由枚举分支显式保留，不再依靠未登记码默认 400 或字符串后缀推断；新增 `ErrorCode` 必须明确别名和状态策略，否则穷尽匹配不能通过编译。该约束限于此类型化映射，不代表所有错误来源及调用路径已完成审阅，详见[类型化错误边界](architecture/error-types.md)。

HTTP Bearer 的认证方案名按 ASCII 不区分大小写；令牌保持原始大小写和字节。重复 Authorization、空令牌及额外空白字段仍被拒绝，不改变 Cookie/CSRF 或原生 SMB 凭据策略。

### Web 浏览分支与默认发布

文件列表的分支切换只发起 GET 读取，不产生 operation_id、不改变默认命名空间或 SMB。目录预览使用 entries 的 `directory_branch_id` 与 `expected_tree_id` 成对查询，返回 `preview: true`、`writable: false`；游标绑定预览身份，历史读取使用冻结节点而不是当前路径选择。离开预览目录范围或切换硬盘结束预览。

右侧“设为默认分支”才发起上述显式 PUT。请求固定当前命名空间 head 及已浏览树／节点，拒绝过期视图；成功后回到默认视图。拒绝保留预览，不确定结果保留原 operation_id 并查询回执，不能生成另一请求掩盖发布状态。预览不授予重命名、删除、写入或批量修改权限。已有低层 select 接口仍是明确的持久化选择命令，不是 Web 浏览接口。

<a id="历史节点合并"></a>

### 历史节点合并

`merge-history` 的来源节点与分支 CAS head 分开：`source_node_id` 是实际参与合并的旧节点，`expected_node_id` 是用户审阅时该来源分支的当前 head。允许同一原分支的不同历史节点以及归档分支、只读合并别名；节点必须属于同一文件及来源分支的历史，重复来源节点、跨文件或跨分支历史明确拒绝。至少两个不同来源，数量仍受核心 `max_entries` 和 HTTP 1 MiB JSON 上限约束。

`name` 指定一个全新可写结果分支，不接受 `surviving_branch_id`，不静默选择现有归属。服务端在同一次 mutation 内为首个来源创建该名称的分支，其余来源创建 `history-merge-<operation_id>-<从2开始的输入序号>` 分支，随后复用文件合并逻辑。辅助名称公开用于解释来源，遵守同一命名与冲突约束；任意命名冲突或其他前置失败都不留下派生分支。原分支的 head、可写状态及路径默认引用不变；新结果需显式切换后才成为当前文件。

`chosen_node_id` 可为同文件任意健康节点。始终新建继承其正文与标签的 `fmeta_` 合并节点（`reused_node:false`），不新增正文版本、不改变任何原节点的父边。独立合并记录保留全部派生分支与来源节点，新节点连接参与来源及所选结果来源。返回既有合并结果字段，并增加 `name` 与 `source_inputs`，后者按请求顺序保存原分支、历史节点和审阅 head。

请求摘要为 `file.history.merge`、actor 与完整 `{file_id,inputs,chosen_node_id,name,location}`；`repository_id/path` 必须成对提供，省略则摘要明确绑定 `location:null`。重试必须保留原 operation_id 和全部输入；相同请求永久重放，不重复派生分支。确认、鉴权及撤权检查仍适用于重放。此接口不改变既有 `/merge` 的当前 head 合并语义，也不引入磁盘格式迁移。

<a id="目录只读历史窗口"></a>

### 目录只读历史窗口

`GET /api/repositories/{repo}/directories/{directory}/history` 读取真实 `DirectoryTree` 节点，`.../history/branches` 读取真实 `DirectoryBranch` 摘要。`directory` 是目录条目的稳定 `entry_id`，根目录为 `repository_id`；路径仅是节点保存的历史属性，不能替代身份。改名沿用身份与历史，删除仍可读取已记录历史，同路径重建是新的身份，不混入旧图。读取不会调用目录 scope 初始化、生成节点或改变默认选择；没有记录的现存目录返回真实空数组。只读备份浏览可用同一仓库/目录身份读取，不能由此获得目录修改权限。

两路返回 `DirectoryHistoryPage`：`identity:{repository_id,directory_id,history_tree_id,history_root,collection,limit}`、`current:{directory_branch_id,tree_id,committed_tree_id,staged}`、`nodes`、`branches`、`page:{previous_cursor,next_cursor}`。非目标集合为空数组；没有当前选择时 current 三个 ID 为 null，staged 为 false。节点只包含显式用户提交，字段为 `tree_id,directory_branch_id,parent_tree_id,parent_endpoint,commit_id,message,path,sequence`；父边沿不可变父链跨过内部暂存检查点，指向前一次用户提交，在页内为 `in_window`，页外为 `outside_window`，首次提交两个父字段均为 null。子目录 composition 引用不是历史父边。分支包含 `directory_branch_id,name,parent_branch_id,head_tree_id,committed_tree_id,staged,materialized,current,sequence`；工作 head 不等于目录提交，尚无用户提交时 committed_tree_id 为 null，不能把内部工作检查点画成用户历史节点。

`limit` 默认 64、最大 128，`sequence` 为十进制字符串；顺序按 sequence 降序再按 ID 升序。`cursor` 是最多 4096 字节的不透明游标，绑定仓库、稳定目录身份、集合、limit 和 exact `history_root`；调用方不可改写游标或跨集合复用。节点路支持 `anchor_tree_id` 从指定真实节点开始，不能与 cursor 同用。`history_root` 可显式提供；续页即使不传 root 也由 cursor 约束。root 变化返回 `HISTORY_CHANGED` (409)，必须重取当前窗口，不混接旧根。非法 scope/query/cursor 为 400，目录身份不存在为 `DIRECTORY_NOT_FOUND` (404)。取消沿既有 `OPERATION_ABORTED` (400)。

每页最多 128 行和 1 MiB 编码 JSON；借用共享 metadata projection，选择扫描真实目录头并在每步检查取消，额外保留 O(limit) 行，不宣称查询 CPU 为 O(limit)。服务端在查询前后重查当前鉴权和 exact root，编码与响应持有同一 admission slot。客户端与浏览器复用 `storage_contract::directory_history::DirectoryHistoryRequest` 和 `DirectoryHistoryPage::validate`，校验稳定身份、root、集合预算、顺序、父边端点与前进游标。这里不提供或宣称目录合并。

历史合并点选来源时，如普通历史摘要没有可证明的 `branch_id`，可按需请求 `GET /api/file-manager/files/{file}/merge-history/source?node_id=...&history_root=...`。返回 `FileHistorySource {file_id,source_node_id,history_root,branch:HistoryBranchSummary}`；branch.node_id 是非空、同次读取的当前 CAS head。服务端校验同文件节点，沿真实父关系寻找独立分支，或使用不可变 metadata/path 节点的明确归属；归档和只读合并别名仍能作来源。没有独立归属返回 `FILE_BRANCH_REQUIRED` (400)，必须先由用户明确从节点创建分支，不能猜当前分支。root 变化返回 `HISTORY_CHANGED` (409)。

该查询只在显式选择来源时进行，不增加普通历史页的全图遍历。先检查明确作者与所有候选的 `head == source`，只有需要祖先证明才构建一次请求内索引。所有候选 head 进入同一 BFS，visited 去重，每个唯一节点及唯一父边只扩展一次；diamond 共享祖先不按路径重复遍历，候选顺序不会让某个独立 DFS 阻断后续来源。索引只属于当前同一快照，不跨 root 缓存。

祖先索引硬上限：同文件 65,536 个节点、262,144 条不同父边；扫描所有文件的 version/path/metadata/merge header 合计不超过 1,048,576，原始 parent record 合计不超过 1,048,576（重复记录不重复增加 distinct edge，但扫描仍计费）。来源候选分支 header、查 head 与最终 branch summary 投影所需 commit entry（包括明确作者/直接 head 成功路径）共享 1,048,576 上限。read 每步检查取消，查询为 O(上述全局 header + parent record + scoped graph 的索引/遍历工作)，不宣称 O(scoped) 或常数 CPU。全局索引容量不足或搜索不完整，明确 `RESOURCE_LIMIT_EXCEEDED` (429)，不能报“无来源”。响应最多 64 KiB，读取前后重查当前鉴权与 exact root。

`merge-history` 在派生分支前，对相同 snapshot 完成 CAS、归属与有界祖先预验证；所有输入与 chosen 必须属于同一 history，混用返回 FILE_NODE_NOT_FOUND。`head == source` 与不可变节点明确作者不需要建图；其余来源按 reviewed head 分组，共用一次索引，一次 BFS 同时验证该 head 的所有来源。写请求累计搜索上限为 262,144 次唯一-per-head 节点访问与 1,048,576 次唯一-per-head 父边访问；chosen 标签继承只借用原始 first base parent，不克隆宽父数组，也共用该工作预算。前置预算失败不派生分支、不记录新回执、不发布状态。此处有界的是新增祖先搜索和标签继承，不改变旧分支命名/派生、路径核验、正文健康检查和持久发布的全局扫描或 I/O 成本，也不宣称整个合并耗时为常数。


### 目录暂存与显式提交接口

`GET /api/repositories/{repo}/directory-branches/{id}/staging` 读取持久化工作树、最近用户提交及新增／修改／删除计数。`POST /api/repositories/{repo}/directory-branches/{id}/commits` 需要 `expected_tree_id`、非空且不超过 128 UTF-8 字节的单行 `message`、永久 `operation_id`；JSON 严格拒绝未知字段。成功返回普通操作回执，重复同一操作返回原结果；树已变返回 `DIRECTORY_BRANCH_CONFLICT`，不能自动改用新基线提交。

目录提交在同一元数据事务内冻结所选目录及其精确子目录组合，不增加文件版本、不切换默认命名空间；所选目录外的祖先和兄弟仍保持原暂存状态。文件保存即已持久化，未做目录提交不代表正文只在内存。发布结果未知时按原操作 ID 查询，不另换操作号重做。

## 显式目录仓库边界

目录 Git 初始化、暂存和提交使用独立接口，不把普通文件保存当成目录提交，也不递归提交子仓库。准确目录尚未初始化时，所属祖先可以返回，但列表的 `directory_branches_supported` 仍为 false。旧 `/api/repositories/{repo}/directory-scopes` 公开入口已移除，不自动迁移为 Git 初始化。

状态 GET 必须给 `path`，根为显式空字符串，未知或重复查询字段拒绝。完整响应最多 1 MiB，超限失败而不截断；所有 nullable 字段必须出现。工作树与 index 代数在暂存/提交时同时复核，独立子仓库边界变化要求重新暂存。结果不明保留原操作 ID 查询，不以新 ID 盲重试。请求、状态与严格整数编码见机器契约，授权和实际归属仍由服务器及核心检查，详见[目录 Git HTTP 边界](components/server/src/directory-git.md)。
