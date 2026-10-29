# 最终组合候选验收控制器

本控制器仅用于独立验证分支，不改变产品 CI、main 或生产格式。原 `40835674738` 的 11 对完整／1 对 Incomplete 及全部原始 artifact 保持原状；新观察器验证不能补记旧追踪缺口为通过。

## 输入与执行模式

唯一产品输入为 `tools/namespace-adoption-bench/inputs.json`。baseline 固定原 e2f33d9；`final_candidate` 在收到最终 SHA/tree 与独立适配器字节身份前保持 `null`；本轮已由主任务明确冻结为 `209f31a4122174129bcf40b56c0598ac61e66352`，tree `d8c56a63ff52eb324067c0362bcd746189761c3f`。`artifacts.py resolve --mode final` 对未冻结输入在产品 checkout、编译前拒绝执行，旧 99 和单独恢复补丁仅按各自指定范围使用。

- `verification.yml` 是最终验收载体模板，发布到独立 `verify/namespace-final-acceptance-20261029` 的 `ci.yml`。两个准确输入各构建一次，十二个独立 job 覆盖三档规模的三轮无追踪 AB/BA/AB 与独立追踪 BA。每个 AB 对仍在同一 runner，双方用新逻辑池；全部原语义断言、负载与计时不变。
- `observer-validation.yml` 仅供独立 `verify/namespace-observer-filter-20261029`。它只对原 500 文件负载分别运行 plain 和 BPF 两个完整 BA 对，各自真实执行 97 目录、初始发布、20 次保存、22 结果／历史命名空间、520 份正文和 P01/target1 校验。旧 99 仅是观察器工具验证材料，此模式不能进入最终验收汇总。

重复对拆为不同 runner job 以并行执行，跨对的机器差异随原始 lscpu/uname 证据保留；每个 AB 对的两版始终使用同一 runner。负载、次数和固定 AB/BA/AB 顺序未改，不剔除较慢轮次。

manifest v2 认证本次 run/attempt、执行模式、输入 JSON、源码 SHA/tree、锁和来源、精确适配器、实际 Rust/C 工具链、target/profile/features、binary SHA、ELF/动态依赖及控制器脚本字节。实际 producer 和 consumer 均保存工具版本；不缓存测试通过结论。编译、测试和测量均在 GitHub，Rust 固定 1.99.0；云工作区只做源码、Git 与静态检查。

## strace 观察器契约

固定 syscall 集为 `fsync,fdatasync,rename,renameat,renameat2,mkdir,mkdirat,unlink,unlinkat`，双方相同。plain 保留原 `strace -f -c -e trace=... -o ...`；BPF 只增 `--seccomp-bpf -d`。官方 6.8 的 `-c` synopsis 与实现支持该组合（不使用 `-ff`、attach、detach-on-exec、syscall-limit）。

