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
不放宽预算。该基准首次 GitHub 结果见下；本地仍未执行编译、测试或 rustfmt。

## #1289 的组件结果与下一步边界

`1558d2f3c58ddda0c75a1aa86e4b6b1b9c51284c` 的 #1289：
61 项 Rust、8 个模块共 151 项 Python 和 1 个 Engine 无默认特性产品检查通过。
新增真实组件测试通过，整体耗时 47.92 秒。唯一失败仍为原完整事务在 900 秒超时，
已选范围没有 skipped 或 blocked；组件通过不能替代完整切换与冷重放。

两轮实测如下，单位为毫秒。每个桶均为 inclusive 且是独立调用，
不可相加为总耗时，也不能用两项差值冒充同一次调用的精确内部成本。

| 真实入口 | 第 0 轮 | 第 1 轮 |
| --- | ---: | ---: |
| first-publication observation | 1079.442 | 1074.449 |
| configuration inspection | 2293.774 | 2050.905 |
| published-registry capture | 2324.044 | 2073.665 |
| published-registry observation | 233.880 | 198.077 |
| serving Plan capture | 2378.812 | 2316.194 |

四次逐目标对象数据库准备为 2.067–3.636 毫秒，镜像捕获为 3.335–5.634 毫秒，
包含真实物理 verify 的 with-image 为 8.776–13.061 毫秒。
这把本夹具的主要成本收窄到 configuration / first-publication 上游，
没有证据支持继续增加 FILE / SQLite 编码缓存。

原完整事务首次观察到 serving/scopes/common/Current/aliases/盘根库/SYSTEM
约为 26/42/74/122/216/316/510 秒，native metadata 未报告完成即超时。
进度采样不等于函数精确时长，不能跨不同运行环境直接宣称性能改进。

审查否决了简单删除 configuration 的第二次 first-candidate read：
它在 Publication/payload 现场读回之后重新读取所有目标的 inputs、authority、
archive、source、candidate 和 completion。内存候选没有这些文件的不可变租约；
尾部 decision / isolation / name 检查不能替代它们。保留全部原生产调用和错误、
取消、对象/字节预算，不以“同一调用栈”冒充仍然有效的磁盘证明。

下一轮仅扩展同一个测试夹具的七项上游外侧测量，两轮两目标合计 24 次固定调用：
first-candidate 完整读取、generation inputs、control/archive 输入、source snapshot、
authority capture、authority/candidate 输入及 first-candidate 构造。
最后一项仍包含内存语义校验、投影和编码，不能解释为纯 SQL 或纯编码成本。
不增加生产缓存、不扩大 API 可见性、不增加测试数量或放宽原预算。
本节扩展已在下述 #1292 执行，不能据此承诺优化收益或完整事务通过。

工件：https://github.com/isarmg/zxs/actions/runs/43203111948/artifacts/13938900160
组件日志为 `057.stdout.log` / `057.stderr.log`，原完整事务为 `079.stderr.log`。

## #1292 的上游结果与读回分解

`2071952d3ff0f558eadada69ea47e813814bdeab` 的 #1292：
61 项 Rust、8 个模块共 151 项 Python 和 1 个 Engine 无默认特性产品检查通过。
同一组件测试通过，耗时 67.80 秒；原完整事务仍在 900 秒超时。
已选范围 skipped / blocked 均为空，尚未完成 ordinary-serving 与后继发布冷重放。

两轮上游观察如下，单位为毫秒；逐目标项给出四次独立调用的范围。

| 入口 | 第 0 轮 / 范围 | 第 1 轮 |
| --- | ---: | ---: |
| first-candidate read_verified | 1134.242 | 1123.534 |
| generation inputs observe | 280.276 | 281.673 |
| control / archive authority 输入 | 97.816–98.201 | 同左范围 |
| source snapshot read | 3.535–3.618 | 同左范围 |
| authority capture | 14.254–14.390 | 同左范围 |
| authority / candidate 输入 | 123.236–123.941 | 同左范围 |
| authority first-candidate 构造 | 28.242–28.566 | 同左范围 |

