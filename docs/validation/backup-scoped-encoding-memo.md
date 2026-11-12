# Backup 任务内纯 SQLite 编码复用

## 已知失败与边界

前序 `356fc1fe12a262e54c2d003f189f58fa274e4c63` 的 CI #1271：
31 项 Rust、117 项 Python 和 Engine 无默认特性产品检查通过；
`backup_preparation::transaction_serving::same_backup_switch_serves_and_replays_normal_successor_publications`
仍在原 900 秒上限超时。只读进度记录表明该轮经过 scoped/common/Current/alias/root
数据库阶段，约 696 秒结束 ordinary SYSTEM 阶段并进入 native metadata 安装；
日志没有栈采样，不能据此认定某一行死锁或宣称根因已解决。

静态调用展开发现一次 SYSTEM observation 可递归触发 32 次完整 serving
observation。此修改只避免相同、完整输入的纯 SQL 重复编码，不缓存 admission、
验证结论、归档重放结果或最终业务成功。完整原测试和 900 秒预算保持不变。

## 生命周期、完整输入与限制

- `PreparedBackupRegistry::capture` 默认不带缓存。只有 `runtime::run` 的栈内
  worker 显式开启，结束、失败或重试即释放；长期 inspection/recovery 对象不保留它。
- 外层键包含目标 pool/disk/slot/binding、已现场核对的 device/inode、operation、
  source fence、archive/candidate/decision/publication/authority/snapshot、
  first sequence、registry commitment 与 config hash，并有版本域。
- SYSTEM 输入摘要复用原有 64 MiB 限制内完整 State 编码；DISK 摘要流式覆盖完整
  current + history/header/全部 changes，不以最终 State 代替历史。kind 区分编码器。
- 摘要输入最多 64 MiB，超限仅放弃缓存，不改变业务编码结果；流式每个 write 及
  32 KiB 子块检查取消，保留原 OperationAborted。没有完成的摘要不被使用。
- 缓存条目和未提交预留合计最多 256 项、32 MiB 键/值/负载计费；
  BTree/分配器附加开销由固定条目数限制。Scopes 原 128 MiB 输出限额独立保持。
  满额、单项过大或竞争重复均正常回退原编码，不逐出或扩大限额。
- 新字节先在本次 capture 中待定，完整输出、manifest、最终 target binding 与
  取消检查全部通过后才提交。任何错误/取消/drop 释放预留，不缓存错误。

## 每次命中仍执行的检查

目标命名绑定、registry admission、state_at、归档的实际读取及原始历史重放、
DISK prefix/当前 rows 对照均保留。SYSTEM 字节仍完整 SQLite readback；
DISK current/history 两份字节仍实际解码并与刚生成的完整快照比较。
落盘和下游观察者原有实际读取、大小/条目预算及前后检查不变。
缓存本身没有授权、有效性或目标完成状态的含义。

## 验证范围

新增单元测试覆盖键分离、字段敏感性、待定隔离、失败释放、全局含预留限额、
禁用读者、返回字节所有权、完整历史输入及取消；编码器测试覆盖原字节相等、
错误字节拒绝、原 prefix 检查和取消。它们不能代替实际目标变更/断电恢复证明。
定向 CI 同时保留原预算与对象快照回归、产品检查和原 900 秒完整事务用例。

未本地编译、运行测试或 rustfmt。是否改善超时、实际通过项与未执行项以本次准确
GitHub 提交的逐测试结果为准；在完整事务通过前，本片不是已经验证的超时修复。

## #1277 的实际结果与夹具修正

`162211885a2634f10b6c3433ba9fdde6d2130cce` 的 #1277：8 个 Python 模块共
133 项测试通过，Engine 无默认特性产品检查通过。lib 测试编译因两处新夹具
把公开 `config()` 的 `&Config` 传给要求 `&ValidatedConfig` 的接口失败；
本次只改为测试模块可见的原已验证配置引用，不放宽生产接口或重新准入配置。
该组 Rust 单元测试尚未执行，不能把已编写的 19 项新增回归报作通过。

完整事务编译并执行，仍在 900 秒超时。该轮约 72 秒完成 scopes、132 秒 common、
218 秒 Current、390 秒 aliases、568 秒普通盘根库，SYSTEM 安装尚未报告完成。
不同轮运行环境不能据此直接证明变快或变慢；目前没有优化已解决超时的证据。
后续分项计时应定位实际计算/读取成本，保持现场验证、错误语义与原预算不变。

工件：https://github.com/isarmg/zxs/actions/runs/43183812974/artifacts/13928791603

## 仅测试特性的分项计时

现有 `fault-injection` 开发测试特性下，Scopes capture 按线程累计 registry、
state_at、archive（含历史投影）、extend、SYSTEM 输入/摘要、memo请求、真实SQL编码、
SYSTEM读回、DISK finish（含hash/memo/SQL及真实读回）、manifest/最终绑定耗时。
固定桶是包含关系，不能相加当总时间；同时记录memo请求与真实encoder调用数。
第1次以及每32次capture退出输出，硬限每线程32行，包括失败和展开退出，不当成功证据。
输出不含业务身份、路径、正文或持久材料，不用于准入/取消/完成判定。

正常产品编译完全移除该模块和计时guard，没有新增clock、TLS、环境开关或stderr输出。
原校验、错误传播、取消顺序及900秒预算不变。输出I/O错误被忽略，
输出时不持有TLS借用或memo互斥锁；原任务目录/writer租约仍按原生命周期持有。
