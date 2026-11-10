# 类型化管理入口的源码集成

本轮在 R23 `9941f4618cec1f051652c9a7cf6c02dd14ed026f` 上集成文件合并、标签、
磁盘 mkdir／写会话、备份配置 GET／PUT，以及计划 GET／取消／提交。
三类计划准备和磁盘登记仍使用现有 dispatcher；不把本轮描述成全部迁移完成。

## 边界与保留行为

- HTTP 严格 JSON、路径 ID 绑定和字段解析负责外部形状；内部 Backend 使用具名命令、
  快照和结果。当前身份、授权撤销、磁盘状态、业务代数和正文可读性仍在真实执行点检查。
- 文件合并保留显式 `confirmed:true` 和位置摘要的 null／object 区别；标签保留静态
  标签策略、永久收据以及动态全目标预检。mkdir 保留根操作冲突与只读盘的既定顺序。
- 磁盘写会话不添加 operation ID 或永久收据；指定分支采用 Write 准入，不以 Create
  懒建根。Workspace 通过显式安全 DTO 投影，删除失去消费者的递归 JSON 脱敏。
- 备份 generation 按原规范十进制解析后参与规范摘要；`ROUTE_INCOMPLETE` 和
  `SCOPE_CHANGED` 保留 409。详见[备份边界](typed-backup-configuration.md)。
- 计划取消／提交的两阶段解析结果保留原错误优先级：当前身份优先，计划磁盘读取、
  所有者、摘要（仅提交）和历史域检查先于 operation ID 错误。提交确认属于现有 UI
  行为，服务端没有新增 `confirmed` 字段；已发布结果仍进入核心 cleanup 恢复流程。
- R23 的文件历史、活动历史、列表 typed 投影及其合法缺失／pending 语义保留。
  R22 元数据对象 I/O、标准 semaphore 和 R21 共享记录优化未改动；没有引入 B+ 候选。

## 集成修正

旧基底冲突逐项合并，保留四路撤权与 AuthReader，不复活已退役 FileManager action。
最后备份调用迁出后删除 `file_manager/operations.rs`、旧 read 包装、path/query 解析器；
计划提交迁出后删除无消费者的 `wire::operation`。旧 path 单测移到真实备份 HTTP
解析器，保留 null／数字／数组／对象／布尔值拒绝及缺省根、空根、合法路径覆盖。

Merge、Tags、mkdir、写会话、备份读写的六个 blocking worker 原本只精确捕获 backend，
未持有认证所有者。现显式持有 `state.auth`，与三个 R23 读取及三个计划入口一致。
强取消测试移除探针持有的认证 Arc，证明 Router 取消后 Weak 仍存活，worker 完成后
在有界等待内释放；不以提前的 finished 信号或任意 sleep 代替所有权证明。

## 源码组装阶段的验证记录

源码级核对：85 个 Request 变体、85 个执行分支、85 条四路撤权矩阵和独立 85 条
无凭据矩阵集合完全一致且无重复；四路组合为 340 条，另保留取消／提交各 8 个
解析错误与已撤权身份的交叉断言。12 条路由接入强认证所有权取消测试。
9 个新 Backend 方法在 22 个实现及 trait 声明中齐全；Merge／CommitPlan 另有真实
Application 方法。授权透传 fixture 的调用与期望参数各 88 项。

源码组装阶段已执行：定向 rustfmt／check、`git diff --check`、来源 refresh／verify（仅 Cargo
metadata）、消费者索引 refresh／verify、20 个第一方 owner／30 条依赖边的架构
检查、29 个无编译架构策略 fixture。来源库存保持 224 active／107 retired／3
superseded／9 profiles，未改写原始来源材料。

**源码组装阶段没有编译，没有运行 Rust 测试、Clippy、SDK、socket、负载或性能测量。**
上述矩阵与回归用例描述的是组装时的源码覆盖，不是运行通过声明；后续中央集成单独记录实际运行结果。
不创建新的 target，不推送，不变更 main。没有修改持久化、同步或物理复制预算，
不宣称吞吐改善。