同轮 configuration inspection 为 2800.524–2833.875 毫秒，
first-publication observation 为 1394.094–1399.681 毫秒。
这些是独立 inclusive 调用，不能相加或相减充当同一次执行的精确内部剖分。
尤其 candidate 构造包含内存语义检查与编码，但测量不支持把主要成本归于它，
也不支持再增加一个盲目的 SQL / candidate 编码缓存。

原完整事务首次观察到 serving/scopes/common/Current/aliases/盘根库/SYSTEM
约为 36/56/100/162/290/422/688 秒，native metadata 未报告完成即超时。
外部采样到约 895.19 秒，进程 user / system CPU 累计约 813.39 / 85.22 秒，
主工作线程 CPU 约 808.30 秒、调度等待约 0.178 秒；
逻辑读取约 5.85 GB、1,131 万次读调用，物理 read_bytes 为 0。
仍支持持续计算/逻辑读取，不证明具体热函数或性能修复。

静态链核对发现 first-candidate 验证包含独立的完整材料 inventory / 逐字节读比、
candidate / snapshot / State 依赖的认证和准入，以及全部依赖再次原字节读回。
后两段每次读取和重复读取都有 ReadBudget / 取消责任，不能因上一段成功而省掉。

下一片仍只扩展该测试，不改生产算法。测试 helper 按现有 read_verified 次序，
逐项观察入口/出口 isolation 和 decision、generation inputs、每目标 Plan capture、
materials inventory / bytes、State readback 和 completion；另独立观察 writer-routes。
两轮固定 28 次外侧测量，没有新测试条目、缓存、阈值、产品 hook 或 API 扩权。
原完整读取与末尾检查仍保留，原 900 秒用例与全部既有选测预算不变。
额外独立读取会预热缓存并增加测试耗时，不是冷缓存剖析；断言、日志输出和部分
析构不全部包含在计时内，各项不能相加推算原事务耗时或直接证明优化收益。
读回分项已在下述 #1294 执行；不能宣称已经解决超时或完成整池恢复。

工件：https://github.com/isarmg/zxs/actions/runs/43207338808/artifacts/13940207969
ZIP SHA256：`97866341117433c6de361699822a1628f704402723535185dbf6ce1243e8470e`。
组件日志为 `057.stdout.log` / `057.stderr.log`，原完整事务为 `079.stderr.log`，
外部观测为 `079.sampling.log`。

## #1294 的真实读回结果

`39bcae218116b2d8f925120d9a8f34cea791df5a` 的 #1294：
61 项 Rust、8 个模块共 160 项 Python 和 1 个 Engine 无默认产品检查通过，
真实组件耗时 90.13 秒通过；唯一失败仍是原完整事务 900 秒超时。
执行清单 skipped / blocked 均为空。CI 聚合正确因执行分片失败而失败，
不能把完整选测计划或组件成功视为完整事务通过。

两轮 readback 测量中，入口 decision 为 351.873–356.623 毫秒，
出口 decision 为 353.512–355.993 毫秒，writer-routes 为 352.814–354.073 毫秒。
每目标材料 inventory / 原字节比较仅 1.812–1.855 毫秒，State 真实读回为
5.472–5.782 毫秒，completion 约 0.013 毫秒。
因此，不应把主成本归于本夹具的 State 解码或材料 inventory 并增加对应缓存。

decision 门并非单个证书 JSON 解码。它调用原完整安装复验链，重新核验 native
历史/日志、FILE 库、原 SYSTEM/DISK scopes、cutover 和实体镜像，再读决定副本。
这些边界继续保留；本轮未删除首尾 decision 或任何实际读回。

