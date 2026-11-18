# 日常管理界面与 API

Web 管理页使用 `src/web/src/admin/request.rs` 的 HTTP 请求构建器。存储变更需要有效管理员会话及 `management.storage.writable()`；服务端执行当前唯一管理员认证与存储准入。登录、注销和改密使用独立系统权威 AuthStore，不依赖数据存储可用。

## 管理入口

| 管理功能 | HTTP 端点 | Web 入口 |
|---|---|---|
| 当前管理员 | POST /api/auth/login；GET /api/auth/me；POST /api/auth/logout、/api/auth/password | 登录、只读用户名、退出和改密；无账户 CRUD、角色或准入表单 |
| 已登记根 | GET /api/repositories；GET /api/repositories/{id} | 文件管理器从已登记磁盘进入 |
| 磁盘 | GET /api/admin/disks | 查看磁盘 / 现有文件管理器磁盘导航 |
| 备份拓扑 | GET /api/backup-topologies/default | 查看拓扑 |
| 路径保护策略 | GET/PUT /api/policies/{id} | 查看当前值；策略表单仅创建／更新无稳定 scope 的现行路径规则，要求显式预期代数 |
| 文件／目录备份配置 | GET/PUT /api/repositories/{id}/backup-configuration | 文件管理器备份设置，固定 scope 身份与自身代数；保留旧绑定并回填所覆盖的可用历史版本 |
| 放置规则 | GET/PUT /api/repositories/{repository}/placement-configuration；原 GET/PUT /api/primary-placement-rules/{id} 保留 | 文件夹详情及当前目录的「主盘规则」按实际主盘根路径选择，自动携带对象身份、规则 ID 和代数；不移动旧版本。原管理表单在新入口验收前保留 |
| SMB 绑定 | GET/PUT /api/repositories/{id}/smb-binding | 绑定表单，包含分支、启用状态和预期历史代数 |
| 目录树 | GET /api/branches/{id}/tree | 查看分支树 / 现有文件管理器浏览 |
| 文件历史 | GET /api/files/{id}/history | 查看文件历史 / 现有文件详情 |
| 历史图 | GET /api/history-trees/{id}/graph | 查看历史图 / 现有历史树 |
| 创建文件分支 | POST /api/file-manager/files/{file}/branches | 文件详情：选择该文件自己的源版本，可用 source_node_id 固定来源节点 |
| 关系合并标记 | POST /api/history-trees/{id}/merge-links | 合并链接表单，包含精确的源/目标分支和提交 |
| 普通文件夹 | POST /api/file-manager/disks/{slot}/namespace | 在登记根创建文件夹，不发布目录分支；移动已有对象以其准确身份和父目录为准 |
| 历史内容清理 | POST /api/history-trees/{id}/purge-plans | 清理预览，显式版本 ID |
| 历史树拆分 | POST /api/history-trees/{id}/split-plans | 拆分预览，显式切分提交 |
| 历史保存 | POST /api/history-trees/{id}/historical-save-plans | 历史保存预览；不上传内容；可选 slot/copies |
| 审查/提交计划 | GET /api/plans/{id}; POST /api/plans/{id}/commit | 完整不可变预览 + 摘要 + 显式后果确认；仅限就绪计划 |
| 取消计划 | POST /api/plans/{id}/cancel | 独立的显式取消操作 |
| 操作结果 | GET /api/operations/{id} | 查看操作；结果未知的变更保留原操作 ID |
| 完整性检查 | GET /api/admin/check | 查看完整性检查 |


## 操作边界

- 全部会话均属于唯一管理员；不存在仓库 ACL 或账户准入列表。系统管理仍保留存储策略、绑定、计划及恢复功能。
- 策略、规则和绑定更新携带明确代数；冲突不得静默重试覆盖。
- 稳定 scope 规则与文件绑定保留设置不得通过通用策略表单修改；`BACKUP_CONFIGURATION_REQUIRED` 要求改用对应文件／目录的备份设置，不能降级为路径规则更新。
- 清理、拆分和历史保存先生成不可变预览，审查摘要及影响后明确提交；取消是独立操作。未就绪或已完成计划不能提交。
- 结果未知时保留原操作 ID，查询原操作，不改用新 ID 重发。
- 历史保存可引用已有工作区 ID，但管理页不上传、下载或预览文件正文。
- 整树高级操作受核心支持范围限制；含独立文件分支或目录组合的历史域不能使用未适配的旧整树操作。
- 首次初始化使用受本机 bootstrap 授权保护的 Web 向导。配置草稿及完整历史包由[离线辅助工具](helper.md)处理；正常服务直接运行 `zxs`，见[服务端](server.md)。

## 检查

运行 `cargo test --locked --offline -p storage-server --test http_engine`。`test/server/integration/http_engine/daily_management.rs` 直接接入 Web 同一组请求构建器与响应契约，发送具名 `Request` 的 method/path/body，并校验真实 Router/持久化 Engine 成功响应的形状与捕获身份，覆盖唯一管理员认证、已移除账户/ACL路由的拒绝、策略代数、SMB 绑定、分支、预览摘要、明确提交、取消及重试幂等。

`branch_namespace_batch_preserves_unselected_views_and_is_idempotent` 检查跨非默认分支批量操作与审计边界。独立 AuthStore 和数据存储不可用时的登录、注销、改密和恢复边界由管理运行时测试覆盖。请求／响应模块只依赖共享契约及 JSON，不依赖浏览器 API；共享 URI component 编码和协议错误由 Web adapter 转换，保留原状态、错误源及未知写入分类。HTTP 测试不能替代 WASM 编译、真实浏览器点击或视觉验收。
