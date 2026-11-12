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

## #1280 的真实结果与外部诊断边界

`dfc0126a46a263cfaf25b37ba64c8f72d066d0aa` 的 #1280：
60 项 Rust（包含全部 19 项新增回归）、8 个模块共 124 项 Python、
Engine 无默认特性产品检查均通过，无跳过或阻塞。
唯一失败仍是原完整事务在 900 秒超时，不能宣称超时根因已经解决。

该轮约 46 秒完成 serving images、70 秒 scopes、128 秒 common、
212 秒 Current、378 秒 aliases、548 秒普通盘根库；未报告 SYSTEM 安装完成。
已取得的临时计时证据中，160 次 Scopes capture 的互不包含顶层桶合计约 13 秒。
SYSTEM memo 160 次请求实际编码 2 次，DISK memo 800 次请求实际编码 10 次；
真实 SQL 编码合计约 34 毫秒。内部桶存在包含关系，不能将所有桶直接相加。
这些数据说明本轮主要耗时在所测 Scopes 内部之外；缓存命中不等于已解决整体性能问题。

临时计时使用生产源码中的线程局部计数器，不符合《工程规范》§5.13；
`fault-injection` 特性隔离不是该规则的豁免。本次完整撤除计时模块、宏和调用，
恢复 `162211885` 的同一 Scopes 算法，保留任务内编码复用和已修正的全部测试。
保留历史选测路径，使诊断文件删除本身也继续触发同一组回归和原 900 秒完整事务。
已记录证据不删除，后续优先使用 GitHub 测试进程的外部采样；
不放宽准入、现场校验、取消、错误语义或测试预算。

工件：https://github.com/isarmg/zxs/actions/runs/43187640211/artifacts/13930981843

## 外部测试进程观测

仅对上述已被定向策略选中的原完整事务，在 Cargo 本次构建工件和 SHA256 校验后，
CI runner 直接启动同一二进制及原 `--exact` 参数一次。外部旁采样不改变产品源码，
不重复运行测试，也不替换原 900 秒单一墙钟截止时间。

观察只读取该直接子进程的 `/proc` 固定字段，每 5 秒最多一次、合计最多 180 次，
每次最多 64 线程、单个文件最多 8 KiB；记录身份复核、CPU ticks、运行/等待状态、
调度与 IO 累计值及诊断不可用/截断状态。不读进程环境、命令行、内存或业务文件。
不安装工具、不使用 sudo、不调整 perf/ptrace 安全设置；本片没有 perf 或函数栈采样。
仅记录现有 perf 工具是否存在及内核策略数值，不能据此认定实际采样权限。
这些观测可区分 CPU 工作和等待，但不能单独认定 Rust 热函数或超时根因。

诊断失败不改真实退出码，超时仍 kill/reap 直接子进程并沿用原 TimeoutExpired、
后续范围 blocked 与 descendant cleanup unproven 语义。采样日志不是测试通过证据；
仍以原真实测试输出为准。日志保存在现有 `*.log` 工件范围，未扩大 Rust 选测或构建范围；新增 26 项 Python 回归及有界旁采样。

## #1284、#1286 的真实结果

撤除源内计时的 `c53244a376416bfdc4abb4f270bc05937ef65ca8`（#1284）：
60 项 Rust、8 个模块共 125 项 Python 和 1 个 Engine 无默认特性产品检查通过。
外部采样提交 `2b8bb1eec646705785918b3a39215f783c752f48`（#1286）：
60 项 Rust、8 个模块共 151 项 Python（本采样/选测模块 28 项）及同一产品检查通过。
两轮唯一失败均为原完整事务 900 秒超时，已选范围没有 skipped 或 blocked；
未选择的全仓测试不因此算作执行。

#1286 的 180 次外部采样均通过身份复核、没有线程截断，持续到约 895.20 秒。
进程累计 user CPU 约 814.04 秒、system CPU 约 84.72 秒；
主工作线程 CPU 约 809.19 秒、调度等待约 0.191 秒。
逻辑读取累计约 5.69 GB、约 1,100 万次读调用，物理 `read_bytes` 为 0。
这支持“持续计算/逻辑读取”，不支持“整个进程长期睡死”；
不能据此定位热函数、排除忙循环，或把逻辑读取当成真实物理磁盘流量。

原集成测试每 2 秒只读观察持久阶段，#1286 首次见到 serving/scopes/common/Current/
aliases/盘根库/SYSTEM 约为 38/56/102/166/296/434/712 秒；
native metadata 尚未完成即到原期限。这不是各函数的精确执行时间。
现有 perf 工具存在、内核 `perf_event_paranoid=4`，没有修改策略或认定实际采样获准。

工件：
- #1284：https://github.com/isarmg/zxs/actions/runs/43193433620/artifacts/13932314079
- #1286：https://github.com/isarmg/zxs/actions/runs/43195614937/artifacts/13933119830

## 真实 native serving 组件基准

新增 lib 测试仅在 `test/` 调用外侧计时，生产代码只增加标准测试模块接线。
夹具保持原用例的三层源链、两个新目标、一个文件的两份版本（共 40 字节）和空
namespace 目录，不初始化普通目录 Git。通过真实 Engine 入口推进到实体准备，
持有原 source/target 锁及 SYSTEM quiescence token，执行现有原生决定、发布、
registry 与 serving 安装；不伪造“已发布 State”、manifest 或物理文件树。
进度展示写入不是业务权威，此测试不把展示状态用作准入凭据。

测量 first-publication observation、configuration inspection、published-registry
capture/observation、serving Plan capture、registry/State 读取、完整对象数据库准备/读回、
镜像捕获/真实物理验证以及包含 verify 回调的 with-image 调用。全局上游调用及每个目标
均在安装后首次观测并重复一次，
同时比较真实数据库字节、manifest 与目标报告；不是冷缓存基准，也没有时间阈值。
边界有包含关系，不能把各项相加充当总耗时。所有原准入与现场验证继续执行，
无生产计数器、测试专用替换实现、环境开关或 API 可见性扩大。

该夹具止于 published serving image 安装，未执行后续 scopes、SYSTEM、ordinary-serving
切换与恢复；不能代替真实完整切换与冷重放。原 900 秒集成仍独立选中，
不放宽预算；新基准尚未在本地或 GitHub 执行，结果以随后准确提交的 CI 工件为准。
