# 超预算文件 D 的分页审阅与摘要恢复

## 决策

单文件 D 超出完整响应预算时，Web 使用准确文件／D 身份读取摘要以及分支、父边分页，再明确确认恢复。摘要和分页只投影已准入的不可变 D，不建立新的凭据、服务端游标缓存或持久记录。原始 D、全部 frontier、原删除和撤销永久收据保持完整。

恢复仍执行原 `file.delete.undo`，请求正文仍只有 `deletion_node_id`、`operation_id`，规范摘要仍绑定 `file_id`、`deletion_node_id` 与原主体。当前管理员、准确活动 D、所有者、路径及活动目录删除引用在原核心入口重新核验。目录和原批次恢复继续使用原完整成员协议。

## 有界读取

- `GET /api/files/{file}/deletion/summary?deletion_node_id={D}` 返回文件、历史域、仓库、原路径、D、原操作、作者、序号、时间及分支／去重父边数量。
- `GET /api/files/{file}/deletion/branches?deletion_node_id={D}[&after={branch}]` 按分支身份顺序返回最多 100 个 branch/head 对及显式 nullable 的 `next_after`。
- `GET /api/files/{file}/deletion/parents?deletion_node_id={D}[&offset={decimal}]` 按 D 原父边序列返回最多 100 个父节点及显式 nullable 的 `next_offset`；省略 offset 表示零。

每页都携带准确 file/D 身份，父边页同时携带本次 offset。读取在既有管理员授权和 Engine owner 锁下直接借用当前 D，分支页使用有序范围，父边页使用切片；只分配本页，复用现有有界响应 owner 和 1 MiB 响应上限。准确 D 不存在或已替换时返回现有 `FILE_NODE_NOT_FOUND`，旧页面不能换接新 D。未知／重复查询字段与非法游标沿 HTTP 边界拒绝。计数、序号、时间和父边位置均编码为十进制字符串。

## 紧凑结果与原请求恢复

`POST /api/files/{file}/deletion/undo-summary` 使用原撤销正文和核心调用，将成功结果中的完整 `removed_deletion_node` 投影为 `removed_deletion_summary`。完整永久收据在核心仍原样保存；原普通撤销端点与其完整响应保持。

`GET /api/file-deletion-undos/{operation}` 从原永久操作读取相同紧凑结果，复用同锁 `operations::authorized_record` 主体与历史域授权。响应沿用 operation ID、kind、request digest、commit state、publication sequence 等既有封套。只接 `file.delete.undo`；结果保留恢复文件 ID、摘要、时间、membership epoch 和原保护字段。未确定发布的结果继续表示未知。

Web 为摘要恢复保存小型 `UndoSummary` 原意图，仍使用现有每条／队列预算和发送前保存读回流程。完整确认同时核对原操作 ID、请求摘要、kind、准确 file/D、整个已审阅摘要和发布状态／序号；摘要与分页从不替代这些核对。响应未知或刷新后只查询原操作的紧凑结果，明确的原请求重试沿现有再次审阅入口进行。确认结果和本地记录清理完成后才解除写围栏。

## 审阅与成本

Web 继续保留小 D 的完整审阅和整批恢复入口。完整 D 读取明确超预算时才进入摘要审阅；分支和去重父边均可逐页查看，当前页及迟到回调绑定同一 file/D、会话和读取代数。恢复确认重新读取并比较摘要。分页无需下载、解码或重组完整 frontier，浏览器保留摘要与有界页面。

摘要复制固定字段；分支页为 O(log B + 100)，父边页为 O(100)。撤销及原操作读取仍承担既有完整收据的核心成本；本方案降低传输和浏览器恢复记录的大小，不改变核心发布、冷准入或永久审计成本。