工件：https://github.com/isarmg/zxs/actions/runs/43212457818/artifacts/13943581721
ZIP SHA256：`b7de6b95d584c7b606b6a28ddfce88cb1361addb2b05e92f24e933cbd9b298eb`。

## 原预备 scopes 的纯编码复用

原 `scoped/projection.rs` 与 published scopes 是两个调用路径。
前者过去在每次完整 archive 回放之后重新编码 SYSTEM 和所有 original DISK
current/history SQLite。此片只让它使用已有的纯编码接口与同一任务级 memo，
不缓存整个 PreparedScopes、State 准入、归档回放、决定验证或最终成功状态。

通用 memo 上移到 scoped 私有模块；published Context 的字段和域保持原样。
runtime 仍只创建一个 request-owned 32 MiB / 256 条总账户，包含全部 pending；
原始与 published 路径共享此配额，不新增第二缓存。
ordinary/offline reader 仍显式禁用，且禁用时不计算额外 binding 摘要。

original 键有独立域，再绑定完整 registry task/projection、完整 ScopedTask 与当前 Job。
这些输入包含 source fence、操作、原始 archive/state/configuration、目标顺序、
注册身份和物理 binding。SYSTEM 另使用完整输入摘要，DISK pair 另使用完整
current/history 摘要。不能只凭最终 State、磁盘 ID、长度或旧完成标记命中。
64 MiB 的可选 context/history 摘要上限或共享 cache 配额耗尽只退回原编码，
不扩大或替代原有输入、输出和真实读取预算。

每次 capture 仍执行目标本地 archive 原字节读取/认证、完整 published-prefix
observe、State/hash 对照、SYSTEM 输入与 SQL readback、DISK prefix/current rows
和两库 readback。65 数据库 / 64 MiB 输出限制、manifest/数据库/intent/completion
实际读回、目标命名与物理/锁检查均保留。
原始 pending 会话跨完整 selected-set 验证持有，只有 registry.verify 完成原
installation/project 核对、最终名称复核及取消 checkpoint 后提交。
失败或取消由 Drop 释放全部未提交预约。后续 decision 副本检查仍照旧执行，
这里不声称它已成功，也不把纯 SQL bytes 当作授权。

旧 12 项 memo 回归拆为 11 个通用测试与 1 个 published Context 测试，行为保留。
新增 3 项事务回归检查共同配额、跨目标取消释放与 disabled/declined fallback；
新增 5 项真实 Engine 回归覆盖 None/cold/hit 的文件、manifest/report 不变，
真实 registry 提交后 SYSTEM 命中，暖后损坏 archive/SQLite/completion 或更换根仍拒绝，
后目标失败及第二目标取消丢弃前目标 pending。
disk-pair 命中/坏缓存 readback 由原组合测试继续证明；多数坏材料回归只断言拒绝，
不宣称准确错误码或全部恢复路径均已覆盖。目录/文件快照不覆盖权限和 mtime。

原 900 秒完整事务及现有组件测量不变。这次编码接线的首次 GitHub 执行结果
见下节，尚无行为通过或收益证明。不能把 350 毫秒全部归因于 SQL；新的完整
context 摘要本身也有计算成本。未本地编译、测试或格式化。


## #1298 的失败证据与复验链分解

`9726d8dfb331489fbf488370c03631faf7fc33e5` 的 #1298：
7 个完整通过的 Python 模块共 78 项，Engine 无默认特性产品检查通过。
没有 Rust 测试通过记录：lib 单测构建因新迁移 memo 的测试接线少一层
`../` 而失败；因此原有单测、8 项新增回归和组件测试均未执行，不能把
产品检查通过等同于这些行为验证。另一个 Python 模块因既有模块名正则
不接受数字失败，已由 `f3dd9403` 修复；不能把该模块内部部分成功计入通过数。
lib 接线错误已独立以 `3c534060` 修正并提交复验，未放宽选择或预算。