官方依据：[手册](https://github.com/strace/strace/blob/v6.8/doc/strace.1.in)、[兼容性与回退](https://github.com/strace/strace/blob/v6.8/src/strace.c#L2838-L2877)、[enabled 标记](https://github.com/strace/strace/blob/v6.8/src/strace.c#L1519-L1525)、[真实 seccomp wait 事件](https://github.com/strace/strace/blob/v6.8/src/strace.c#L3177-L3213)、[实际安装失败与预检回退](https://github.com/strace/strace/blob/v6.8/src/filter_seccomp.c#L679-L724)。不是所有安装失败都会自动回退；fatal 退出保留实际失败。

每个实际追踪 runner 先运行 `observer_probe.py`，不复用另一 job 的环境结论。纯工具探针在父进程、fork 子进程和线程中对九个 Linux x86_64 syscall 各做一次成功和一次预期错误：plain 与 BPF 都必须逐项为 6 calls／3 errors，总计 54／27。它通过显式 syscall ABI 避免 libc 将 renameat2 转发到另一 syscall，并保存 worker 自身状态及真实事件 PID；它不缩减或替换产品负载。

BPF 产品测量还须认证本 job 探针的 run/attempt/SHA/job/runner、探针脚本、当前 strace binary SHA 和版本证据、固定计数与错误、父／子／线程覆盖。父 runner 必须明确无预设 seccomp filter；探针 tracee 的 seccomp 状态须与新加一层过滤器相符。每个实际产品阶段须同时有 `seccomp filter enabled`、真实 `EVENT_SECCOMP (7)`、无冲突／回退诊断和目标的严格 stdout／成功退出，不能凭 argv 或无警告就称已启用。

已有更高优先级 seccomp 动作可能让 BPF 观察器看不到相应调用，故环境已有过滤器、状态缺失、探针不通过或正证据缺失均保留 Incomplete。不会更改宿主 sysctl、内核、安全策略、权限或凭据来使其通过。strace 只对自有 tracee 设置所需过滤和 NoNewPrivs。限制依据：[当前官方手册](https://github.com/strace/strace/blob/master/doc/strace.1.in#L1881-L1889)、[Linux seccomp 动作优先级](https://www.kernel.org/doc/html/latest/userspace-api/seccomp_filter.html#return-values)。

## 结果口径与资源

`observer_compare.py` 要求确定性工具探针精确一致，并逐产品、逐阶段、逐 syscall 保存 plain/BPF 的 calls、errors 和差异。独立新池中的产品 operation/version ID 与对象哈希路径会变化，不把两次产品计数机械相等当作工具计数正确性的唯一证明；也不隐藏任何产品计数差异。观察器验证结果只用于工具准备，不是最终组合性能结果。

所有追踪计数覆盖整个子进程（包含启动／恢复），追踪耗时不混入无追踪性能数据。最终组合必须重新真实执行九个无追踪对，不能套用恢复补丁 505 单独 500 档的 72.9% 改善。

产品阶段继续为 1200 秒、8 GiB 地址空间和至少 8 GiB 空闲盘，3000 档仍需 24 GiB 启动空闲盘。最终全部 job 共享构建前设定的 150 分钟绝对截止，保留原上传余量。小档观察器验证使用更短的 60 分钟整体截止；工具探针单次 60 秒。未完成和失败均保留实际命令、原始 stdout/stderr/debug、版本／帮助／已安装手册、进程状态与全部部分 artifact；不盲目增大超时。


## 已执行的观察器工具验证

[run 40852567125](https://github.com/isarmg/zxs/actions/runs/40852567125) 于 2026-10-29 09:48:51 UTC 启动、10:09:33 UTC 完成，终态 success，汇总 `completed_observer_validation`。代码提交 `d64c8c4773b2a446bdbb58cb92ca8543e0228df6`，独立载体 `dd183b1474a9aa89b363fbdd8098eb5f587472af` 无父提交。26+3 项工具回归、两版 release 构建、输入身份认证、两种观察器完整 500 档 BA 对均通过，且已独立审阅。该运行只使用冻结旧 e2/99 小档，不是 209 的性能验收。

两个 runner 各自的确定性 plain/BPF 探针都得到九项各 6 calls／3 errors、总 54／27。原始 worker errno、父／子／线程 TID、无预设 filter、BPF 新加一层 filter 与真实事件覆盖均核验通过；两份探针结果文件的 hash 不同，脚本字节保持一致，未跨 job 复用环境结论。完整产品两模式均通过双方 22 个结果、22 历史命名空间、520 份正文和 20 次保存，共 20 个真实子阶段和 4 个新逻辑池。BPF 十阶段均启用、无回退，事件数等于原始目标 syscall 总调用数加上内部 execve 事件。

实际产品计数并非逐项完全相同：90 项 calls/errors 比对中有 12 项非零，逐项差异全部保留；没有把独立新池的随机 ID／哈希路径差异藏成“完全一致”。确定性计数契约才采用精确相等判据。两种观察器的十子阶段包围耗时合计分别为 plain 777.231953374 秒、BPF 90.053714962 秒；它们包含观察器与进程启动且在不同 runner 上，仅用于观察器开销诊断，不能作无追踪产品改善、最终组合采用结论，或补记旧 3000 追踪缺口。

证据：[plain 全部原始结果](https://github.com/isarmg/zxs/actions/runs/40852567125/artifacts/12888628520)、[BPF 原始结果与实际启用证据](https://github.com/isarmg/zxs/actions/runs/40852567125/artifacts/12887424433)、[90 项计数差异与完整汇总](https://github.com/isarmg/zxs/actions/runs/40852567125/artifacts/12888329335)、[本轮构建清单与二进制](https://github.com/isarmg/zxs/actions/runs/40852567125/artifacts/12887907289)、[原始构建日志](https://github.com/isarmg/zxs/actions/runs/40852567125/artifacts/12887433235)。清单 SHA256 为 `58c5f2b86989add7b844110ffd437d037acc72dd41bb0862eb4a2dc93acbf0a8`。本轮 BPF 工具支持与完整语义验证已建立，3000 档在最终组合下是否能完成仍须正式矩阵实测。

对冻结 209 的适配器接口已静态核对：四个 NamespaceTree 适配点与 owner/result 收据路径仍适用，适配器无需修改，SHA256 仍为 `c2e05b8bc6e889e11c168197026cba334409007263186084f74fc8eec677a8c3`。这不替代该组合的实际构建和完整矩阵。

## 冻结 209 正式 namespace 矩阵证据

[run 40856023731](https://github.com/isarmg/zxs/actions/runs/40856023731)，GitHub 终态 `success`，矩阵汇总 `completed_measurements`。启动 `2026-10-29T10:14:50Z`，终态更新 `2026-10-29T11:22:41Z`。

正式工具提交 `b8694ea08db631f422e5eb0f125933d9e2d893da`，无父载体 `d57516eabf82d244205adfcbc6db60876c066555`，产品输入配置 SHA256 `7e78b8d7a6a2c64f4ee263d1e1e14597ae442fd579219f58807b308f5963aa5b`。产品为固定 e2f33d9882cf7263412d9370b6a703e0b216357d （tree bee9d4686088a4af6c659a983c6e711f5f62d272）与冻结 209f31a4122174129bcf40b56c0598ac61e66352 （tree d8c56a63ff52eb324067c0362bcd746189761c3f）。本节不使用旧 99 或 505 单独结果作为组合验收数据。

29 项工具回归在本次 GitHub 实际执行通过；两版各构建一次。engine library/bin 实际 features 均为 `[]`，Rust/Cargo 1.99.0、release、通用 x86_64 输入、C 工具链与 ELF 动态依赖证据保存在构建清单。每个追踪 job 重做同 runner 的 54/27 工具探针，产品逐阶段核对真实 BPF enabled／event／完整参数及无回退。无追踪计时与追踪计数分列。

独立审阅逐一复核了全部 12 对、24 个新逻辑池、15 个配对比值中位数、30 个产品追踪阶段及 15 份 artifact 身份和链接，均与原始证据一致；旧 handoff 文档逐字节保持不变。

| 文件数 | 完整对照 | 失败 | Incomplete／缺失 | 状态 |
| --- | --- | --- | --- | --- |
| 500 | 4/4 | 0 | 0 | `completed_measurements` |
| 1500 | 4/4 | 0 | 0 | `completed_measurements` |
| 3000 | 4/4 | 0 | 0 | `completed_measurements` |

每个完整 load 都实际核对 22 个永久结果、owner/sequence/提交链、22 个历史命名空间与当前 HEAD、N+20 个唯一版本的真实 open_content／正文及 P01/target1 唯一 placement；公开 API 为 operation_record，未运行完整健康扫描 API。

实际子阶段状态计数为 `{"completed": 120}`，完成的验证进程共读验 40480 份池内唯一版本正文。所有 pair 的绝对 deadline 集合为 `[1793277909]`（2026-10-29 12:45:09 UTC）。最慢阶段为 3000/BA 基线 verify，实际包围耗时 1191.919881787 秒，仍在原 1200 秒上限内，余量约 8.08 秒；该慢轮完整保留。原始不完整／失败与未执行阶段均保留，不能用其他轮通过结论补记。

以下先逐对计算候选耗时÷同 runner 基线耗时，再取比值中位数；绝对值中位数另见 JSON。仅双方完整五阶段与 oracle 成功的无追踪对进入统计。build 表示初始批量发布；Cargo 编译另在构建 job 记录。

| 文件数 | 初始批量发布 | 20 次保存 | 首次查询批次 | 重复查询批次 | open |
| --- | --- | --- | --- | --- | --- |
| 500 | 0.963（3 对） | 1.133（3 对） | 25.568（3 对） | 6.302（3 对） | 0.880（3 对） |
| 1500 | 1.010（3 对） | 1.005（3 对） | 17.126（3 对） | 5.097（3 对） | 0.730（3 对） |
| 3000 | 1.023（3 对） | 0.801（3 对） | 26.378（3 对） | 4.538（3 对） | 0.888（3 对） |

独立 BPF BA 对的 syscall calls 如下；每格为基线→候选。rename 合计 rename/renameat/renameat2，mkdir 合计 mkdir/mkdirat，unlink 合计 unlink/unlinkat。九项逐 syscall calls/errors 与原始 strace 表均完整保留在汇总和原始 artifact；此表不使用追踪时间计算性能改善；各阶段计数均覆盖整个子进程，包括启动／恢复。

| 文件数 | 子进程阶段 | fsync | fdatasync | rename 合计 | mkdir 合计 | unlink 合计 |
| --- | --- | --- | --- | --- | --- | --- |
| 500 | build | 28711→29480 | 10→10 | 8125→8226 | 9744→10764 | 501→501 |
| 500 | edit | 14167→26047 | 102→102 | 1291→1321 | 6219→27067 | 20→20 |
| 500 | read | 1699→4497 | 21→21 | 21→21 | 828→2949 | 0→0 |
| 500 | open | 3→3 | 1→1 | 0→0 | 0→0 | 0→0 |
| 500 | verify | 3→3 | 1→1 | 0→0 | 0→0 | 0→0 |
| 1500 | build | 85039→86096 | 10→10 | 24189→24290 | 28871→30277 | 1501→1501 |
| 1500 | edit | 25683→46308 | 102→102 | 2311→2361 | 10967→43019 | 20→20 |
| 1500 | read | 2033→5745 | 21→21 | 21→21 | 995→3781 | 0→0 |
| 1500 | open | 3→3 | 1→1 | 0→0 | 0→0 | 0→0 |
| 1500 | verify | 3→3 | 1→1 | 0→0 | 0→0 | 0→0 |
| 3000 | build | 174305→180272 | 10→10 | 49405→49506 | 59804→68504 | 3001→3001 |
| 3000 | edit | 136541→261859 | 102→102 | 3850→3880 | 64984→247106 | 20→20 |
| 3000 | read | 25135→38843 | 22→22 | 3020→3020 | 9546→19846 | 0→0 |
| 3000 | open | 25135→38843 | 22→22 | 3020→3020 | 9546→19846 | 0→0 |
| 3000 | verify | 25137→38845 | 22→22 | 3020→3020 | 9546→19846 | 0→0 |

完整无追踪对中 open 子进程 VmHWM 的中位数如下，Linux `/proc` 单位为 KiB；不是分配次数或峰值活动堆。

| 文件数 | 对数 | 基线 KiB | 候选 KiB | 候选／基线 |
| --- | --- | --- | --- | --- |
| 500 | 3 | 59880 | 36436 | 0.608 |
| 1500 | 3 | 151748 | 86044 | 0.567 |
| 3000 | 3 | 429152 | 127612 | 0.297 |

| 文件数 | 无追踪 AB 0 | 无追踪 BA 1 | 无追踪 AB 2 | 独立 BPF BA 3 |
| --- | --- | --- | --- | --- |
| 500 | [原始对照 0](https://github.com/isarmg/zxs/actions/runs/40856023731/artifacts/12890026656) | [原始对照 1](https://github.com/isarmg/zxs/actions/runs/40856023731/artifacts/12890295905) | [原始对照 2](https://github.com/isarmg/zxs/actions/runs/40856023731/artifacts/12889941970) | [原始对照 3](https://github.com/isarmg/zxs/actions/runs/40856023731/artifacts/12889253007) |
| 1500 | [原始对照 0](https://github.com/isarmg/zxs/actions/runs/40856023731/artifacts/12890456234) | [原始对照 1](https://github.com/isarmg/zxs/actions/runs/40856023731/artifacts/12890326607) | [原始对照 2](https://github.com/isarmg/zxs/actions/runs/40856023731/artifacts/12890242532) | [原始对照 3](https://github.com/isarmg/zxs/actions/runs/40856023731/artifacts/12889993040) |
| 3000 | [原始对照 0](https://github.com/isarmg/zxs/actions/runs/40856023731/artifacts/12891449770) | [原始对照 1](https://github.com/isarmg/zxs/actions/runs/40856023731/artifacts/12892224475) | [原始对照 2](https://github.com/isarmg/zxs/actions/runs/40856023731/artifacts/12892655929) | [原始对照 3](https://github.com/isarmg/zxs/actions/runs/40856023731/artifacts/12892935593) |

构建与汇总：[构建清单／二进制／ELF／精确输入](https://github.com/isarmg/zxs/actions/runs/40856023731/artifacts/12889502807)；[原始构建日志](https://github.com/isarmg/zxs/actions/runs/40856023731/artifacts/12889811877)；[全部对照终态／逐对计时／系统调用计数](https://github.com/isarmg/zxs/actions/runs/40856023731/artifacts/12893795060)。本轮 artifact 保留 14 天。

清单 SHA256：`2eb67f3ee317fd19ed36071aac93ab1f9f817abc1ac2bc6ee8eb8c5923c02530`。
baseline binary SHA256：`5fe3d3ce0acc943b5b3553f162c9bff0d8d9be0d1771c2e52c3d413d706ab36d`。
candidate binary SHA256：`97652e3934a2841ae74e513633cc78d694a768b6576b3d9a2c49e8d76001a62b`。

计数变化也须保留：20 次保存所在子进程的 fsync，500／1500／3000 档分别为 14,167→26,047、25,683→46,308、136,541→261,859；mkdir 族调用也增加。它们包含启动／恢复，后续定位须区分这些部分，不能直接称为单次保存的调用数。持久化与完整性约束保持原样。

后续定位方向：500 档查询三轮均回退，且热重复查询的双方 rchar 均为约 121 字节（含计数读取自身开销），不能把这一回退简单归为设备读取。批次包含 1000 点查、一次目录页及事件页，应先区分各部分成本。候选 [NamespaceTree::get/get_key](https://github.com/isarmg/zxs/blob/209f31a4122174129bcf40b56c0598ac61e66352/src/engine/src/business/namespace/query.rs#L17)经 NamespaceView 重建显示路径、解析 PortablePath 并物化 owned Entry，基线适配器返回已驻留条目的借用。[NamespaceView::resolve_key_raw](https://github.com/isarmg/zxs/blob/209f31a4122174129bcf40b56c0598ac61e66352/src/engine/src/namespace.rs#L1562)及 [SegmentReader::namespace_node 缓存命中路径](https://github.com/isarmg/zxs/blob/209f31a4122174129bcf40b56c0598ac61e66352/src/engine/src/segment_store.rs#L264)中的路径／引用校验、缓存键格式化、锁和物化可供后续 profile；缓存节点自身为 Arc，共享命中不等于深拷贝整页。这些是源码调用链定位建议，不是已经测出的分配、锁耗时占比，也不是删除完整性或名称检查的授权。

本次改动仍仅限授权验证分支、控制器和证据文档；生产回退交由主任务后续处理。功能 oracle 和完整测量覆盖不等于 ADR0011 性能采用通过；读取回退及其他采用门槛仍须按实际证据判断。VmRSS 为采样时驻留内存，VmHWM 为高水位，rchar 为进程读取字节；均不冒充设备读盘、分配请求或活动堆。新逻辑池共享各自 runner 的存储，不表示独立物理设备。真实冷 OS 缓存、Backend 锁争用、namespace-cache/heaptrack、历史增长及正常产品 CI 不在本矩阵结论内。
