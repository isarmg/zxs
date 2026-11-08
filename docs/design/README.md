# 专题设计

[文档目录](../README.md) · [历史与分支章节](../06-history-and-branches.md)

- [分支系统完整逻辑](branch-system-logic.md)：文件节点、目录组成、切换、移动、复制、删除、重命名与批量范围。
- [文件合并、标签与备份](file-merge-tags-backup.md)：结果选择与归属、只读别名、继承标签和用户逻辑前缀。

- [外来历史执行意图](foreign-import-writer-first-slice.md)：明确保存范围、目标盘局部身份、当前权限与耐久证据；不开放复制或 namespace 发布。

- [逐盘权威的离线存储切片](independent-disk-journal.md)：独立新格式、本盘发布和 SQLite 重建；不代表当前在线池已切换。

- [系统盘全局变更库基础](system-change-database.md)：完整保留回执与明确文件路径事件、有界读取及可重建边界。

这些文件说明当前产品行为和设计约束；可用能力与限制见[当前支持范围](../23-delivery-and-decisions.md)及对应[组件说明](../components/README.md)。