原完整事务另行成功构建并实际运行，但仍在 900 秒超时。首次观察到
serving/scopes/common/Current/aliases/盘根库/SYSTEM 约为
40/58/104/168/294/424/694 秒；native metadata 未报告完成。
这不证明原预备 SQL memo 有效，也不证明无效，更不代表完整恢复完成。
外部 180 次有效观测到约 895.25 秒，进程 user/system 约 815.89/78.65 秒，
主要工作线程 CPU 约 826.85 秒、调度等待约 0.155 秒；逻辑读取约 5.85 GB、
1,152 万次读调用，物理 read_bytes 为 0。它仍只支持持续计算/逻辑读取，
不是具体函数采样，也不能跨运行环境直接宣称性能改进。

本次只扩展既有真实组件测试：在原两轮内各增加 native、对象库、原 scopes、
cutover、installation、registry projection、registry verify 的独立整体调用，
以及两个目标各一次原 image capture + verify，共 18 次固定计时调用。
报告与真实冻结 evidence、投影与原 registry projection 完整比较；image
对照真实 manifest、placement、文件/目录/body 数量后执行原目标读回。
原始 fixture、全部已有测量与末尾 admission 保留，不增加生产可见性、测试钩子、
缓存、时限或成功阈值，也不改变原 900 秒完整事务。

三个独立层级的 SQL 观察使用真实 prepared memo 和完整 task/projection binding；
不提交局部观察的 pending。只有完整生产 registry.verify 经原成功门后可提交。
这些额外真实读取会预热缓存，位置在已有真实前缀推进之后，不是冷缓存基准；
各桶包含下层、是独立调用，不能相加或直接相减当同一次执行的排它成本。
断言、日志和部分析构不全部计入。decision 的 bind/copies 私有责任未单独隔离，
不从目标读取一份新的不可信“期望值”冒充它们；完整 decision 测量仍保留。

工件：https://github.com/isarmg/zxs/actions/runs/43218824624/artifacts/13946226942
ZIP SHA256：`34c4aef0d3200dcd20acbbccdabae502f32f209c218b06a34d9862ed14f7138c`。
单测构建诊断在 `011.stdout.log`，原完整事务及外部观测在 `016.stderr.log` /
`016.sampling.log`。新增层级测量在下述 #1304 执行。


## #1304：69 项备份 unit 通过，完整事务仍超时

准确 head 为 `09b7715f3d9aa2f85820fd8dc561cc1a800936bc`，
完整比较基线为 `39bcae218116b2d8f925120d9a8f34cea791df5a`。
shard 0 的当前库存和实际逐例回执证明：本备份组 69 项 Rust 全部通过，
包括新增 3 项共享事务回归、5 项真实 original-scope 回归、原有 12 项 memo
覆盖及完整组件测试；该组 failed/skipped/blocked 均为空。
Engine 无默认特性产品检查也通过。这首次证明新的接线可执行、编码命中、
坏材料/换根仍拒绝及失败/取消丢弃 pending，不证明端到端性能问题已解决。

组件测试整体为 88.85 秒，日志 `501.stdout.log` / `501.stderr.log`。
两轮新测量如下，单位为毫秒：

| 独立整体入口 | 第 0 轮 | 第 1 轮 |
| --- | ---: | ---: |
| 原 native 完整验证 | 108.382 | 107.629 |
| 原 object 完整验证 | 131.572 | 131.247 |
| 原 scoped 完整验证 | 271.642 | 264.375 |
| 原 cutover 完整验证 | 281.062 | 277.831 |
| 原 installation 完整验证 | 311.064 | 302.215 |
| 原 registry projection | 25.101 | 24.351 |
| 原 registry 完整验证 | 333.575 | 326.478 |

原 image capture + 实际 verify 的四次逐目标调用为 13.162–13.876 毫秒。
范围指向 original scopes 内的真实回放/派生处理及其前置验证链；
不能把两个独立 inclusive 数字直接相减为同一次执行的排它成本，
也不能据此跳过任何 archive、SQL、身份、名称、决定或取消门。
此次没有同一提交的关闭/开启端到端计时对照，不能宣称 memo 带来确定加速。

