# 最终组合候选验收控制器

本控制器仅用于独立验证分支，不改变产品 CI、main 或生产格式。原 `40835674738` 的 11 对完整／1 对 Incomplete 及全部原始 artifact 保持原状；新观察器验证不能补记旧追踪缺口为通过。

## 输入与执行模式

唯一产品输入为 `tools/namespace-adoption-bench/inputs.json`。baseline 固定原 e2f33d9；`final_candidate` 在收到最终 SHA/tree 与独立适配器字节身份前保持 `null`。`artifacts.py resolve --mode final` 在产品 checkout、编译前失败关闭，不把旧 99 或单独恢复补丁当最终候选。

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
