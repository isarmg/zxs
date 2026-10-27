# 备份配置读写的类型化边界

本轮基于 R22 `7e9d0f7c150b7836e631baf6d3b62ab04d16425e`，仅迁移
`GET/PUT /api/repositories/{repository}/backup-configuration`。实际消费者是
Web `src/web/src/files/backup.rs` 和 Rust client 的 `backup_configuration` /
`configure_backups`；不增加旧接口或测试专用兼容方法。

## 数据流与边界

- GET：HTTP query → `ReadBackupConfiguration` → 当前身份／仓库准入 →
  Engine `BackupConfigurationSnapshot` → HTTP `BackupConfigurationResponse`。
  query 省略 `path` 仍表示根；原来接受的额外 query key 仍接受。
- PUT：已有严格 JSON/body-size/duplicate-key 边界和路径 ID 绑定 → 私有 `Fields`
  → `ConfigureBackups` → 当前身份／仓库准入 → 现有 scoped Engine mutation。
  未知字段拒绝；省略 path 仍为根，显式 null 拒绝。副本数仍接受 u32 范围内的
  JSON 无符号整数（含 0），是否可形成实际 route 由核心当前拓扑决定。
- `expected_generation` 只接受规范无符号十进制字符串；`00`、`+0`、数字 JSON、
  空白和溢出不准入。旧代码就是 `parse_u64_decimal` 后由 Engine
  `expected_generation.to_string()` 构造摘要；不是保留任意原始数字拼写。
  `backup.configure` 摘要继续包含 repository、path、scope、copies、prefixes 和
  **规范十进制字符串** generation。省略 path 与显式空字符串的摘要相同。
- HTTP 静态形状错误现在在调用 typed Backend 前报告；对同时具有非法字段和
  不存在仓库的请求，静态错误优先于仓库查询。有效请求的动态检查顺序没有变化。
  反序列化器的字段错误文案不保证与旧手写解析逐字一致，既有错误码／状态保留。
- Engine 仍是公开库未知调用方的边界；路径解释、当前 scope 身份、generation、
  继承规则、移出文件的独立策略、真实副本路由、显式 backup prefix、物理准备及
  故障来源均保留。类型化不证明这些随时间变化的条件，也不提前越过永久收据查询。
  `ROUTE_INCOMPLETE` / `SCOPE_CHANGED` 继续特别映射 HTTP 409。

读取投影是无 serde 的领域数据，数字在内存中保持数字。HTTP 层唯一负责 generation、
effective_generation、file_count 的十进制文本编码；policy 缺失仍编码 null，合法空
prefix/path 集合仍为空集合。Engine 不再创建读取 Value 后由上层反解析。范围内没有
新增 trait、泛型框架、后台修复、兼容路径、缓存或 fallback。

## 复杂度与持久化

读取仍扫描当前作用域、相关策略和备份路由，复杂度不作新的渐近承诺；主要变化是去掉
Engine 内的 JSON Value 中间树。拥有所有权的投影在 Engine 锁内取得，DTO 转换和
HTTP 编码在返回后进行。所有写入、同步、复制和事务顺序均未改动，不能据此宣称吞吐
提升或减少 fsync。存在默认 primary route 是内部不变量，最大副本数不再以 `1` 掩盖
不可能的空 route 集合。

## 验证范围与状态

新增源码覆盖：

- GET 缺省根／额外 query、PUT 非规范整数／范围／未知字段／null／路径 ID 冲突。
- 读取全部字段的精确 wire fixture，u64::MAX、null、空集合及 strict JSON 解析。
- `backup.configure` 固定结构摘要、根路径等价、409 路由不足、后续 generation 更新、
  关闭重启后的永久收据重放，以及新 operation 的 stale generation 拒绝。
- 保留原 scope identity、继承、物理副本、重启、跨 scope、导入／split 等测试；Engine
  读取断言改为访问 typed 字段，不保留 Value 兼容接口。
- 两个 mandatory Backend 方法接入真实 Engine、管理代理、授权装饰器及全部测试实现；
  四路撤权（managed/embedded × logout/password）、独立无凭据、参数／错误透传、
  admission、HTTP cancellation 和真实 panic mock 矩阵均加入两条入口。

候选源码阶段只执行定向 rustfmt、源码治理 refresh/verify、架构静态检查及 diff 检查。
**候选源码阶段没有执行 Cargo 编译、Rust 测试、clippy、SDK、socket 或负载测量**；上述测试是新增
源码覆盖，不是通过声明，统一编译与实际执行由集成任务负责。

候选源码阶段静态核对结果：22 个 Backend 实现全部显式实现两方法（另有 trait 声明）；
撤权和无凭据列表各 72 条且无重复，四路撤权源码组合为 288 条。定向 rustfmt
检查 39 个 Rust 文件；源码治理 refresh/verify、20 个第一方 owner / 30 条声明依赖
边的架构检查和 `git diff --check` 通过。这些数目不代表 Rust 测试已经执行。