独立 shard 1 原完整事务按相同 exact/max1/900 执行，仍在 900 秒超时，
其 selected skipped/blocked 为空。首次观察到 serving/scopes/common/Current/
aliases/盘根库/SYSTEM 约为 42/64/114/190/334/484/792 秒；
native metadata 未报告完成。180 次有效外部观测至 895.21 秒，进程
user/system 约 849.17/44.86 秒，主工作线程 CPU 823.70 秒、调度等待
0.148 秒；逻辑读取约 5.12 GB、1,008 万次读调用，物理 read_bytes 为 0。

整个 #1304 并未通过：shard 0 另有 31 项非本备份组失败，包含 pending 格式
夹具重复初始化替身池，以及首 registry activation 的恢复前门过早要求完整
Publication/seal 的真实恢复顺序问题；这些由各自修复片处理。
不能用 69 项通过替代 pending/readmission、三范围共同恢复、后继发布或原 900。
独立分片只改变执行调度，原完整事务和总失败聚合门均保留。

备份 unit 工件：
https://github.com/isarmg/zxs/actions/runs/43227554097/artifacts/13954370354
ZIP SHA256：`a901144c0594c7505bcc046ec3aa9f5493d2728a80072ca0698ce6fc4ffe8dff`。

原完整事务工件：
https://github.com/isarmg/zxs/actions/runs/43227554097/artifacts/13950581200
ZIP SHA256：`b5766ba61a4a65bbbe6db0b90ee45e006a7cff9358c7bf5fc184be66e6805cc5`。
事务与采样日志为 `011.stderr.log` / `011.sampling.log`。


### 下一层：真实 archive、scoped capture 与磁盘读回

在同一真实组件测试的原两轮内，再各执行一次完整 selection material 校验，
并对两个目标各观察 archive noop-reader、原 scoped capture 和 scoped store verify。
合计 14 次新增固定调用，不增加测试条目、生产钩子、可见性、缓存或预算。
完整 material/control report、State hash/sequence/snapshot 与冻结证据对照；
capture 的完整 scoped report、manifest hash、数据库个数/字节数继续对照，
store 仍实际核 inventory、manifest 和所有 SQLite 原字节。

archive noop 仅表示不额外加入派生 observer，仍执行原归档全部验证。
scoped capture 包含另一次完整 archive、派生盘观察、编码/命中与 SQL readback；
同精确 binding 的局部事务在 capture 返回时丢弃，不提交 pending。
原 18 次层级测量、原有其他标签/断言和末尾决定/名字/取消门全部保留。
新增桶仍是独立 inclusive、非冷缓存，不能相加或相减替代排它 profiling。
这 14 次观察已在下述 #1308 执行，原 900 秒也未因此放宽。


## #1308：归档、派生 capture 与实际 store 的分项结果

准确 head 为 `9af37ef4d68e2310272471a8002e8c52e3e16c11`，
比较基线为 `ac7711126764a66ae286c261ce5dbc5bb45a5233`。
shard 0 的 69 项 Rust、8 个 Python 模块共 174 项测试，以及 Engine 无默认
特性产品检查全部通过；该片 failed/skipped/blocked 均为空。
组件耗时 60.61 秒，`051.stdout.log` / `051.stderr.log` 保留实际结果。
本提交只增加测试观察，没有性能算法改动；不能把它与 #1304 的 88.85 秒
跨 runner 比较为优化收益，更不能替代原完整事务的成功证据。

新增 14 次固定调用的结果如下，单位为毫秒。target 0/1 是同一完整选择中的
两个目标；selection material 是整个目标集合的一次验证，不是单目标值。

