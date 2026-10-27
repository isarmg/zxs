# 最后四个管理动作的源码集成

基线为已完成中央验证的 R24 `199241735861d33b4c6e44655cc487451f22d4b9`。
本轮仅集成 historical-save／purge／split 三类准备和在线磁盘登记的类型化入口，
不包含集中 `HttpBackendJob` 改造；本轮结果仍须中央编译与运行验证。

## 边界与不变量

三类 prepare 在 HTTP 严格 JSON、大小／时间上限和路径 ID 绑定后解析一次。
未知字段的解析错误由外层 `Result` 携带，在当前身份复核之后拒绝；实际选项、
版本列表、cut 字段的解析结果通过具名命令中的内层 `Result` 延迟交付。Engine
先复核当前身份、传播外层字段错误、核对历史域，再传播内层字段错误，保持
原来缺失历史先于非法选项的错误优先级。成功路径复用既有核心准备逻辑。

prepare 不增加 operation ID 或永久收据；Plan ID、digest、pins、实际正文可读性、
预算、磁盘计划格式和后续 commit／cancel／cleanup 协议保持。移动到 contract 的
`HistoricalSaveOptions` 是已有真实纯数据类型，不引入新协议或兼容包装。

在线磁盘登记没有可成功的当前实现，使用 `Result<Infallible, ApiError>`。
HTTP 仍读取严格有界 JSON；当前身份之后返回原 409，管理存储未就绪仍返回原 503。
正文无被业务消费的字段，不发明空 DTO。详见[登记边界](typed-disk-registration.md)。

四个 worker 显式持有认证 owner 及准入 permit。强取消矩阵保留主线真实 Router、
AuthReader 和 Weak 所有权证明，不带回作者旧树的强 Arc 授权包装或 router_shared
隔离夹具。测试先观察取消时 owner、释放 worker、等 Weak 真正消失，再断言观察值；
失败不阻塞 worker 的正常释放，不靠提前完成信号或任意 sleep 充当生命周期证据。

## 最后调用者消失后的删除依据

- 四种动作迁出后，`BackendAction`、Backend／Authorized／Management／Engine 的
  generic `call`、HTTP call 与 handler 宏没有生产消费者，连同全部测试实现删除。
  不保留空枚举、测试专用 default 或回退分派。
- SMB 的 generic-call 记录器只剩断言空列表，原入口已不可调用，因此删除该字段、
  提取 helper 和空断言。实际当前／版本／仓库 typed 授权记录与错误断言保持。
- `wire::encode` 仅剩测试消费者，改为测试直接调用 `json_codec::to_value`；冻结
  wire oracle 和锁外序列化探针保留。无调用的 `wire::keys` 同时删除。
- 没有删除实际准备任务的 `execute`、SMB 测试真实 RPC `call`、独立 setup 协议、
  Plan.input、OperationResult.result 或持久化 Value。

## 源码集成阶段的验证范围

源码集合核对：89 个 Request 变体、89 个执行分支、89 条队列撤权入口和独立
89 条无凭据入口完全相等且无重复；四路组合为 356 条。授权透传调用和期望参数
各 91 项。四个新 mandatory Backend 方法各有 22 个实现及 trait 声明。
强认证所有权取消源码覆盖 16 条路由，既有 panic／容量／取消槽位覆盖保留。

已执行定向 rustfmt／check、diff 检查、来源 refresh／verify（仅 Cargo metadata）、
消费者索引 refresh／verify、20 个第一方 owner／30 条依赖边的架构检查，以及
29 个无编译架构策略 fixture。来源库存保持 224 active／107 retired／3 superseded／
9 profiles，不改写原始来源材料。

**上述源码集成阶段没有 Cargo 编译、Rust 测试、Clippy、socket、SDK、浏览器或负载执行。**
矩阵数字是源码集合核对，不是运行通过；该阶段未创建 target、推送或修改 main。
中央实际验证另行记录编译、测试和门禁结果，不以这里的静态清单代替运行证据。
R24 及之前的持久化、同步、物理复制和资源约束未改动，不宣称性能或 fsync 改善。
