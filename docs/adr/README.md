# 架构决策（ADR）

[文档目录](../README.md) · [设计取舍汇总](../23-delivery-and-decisions.md#s23-02)

本目录解释当前架构选择、约束、代价和替代方案。行为约束以编号规范和机器契约为准，架构选择不代表性能或物理盘验收已经完成。

| 决策 | 状态 | 权威规则 |
| --- | --- | --- |
| [0001：完整版本与固定保护路由](0001-full-only.md) | 当前设计 | 第 08、09、11 章 |
| [0002：全配置成员发布与 SQLite 投影](0002-publication.md) | 当前实验实现，发行格式未冻结 | 第 10、18 章 |
| [0003：自维护 CSR 框架](0003-web-runtime.md) | 当前维护方向 | Web、runtime 与 vendor 说明 |
| [0004：分页历史清单与增量投影](0004-paged-history-manifests.md) | 分页清单实验实现；根格式由0006补充，一般增量投影待推进 | 第 10、18 章；独立历史包与元数据边界 |
| [0005：增量发布与完整性责任分离](0005-incremental-publication-integrity.md) | 方向已批准；当前源码在有效发布谱系与实际周期巡检证明下启用旧依赖免读，条件不满足仍完整读验存 | 第 06、10、12、17、18 章及[复用准入](../architecture/published-dependency-omission.md)；不表示性能或验收完成 |
| [0006：有界元数据根与路径证明](0006-bounded-metadata-roots.md) | 当前实验源码格式，发行格式未冻结；旧依赖免读按同一复用准入有条件启用，不表示格式发行或验收完成 | 第 02、03、08、10、18、19、23、25 章及核心契约 |
| [0007：同代磁盘资源所有权与 FD 相对 I/O](0007-owned-disk-resources.md) | 当前源码设计；真实 I/O 与发布屏障不变 | 第 08、10、12、17、18、20 章；Engine 生命周期 |
| [0008：工作区生命周期与永久操作事实分离](0008-workspace-lifecycle-separation.md) | 已采纳有限空 Aborted 原路径完整清单冷存；一般历史退休未实现 | 第 10、11、19 章及 SMB，永久事实与字节保留不变 |
| [0009：有界文件历史窗口与完整历史分离](0009-bounded-file-history-windows.md) | 已采纳 A，当前根窗口协议已接线；跨端验收与资源测量分别验证 | 第 06、15、19 章及当前有界文件历史 HTTP 契约；永久历史保留不变 |
| [0010：SMB 写入准入分代与有界额度授予](0010-smb-write-generations-and-extent-grants.md) | 已采纳，当前源码设计；发布阶段与保存回执不变 | 第 10、11 章及 [SMB 组件](../components/smb.md) |
| [0011：预置 Samba 账号的持久单一绑定](0011-smb-account-selection.md) | 已采纳，当前源码设计；不提升 Web 权限 | [SMB 组件](../components/smb.md)、API 与 Web 账户设置 |
| [0012：系统初始化与存储池首次激活分离](0012-management-and-storage-activation.md) | 当前源码设计；已激活池动态增删未实现 | 系统管理、首次存储激活及第 09、10 章 |
| [0013：成员配置代际、历史证明与在线交接](0013-generation-bound-membership.md) | 首次元数据纯添加已定向验证，后继批次待验；移除/API/Web未实现 | 第 08、09、10 章及已有池变更 |
| [0014：SYSTEM 持有目录与 SQLite 文件系统边界](0014-system-directory-ownership.md) | 候选设计；SYSTEM 全流程和 SQLite held-directory 接线尚未完成 | 第 08、10、20 章及显式恢复边界 |
| [0015：显式独立盘日志与可重建系统投影](0015-independent-disk-journal.md) | 离线新格式切片，在线消费者与发行格式尚未切换 | 第 08、10、18、20 章及显式逐盘权威边界 |
| [0016：目标盘局部的外来历史执行意图](0016-foreign-import-intents.md) | 首个有限实现；复制与 namespace 发布未接入 | 第 10、15 章及完整来源/目标意图契约 |
| [0017：辅助工具与离线磁盘管理职责](0017-helper-disk-management.md) | 职责已采纳，操作逐项迁移；取消用户计划下线动作 | 磁盘添加、注销/移除、外来接入、初始化及原操作恢复 |
| [递归备份空 scope 目录发布](empty-directory-backup-publication.md) | 有限实现；仅元数据成员目标，Server/Web 提供显式入口 | 第 08、09、10 章及递归备份、冷恢复边界 |
| [显式递归备份的命名空间路径跟随](recursive-namespace-follow.md) | 有限实现设计；实际验证另行绑定源码与证据 | 递归备份、原生请求闭合、永久重试及冷恢复边界 |

| [在线单文件历史 fork](online-file-history-copy.md) | 有限 Engine 入口；不表示 SMB/目录递归已接线 | 第 10、11 章及完整历史、独立副本、原操作恢复边界 |

决策文档只保留当前背景、选择、代价、替代方案与关联规则，不追加实施进度或构建日志。
