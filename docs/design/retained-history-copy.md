# 保留 D/undo 审计的目标历史复制

`Engine::copy_retained_file_history` 使用独立的 `file.history.copy.retained` kind。在现有受限跨主盘文件复制中，除了全部准入正文、分支、节点、标签与备注，还把已经撤销的历史 D/undo 映射到目标文件内部数据库。**仍保留源可见，不发源 D，不提供 SMB rename 成功。** 它是完整移动的目标侧前置，不是完整移动协议。

## 同一决定中的精确闭包

新复制回执含必需的 `retained_lifecycle` 闭合封套：源 file/history、捕获 State 序号、完整路径无关文件记录的规范摘要、目标 file/history，以及完整映射审计前缀。所有 D/分支/节点引用用本次 operation 的确定映射改成目标身份，保留原作者、时间、事件 operation ID 与历史序号。导入行只表示历史事实，不创建或伪造旧 `file.delete` 收据，不加入当前 `file_deletions`，也不能作为当前可撤销的 D。

源记录和端点必须先通过有界、完整的文件数据库行检查。目标仍由普通 Prepared FULL 复制、独立正文校验、候选/决定/Finalize，以及精确 State 绑定的安全数据库安装器完成。CurrentNamespaces 在热 Prepared 及冷 Candidate 前重新生成整个源→目标转换，核准唯一原请求、路由、每个 payload 和全部 State 写集；新封套不能仅凭 kind 或自摘要取得 I/O 权限。降级恢复仍拒绝缺失该历史复制的原完整 payload 集合。

目标自己的永久复制收据保存整个扁平前缀。以后目标合法编辑或源新增 D 不改变已经导入的前缀。再次复制读取上次目标收据加本文件后来产生的直接 D/undo，再映射一次，不递归跟随源文件、读取今日源 frontier 或制造新的活动删除。每份文件数据库仍按其自身当前 D 精确消费完整保留生命周期。

旧 `file.history.copy` 的永久语义和封套保持不变。其结果缺少该审计证明时，新 retained 原语拒绝把这种旧复制作为源；不默认补空、静默升级旧收据或伪称其全部审计已经保存。

## 原子移动仍未开放

后续 `file.history.move` 必须让完整目标闭包和源全分支 frontier 的 D 属于同一个永久 operation、一个完整写集和一个可恢复决定。不能把本方法成功与另一次 `soft_delete_file` 拼接。决定前确定失败不隐藏源；可能已写 ACCEPT 后只能 InDoubt，并按原 operation 恢复。单路径移动还须拒绝多 occurrence，且未完成决定的 readiness 必须覆盖双端命名空间、预取目录页和已有读句柄。

本方法沿现有同步 Engine owner 捕获来源，沿用只绑定两端 namespace HEAD 的 copy 请求；回执中的来源摘要不是外部提交的 CAS token。若改成后台准备/稍后移动，新 move 请求与恢复计划还必须固定完整来源记录、frontier、occurrences 及配置基线，发布时重验，不能仅复用这两个 HEAD。

移动专用的内存捕获与保守重验入口见[源快照切片](file-history-move-capture.md)：它另外绑定稳定 file ID、唯一 occurrence、完整 frontier、逻辑数据库摘要、State／Config 身份及双端 namespace witness，但仍不是持久移动计划或共同决定。

后继[共同元数据投影](file-history-move-projection.md)复用本转换的类型化审计，与源全 frontier 的 D 一起保存在同一个私有写集中。该投影不生成 copy 或 delete 收据、不安装数据库、不改当前 namespace；普通 copy 的 source-retained 行为与结果不变。

本封套的源摘要是从已捕获 State 构造的**逻辑数据库投影**，不是源物理 SQLite、目标所有备份、副本路线或系统/盘根三范围数据库已经齐备的证明。已有 placement/protection/file-backup 约束、缺正文、异路由和继承来源拒绝均不放宽。系统/盘根作用域及完整迁移证明缺口存在时，不发布源 D、不对 SMB 返回完整跨盘移动成功。

当前 `HistoryManifest` 不导出永久 operations/operation_principals。新目标收据可在**本池完整 State 冷恢复**中重建审计，不表示离线 history_package 已携带这些审计、SQLite 或独立恢复闭包；不能把历史包准备成功当作本功能的完整可携带性证明。普通文件历史图/UI 也没有因内部数据库多出审计行便获得历史 D 展示入口。

## 资源和验证

保持每文件 65,536 条记录、12 MiB 编码记录和 16 MiB SQLite 上限；导入审计封套另限 12 MiB。文件与目录共享每 history 的安装保留额度。新复制需要构造一次源所有者索引和完整有界文件投影，成本随 State 索引及该文件保留图增长，不宣称常数成本或全量迁移性能。正常文件写入仍用派生的直接收据归属索引。

验收应分别列出：实际旧 D/undo 映射和目标 SQL、copy-of-copy、目标独立编辑、源后续删除隔离、缺失库冷重建、同 ID 重试、决定前/后故障、封套删减/错归属/漏字段、热冷索引，以及未运行的 SMB 和独立盘恢复。文档不是这些测试已经通过的声明。