| 独立入口 | 第 0 轮 | 第 1 轮 |
| --- | ---: | ---: |
| selection material，全目标 | 17.820 | 18.102 |
| archive noop，target 0 | 26.516 | 26.870 |
| archive noop，target 1 | 27.129 | 27.331 |
| scoped capture，target 0 | 46.817 | 45.585 |
| scoped capture，target 1 | 46.255 | 46.163 |
| scoped store verify，target 0 | 0.491 | 0.510 |
| scoped store verify，target 1 | 0.485 | 0.486 |

同轮 original native 整体约 72.4 毫秒，scoped 整体为 183.245–184.570 毫秒，
registry 整体为 227.415–232.601 毫秒。所有条目仍是各自独立的 inclusive
调用；scoped capture 包含另一遍完整 archive 与派生 observer，不是 archive
的同一次外层测量。不能相减成精确内部成本，也没有证据支持跳过或缓存
store 的真实 inventory、manifest、SQLite 原字节读取。

备份 unit 工件：
https://github.com/isarmg/zxs/actions/runs/43243579291/artifacts/13957005597
ZIP SHA256：`a794a8a67238df145bba308a73627b59b1ad3069380ab71375406d1ed3ac5399`。

独立 shard 1 的原 exact/max1/900 事务仍超时，selected skipped/blocked
为空；整个 #1308 因此失败，不是完整通过。首次见到 serving/scopes/common/
Current/aliases/盘根库/SYSTEM 约为 38/58/102/166/298/432/702 秒，
native metadata 未报告完成。180 次有效外部观测至 895.197 秒，
user/system CPU 约 818.33/77.24 秒，主工作线程 CPU 828.617 秒、
调度等待 0.146 秒；逻辑读取约 5.79 GB、1,139 万次，物理 read_bytes 为 0。
这些仍只支持持续计算/逻辑读取，不是具体热函数证明。

原完整事务工件：
https://github.com/isarmg/zxs/actions/runs/43243579291/artifacts/13958015406
ZIP SHA256：`0c52d3c7909f2bb4592dc2ac7c2c33da4006ecdd36a4c65910d59362853e9365`。

## Native archive 的局部借用解码

仅将 `archive::typed` 在当前真实读取/认证之后的
`from_value(value.clone())` 改为 `T::deserialize(&value)`，避免这一处预先深克隆
JSON Value 树。之后完整 `to_value(parsed) == value` 检查原样保留。
未缓存归档、State、准入结论或业务成功；现场读取、认证、大小/次数预算、
取消、原 journal/receipt/history/payload 验证和旧格式拒绝门均不改变。

本仓 owned 与 borrowed Value 使用同一 JSON 错误类型，但字符串 visitor
入口不同；`DeserializeOwned` 本身不证明任意自定义解析器等价。
新增差分用真实三盘、一文件的 native archive，逐个读七类实际对象，
将原 owned 算法仅作为测试参考；两边各自执行真实 Target 读取与认证，
对比完整成功编码、错误 code/source 链/JSON 分类位置及 read/byte 计费。
反例重算摘要后写入独立测试对象，使未知/缺失/null、枚举、类型化 ID/hash、
数值字符串和嵌套收据错误真正到达解码或投影拒绝，而非提前停在 hash 不符。
合法可选字段与既有裸摘要文本的解码语义保持，不把后续语义检查假装成解码规则。

差分夹具只含该 archive 接受的 Genesis Candidate，不声称 generation/registry
envelope 或 degraded Publication 的完整覆盖；原完整 archive 前后报告相等
以及取消对照继续保留。没有生产测试钩子、扩大可见性、源码计数器或新缓存。
该借用解码及新增差分已在 #1312 的准确 d1a/24a 相关比较基线通过，
连同旧格式拒绝、两项 pending 与全部 21 项 readmission：
共 330 项 Rust、176 项 Python 和 Engine 无默认特性产品检查通过，
唯一失败仍是原 900 秒事务。完整工件记录见
[外部工具能力探测](backup-profiler-capability.md)。
这不证明此处 clone 是热函数、已带来收益或解决完整事务超时。

