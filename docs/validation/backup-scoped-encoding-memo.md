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

原 900 秒完整事务及现有组件测量不变。这次编码接线尚未执行 GitHub 验证，
不能把 350 毫秒全部归因于 SQL，也不能预先认定解决超时。新的完整 context
摘要本身也有计算成本，收益以准确提交实际结果为准；未本地编译、测试或格式化。
