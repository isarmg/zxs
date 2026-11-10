# 专题设计

[文档目录](../README.md) · [历史与分支章节](../06-history-and-branches.md)

- [分支系统完整逻辑](branch-system-logic.md)：文件节点、目录组成、切换、移动、复制、删除、重命名与批量范围。
- [文件合并、标签与备份](file-merge-tags-backup.md)：结果选择与归属、只读别名、继承标签和用户逻辑前缀。

- [外来历史执行意图](foreign-import-writer-first-slice.md)：明确保存范围、目标盘局部身份、当前权限与耐久证据；不开放复制或 namespace 发布。

- [逐盘权威的离线存储切片](independent-disk-journal.md)：独立新格式、本盘发布和 SQLite 重建；不代表当前在线池已切换。

- [系统盘全局变更库基础](system-change-database.md)：完整保留回执与明确文件路径事件、有界读取及可重建边界。

这些文件说明当前产品行为和设计约束；可用能力与限制见[当前支持范围](../23-delivery-and-decisions.md)及对应[组件说明](../components/README.md)。

- [盘根当前范围目录副本](disk-current-catalog.md)：只从正式盘的当前精确 binding 派生版本/路径义务；不是过去全时序日志，新增实现尚未运行验证。
- [盘根本盘发布变更副本](disk-local-publication-history.md)：从完整固定 genesis 发布链逐次派生本盘版本和 owner 路径变更；代际归档及完整三范围恢复未完成，新增实现未编译/测试。

- [离线盘根数据库材料与恢复结果](offline-disk-database-material.md)：无需 SYSTEM 的受限本地清单提取、历史折叠和明确来源/权威边界；恢复后逐盘派生结果，新增代码未运行验证。