## 安装 Task 的同值双编码合并

在 `verify_installed_with_encodings` 中，原来先 `object_hash(FORMAT, task)`，
随后 `task.bytes()` 对同一不可变 Task 再做一次规范化编码。
前者本就调用现有 `encode_hashed_object`，只是丢弃返回的 bytes；
本片直接保留该次 bytes 与同域摘要，再调用集中后的原 16 MiB 意图限额检查。
两次原编码之间没有修改、I/O 或取消点，因此只消除第二次相同纯计算。
首次域验证、规范化和哈希仍先于原限额错误，不新增错误类型或摘要解析。

其它 `Task::bytes` 调用仍只编码并使用同一个原大小守卫；
`run` 中原 bytes → hash 的调用顺序不动。
没有新 memo、State/Report 缓存、完成证明、生产测试钩子或扩大的 API。
之前的完整 cutover/scoped 验证和之后的 Image、INTENT、ACCEPTANCE、
COMPLETE 真实读回、目标名称/物理/锁检查以及最终取消门全部保留。
SYSTEM 的递归 observation 数量不因这次局部合并减少。

新增一个真实三原盘、两目标、一文件两版本的测试，推进到 TargetEntities：
对照原双编码的完整 bytes/hash，覆盖 16 MiB 前一字节、恰好及超一字节；
限额填充只作为编码样本，不声称通过 Task 准入。
坏 INTENT/ACCEPTANCE/COMPLETE 与目标替换仍实际调用验证器并断言拒绝，
取消则对照原错误码/source 链及其优先于坏材料的顺序。
拒绝前后比较整个夹具的目录结构与文件字节，不覆盖权限、mtime 等元数据。
测试只恢复它自己改坏的字节/路径，再次执行真实验证；不修复用户材料。
本片已在 #1318 的准确 `19aa38ea4866f8982997cafdea40ccabce2fb537`
及比较基线 `258d7a7b1265bcda32792d9199b10153f2cef571` 执行：
71 项 Rust、9 个 Python 模块共 205 项，以及 Engine 无默认特性产品检查通过。
新增真实编码/现场拒绝用例实际为 13.73 秒；组件观察为 79.01 秒。
installation 两轮 inclusive 为 270.691/270.328 毫秒，
registry 为 294.294/294.887 毫秒，仍不是跨 runner 收益或同次排它成本证明。
唯一失败仍是原 exact/max1/900 事务，selected skipped/blocked 均为空。
其 SYSTEM 约 572 秒报告完成后，native metadata 仍未完成。

#1318 unit 工件：
https://github.com/isarmg/zxs/actions/runs/43258570252/artifacts/13963202968
ZIP SHA256：
`a51e3656d9cc8dc3654eb8ebee10abff2aada5ee2f14645ff8735773506009c8`。
原完整事务工件：
https://github.com/isarmg/zxs/actions/runs/43258570252/artifacts/13962694792
ZIP SHA256：
`9a85d6abbcc9b14b4cc5f07189d984d84088baf7ef4f82c457aeb81499f19111`。

## #1315 原完整事务仍失败

准确 head `e73c61d2d6a6a1382763976fad5276ff54000b6d`，
比较基线 `39bcae218116b2d8f925120d9a8f34cea791df5a`。
隔离 shard 1 的六个 Python 模块共 161 项通过，
唯一失败仍是原 exact/max1/900 事务，selected skipped/blocked 为空。
该结果属于此 Task 编码合并之前，不能作为合并后的验证结果。

首次观察到 serving/scopes/common/Current/aliases/盘根库/SYSTEM
约为 40/58/104/168/294/432/718 秒，native metadata 未报告完成。
180 个有效外部样本至约 895.21 秒，进程 user/system 为 818.67/78.69 秒，
主工作线程 CPU 829.529 秒、调度等待 0.174 秒；
逻辑读取 5,671,612,107 字节、11,161,279 次，物理 read_bytes 为 0。
这仍只支持持续计算/逻辑读取，不是具体函数的热点或死锁结论。

