# 最终组合候选验收控制器

本控制器仅用于独立验证分支，不改变产品 CI、main 或生产格式。原 `40835674738` 的 11 对完整／1 对 Incomplete 及全部原始 artifact 保持原状；新观察器验证不能补记旧追踪缺口为通过。

## 输入与执行模式

唯一产品输入为 `tools/namespace-adoption-bench/inputs.json`。baseline 固定原 e2f33d9；`final_candidate` 在收到最终 SHA/tree 与独立适配器字节身份前保持 `null`；本轮已由主任务明确冻结为 `209f31a4122174129bcf40b56c0598ac61e66352`，tree `d8c56a63ff52eb324067c0362bcd746189761c3f`。`artifacts.py resolve --mode final` 在产品 checkout、编译前失败关闭，不把旧 99 或单独恢复补丁当最终候选。

- `verification.yml` 是最终验收载体模板，发布到独立 `verify/namespace-final-acceptance-20261029` 的 `ci.yml`。两个准确输入各构建一次，十二个独立 job 覆盖三档规模的三轮无追踪 AB/BA/AB 与独立追踪 BA。每个 AB 对仍在同一 runner，双方用新逻辑池；全部原语义断言、负载与计时不变。
- `observer-validation.yml` 仅供独立 `verify/namespace-observer-filter-20261029`。它只对原 500 文件负载分别运行 plain 和 BPF 两个完整 BA 对，各自真实执行 97 目录、初始发布、20 次保存、22 结果／历史命名空间、520 份正文和 P01/target1 校验。旧 99 仅是观察器工具验证材料，此模式不能进入最终验收汇总。

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

两个 runner 各自的确定性 plain/BPF 探针都得到九项各 6 calls／3 errors、总 54／27。原始 worker errno、父／子／线程 TID、无预设 filter、BPF 新加一层 filter 与真实事件覆盖均核验通过；两份 probe 的 hash 不同，未跨 job 复用环境结论。完整产品两模式均通过双方 22 个结果、22 历史命名空间、520 份正文和 20 次保存，共 20 个真实子阶段和 4 个新逻辑池。BPF 十阶段均启用、无回退，事件数等于原始目标 syscall 总调用数加上内部 execve 事件。

实际产品计数并非逐项完全相同：90 项 calls/errors 比对中有 12 项非零，逐项差异全部保留；没有把独立新池的随机 ID／哈希路径差异藏成“完全一致”。确定性计数契约才采用精确相等判据。两种观察器的十子阶段包围耗时合计分别为 plain 777.231953374 秒、BPF 90.053714962 秒；它们包含观察器与进程启动且在不同 runner 上，仅用于观察器开销诊断，不能作无追踪产品改善、最终组合采用结论，或补记旧 3000 追踪缺口。

证据：[plain 全部原始结果](https://github.com/isarmg/zxs/actions/runs/40852567125/artifacts/12888628520)、[BPF 原始结果与实际启用证据](https://github.com/isarmg/zxs/actions/runs/40852567125/artifacts/12887424433)、[90 项计数差异与完整汇总](https://github.com/isarmg/zxs/actions/runs/40852567125/artifacts/12888329335)、[本轮构建清单与二进制](https://github.com/isarmg/zxs/actions/runs/40852567125/artifacts/12887907289)、[原始构建日志](https://github.com/isarmg/zxs/actions/runs/40852567125/artifacts/12887433235)。清单 SHA256 为 `58c5f2b86989add7b844110ffd437d037acc72dd41bb0862eb4a2dc93acbf0a8`。本轮 BPF 工具支持与完整语义验证已建立，3000 档在最终组合下是否能完成仍须正式矩阵实测。

对冻结 209 的适配器接口已静态核对：四个 NamespaceTree 适配点与 owner/result 收据路径仍适用，适配器无需修改，SHA256 仍为 `c2e05b8bc6e889e11c168197026cba334409007263186084f74fc8eec677a8c3`。这不替代该组合的实际构建和完整矩阵。
