# 显式目录仓库 HTTP 边界

[服务端组件](../../server.md) · [API 契约](../../../15-api-and-errors.md)

每个目录只有经过显式初始化，才成为独立的目录仓库。普通工作区保存仍然耐久保存文件版本与工作组成，但不会替用户创建目录提交。读取接口不初始化、不暂存、不提交。

## 接口

- `GET /api/repositories/{repo}/directory-git?path=...` 返回准确目录的初始化状态、最近所属目录仓库、工作树、已提交 HEAD、暂存代数和直接独立子仓库状态。根目录用显式空 `path=`；查询拒绝缺失、重复和未知字段。
- `POST /api/repositories/{repo}/directory-git/initialize` 要求 `path`、准确 `expected_namespace_head` 和 `operation_id`。初始化只作用于此目录，不递归初始化孩子，也不自动创建第一次提交。
- `POST /api/repositories/{repo}/directory-git/stage` 要求目录、分支、工作树和准确 `expected_index_generation`，暂存自己拥有的工作内容与当前独立子仓库边界。
- `POST /api/repositories/{repo}/directory-git/children/stage` 暂存一个直接独立子仓库的准确已提交 HEAD；未提交孩子不能暂存。不会替孩子提交。
- `POST /api/repositories/{repo}/directory-branches/{id}/commits` 只提交此仓库已暂存的内容和子仓库引用。`expected_tree_id` 与必填 `expected_index_generation` 同时匹配才可发布；不会递归提交孩子。

所有变更保留原 `operation_id` 查询或准确重放。变更字段严格闭合，64 位整数使用规范十进制字符串；缺失代数、JSON 数值代数、未知字段不被补默认值。所有 nullable 状态字段均必须显式出现。原 `/directory-scopes` 公开路由已移除，不再提供只创建内部组成的替代初始化入口；内部原生组成辅助函数不代表 Git 仓库初始化。原磁盘 `/directory-select` 名称选择入口也已移除；选择只走仓库 `default-branch`，提交稳定分支 ID、准确工作树和命名空间 HEAD，防止同名重建目录被误选。

## 授权、预算与列表

接口先经过现有受保护路由认证，在阻塞任务真正执行时再次检查当前会话；EngineBackend 在引擎锁内复核当前管理员和仓库归属。用户提供的仓库、目录、分支或树 ID 不能替代核心对当前归属及 index/head 的校验。Cookie 仍使用既有同源与 CSRF 规则。

GET 状态在锁内捕获当前授权对应的只读元数据快照，锁外完成带预算和取消检查的状态计算，回锁后再次复核授权及快照；客户端取消贯穿阻塞计算。响应在扩容前限制为 1 MiB，并持有原后端准入直到响应字节被消费或丢弃。超限明确失败，不截断孩子列表。它不是分页接口，也不证明核心元数据状态计算具有恒定成本。

默认根分支 GET 也只列出 Git 登记中的分支，未初始化时返回空列表和空选择，不将内部工作组成伪装为用户分支。目录列表只在本次准确目录已初始化时启用目录分支操作。最近祖先已初始化不能使子目录自动获得独立仓库操作；历史预览原有准确 scope 限制继续存在。分离扫描只保留初始化布尔值，不复制目录 Git 表，也不为列表调用递归状态计算；返回前仍复核列表快照与授权。

## 验证范围

真实 HTTP 回归位于 `test/server/integration/http_engine/directory_git.rs`，覆盖读取无变更、撤销会话、严格查询和请求、准确初始化、generation CAS、原操作重放、重开后的持久状态，以及父仓库只引用显式暂存的孩子提交。Schema 与 Rust DTO 的固定正反向量另验证必需 nullable 字段和十进制整数。实际运行结果随工程交付报告，不以本页代替通过证据。