工件：
https://github.com/isarmg/zxs/actions/runs/43253328028/artifacts/13962105785
ZIP SHA256：
`a8f8946d86154c6d4f49fdce757297060798e679b8e96a61814506294c9bf92c`。

## 原生归档 Source 的测试侧成本分解

当前未获得可用的外部函数采样能力，因此只在根测试中追加一个受限观察，
不修改生产代码、API、缓存、计数器、hook 或原 900 秒事务。
沿用现有三原盘、一目标、一文件一版本的 Genesis 归档夹具；
不等同于完整事务的两目标、两版本或 generation/registry 归档。

执行顺序为生产 `read_observed` → 测试侧复制 Source 分桶 →
生产 `read_observed` → 直接委托真实 Target 的 Source → 固定故障差分 →
恢复夹具主动改坏的字节后再次生产 `read_observed`。
各次完整 Report、State、读取预算相同，成功路径的 object/journal 调用数也对照。
生产入口持续验证 manifest 与原 immutable task 的绑定；
对象分桶及 `verify_observed` 总桶不包含预先读/解码 manifest 的时间。

复制边界仅为现有 Target::object 的
checkpoint → 地址/有界真实读取 → account → strict JSON → authenticate。
journal/account/budget 仍委托同一个真实 Target。
分别记录地址构造与读取、strict parse、认证整体和 journal 整体的 Duration，
以及实际次数和字节，最后直接写 stderr，避免成功 libtest 捕获掉诊断。
认证桶仍包括其域检查、规范化、域哈希、大小和摘要比较，
不能把该桶全部算作可省的编码时间。
整体桶还包含 typed 解码/回编码、State/历史/收据等验证及计时开销。

非法 kind/hash 地址、缺文件、坏 JSON、错误摘要、MAX_READS、
剩余字节预算耗尽和取消，均与独立真实 Target 比较错误码/source 链及最终预算。
这些差分约束测试复制漂移，不替代今后的代码审查，也不授权删除生产检查。

日志仅输出固定标签与数值，不输出内容、路径、操作或摘要。
`backup-archive-test-copy-us` 属于测试侧分解；
`backup-archive-delegated-us` 才是直接委托真实 Target 的完整调用观察。
两条路径是先后独立调用，均已预热并含观测开销，不是冷缓存或配对 A/B。
新观察尚未运行 GitHub，不能据此先认定 JSON/hash 是主耗时。

## #1320 完整比较基线中的原事务

准确 head `1c0cfb43fe656f67f3a4f5ad3b49f818b55263d0`，
base `39bcae218116b2d8f925120d9a8f34cea791df5a`。
独立 shard 3 成功构建并实际运行唯一原事务，仍在 900 秒超时；
该片无 Rust/Python 通过或产品检查，selected skipped/blocked 均为空。
不能把 #1318 的窄片通过归入这个分片，其他分片由完整汇总分别核实。

首次观察到 serving/scopes/common/Current/aliases/盘根库/SYSTEM 约为
46/72/130/212/380/548/888 秒，native metadata 未完成。
180 个有效样本至约 895.266 秒，进程 user/system 为 828.84/70.50 秒，
worker CPU 818.509 秒、调度等待 0.235 秒；
逻辑读取 4,560,328,414 字节、8,973,313 次，物理 read_bytes 为 0。
同一 Task 编码修改在不同 runner 上仍有明显时长差异，不能声称确定加速。

工件：
https://github.com/isarmg/zxs/actions/runs/43261366402/artifacts/13966215855
ZIP SHA256：
`44e23f87531a2df87e4fd0afeaead765083ab88bda54b45ba0b2499c7a60e88f`。
