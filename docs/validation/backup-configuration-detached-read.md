# R132：备份配置 GET 的不可变读取

基线 `4fcb1f3cfe9fbb5f6f5de78ab85cc1e31ffae12d`。本轮仅改变
`GET /api/repositories/{repository}/backup-configuration` 的读取所有权与资源边界，
不改变 PUT 的条件写入、持久化、副本准备、完整性证明和独立完整副本规则。

## 实际消费者与权威边界

Web `src/web/src/files/backup.rs` 与 client `backup_configuration` 仍使用相同 URL、
query 和 JSON 字段。省略 path 仍读根，额外 query key 保持原接受行为。
HTTP GET 使用现有 `history_read::run`：读取槽、backend 槽、工作取消/截止时间、
8 MiB 完整响应编码上限和连接响应租约均复用既有实现。超限完整失败，不返回截断配置。
PUT 不进入该读取 runner。

EngineBackend 在锁内先检查当前身份与仓库，再捕获 `BackupConfigurationReadView`。
其 `PublishedReadView` 保留同次发布的 State、磁盘资源、namespace cache 和元数据成员；
`ValidatedConfig` 的 clone 共享同一不可变 admission。成员描述符复制和排序仍随成员数增长，
不是整个捕获 O(1)。不构造临时 Engine/PlanningContext，不深复制完整 State，不新增策略缓存。

namespace、scope、policy、placement、历史 version 和 route 工作在锁外执行。
缓存页读取前仍检查 observer，显式 namespace visitor 与 State 遍历也检查取消；
文件身份 fallback 查找仍使用现有 find_file，其页读取提供有界页之间的检查点。
最终回锁先检查当前身份，再检查 control，最后返回完整结果或原读取错误。
普通业务代数变化不否定已经捕获的观察；不增加读 token、配置重读或 generation conflict。
后续 PUT 继续在它自己的当前事务内检查 scope ID 和 expected_generation。

## 只有一份规则实现

- 原 PlanningContext disk/route 内核移到 ValidatedConfig，真实 planner 继续委托；
  观察版 route 只添加可失败检查点，匹配顺序、错误和完整链要求不变。
- placement settings 提取为具体 State/source 借用函数；planner 和查询都使用它，
  但仅新写入 placement API 继续额外验证完整 route。GET 保留不满足原份数的短链供用户修复。
- scope/effective policy/retained 文件/primary 收集/GET 投影使用一套实现。
  PUT 调用 primary 收集时仍传原 `engine.state`，不是 mutable candidate。
  无 observer 的内部 mutation/planner 路径显式传 None，不伪造测试或生产 no-op observer。

顺序保持 scope 的路径和 namespace 解析、exact/effective policy、继承文件过滤、
placement settings、future/historical primary、route、结果。只有 RouteIncomplete 可
终止可用链或产生空 requested route；其余失败保留具体错误。

file scope 仍全树搜索 alias，directory scope 仍查 subtree，root 仍全树；file_locations
优先，同稳定目录身份的其他 named branch 文件仍补入。显式文件绑定优先，retained 设置不
继承，稳定 scope 在同深度优先，独立子规则不被父规则覆盖。历史 primary 按覆盖 file IDs
扫描全局可用版本，不缩到当前 namespace 或任意 history owner。份数上限仍取相关链的最小值。

## 验证状态

新增/扩展的测试源码覆盖：真实 planner/query route 与 settings、短链修复、captured State/
config/resource 所有者、发布与 Engine 替换后的旧观察、warm cache 各检查点取消、路径/仓库/
namespace 错误顺序、真实 alias 与非活动 named directory 文件、锁外读取、managed/embedded × logout/password 的最终撤权、完整 wire、
超限和最后响应块租约。保留旧策略优先级、retained、descendant route 与 PUT 测试。

本候选只执行指定 1.99.0 rustfmt、定向 Git diff/check 与源码审阅。
**没有执行 Cargo、构建、Rust 测试、仓库脚本、生成器或负载测量。**
新增测试是待运行验证，不是通过声明；继承基线的其他待验证组件由集成流程处理。
不宣称吞吐、延迟、锁等待或持久化预算改善。
