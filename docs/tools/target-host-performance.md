# 目标主机优化的云环境测量入口

本目录编排隔离 Development 探针，不修改产品或持久化协议。
原始负载、正确性断言和计时范围见[性能检查](perf-check.md)及
[恢复测量](../validation/recovery-deduplication.md)。云结果明确标作
`cloud-development-proxy-experiment`，不能代表 Ultra 288V、12 块 E1.S 或真实 Samba 网络性能。
已执行范围、基线数字及候选边界见[目标主机性能基线](../validation/target-host-performance.md)。

根协调者先冻结源码，再集中构建；所有运行必须使用同一次构建的确切 target、profile、
features 与 Cargo.lock。新开一次构建前结束现有测量，不在另一个 worktree 复用来源不清的 target。

```sh
cargo build --release --locked --offline -p perf-check --bin perf-check --bin recovery-startup
python3 tools/target-host-performance/capture.py \
  --repo . --binary target/native/release/perf-check \
  --workload probe --history 8 \
  --output test/evidence/target-host-performance/probe8-r1 \
  --build-command 'cargo build --release --locked --offline -p perf-check --bin perf-check --bin recovery-startup' \
  --profile release --target x86_64-unknown-linux-gnu --features default
```

下一档只替换二进制为 `recovery-startup`、workload 为 `recovery-fixed`、history 为 64，
并使用新的 output。起步每档一次，确认实际耗时和空间后才选择 3 次重复或更大规模。
8 与 64 是实验输入，不是产品阈值。起步预算为 60 秒、采样 RSS 2 GiB、文件系统空闲预留
4 GiB；参数可按云实例实际上限调整，超限保留失败及残留材料，不自动重试或删除。
监测每 250ms 读取一次子进程 RSS 和空闲空间，有开销与采样盲区，不能当作瞬时硬隔离。
既有探针只在私有 TMPDIR 下创建自己的池，不接受已有池或真实设备参数。

每个 output 必须原先不存在。驱动复制并校验预建二进制，再执行该冻结副本，保存：

- 原始 stdout/stderr、逐次资源采样与 wait4 的退出码、CPU、峰值 RSS、缺页和块 I/O 计数；
- commit/tree、跟踪源码 diff 原文与摘要、Cargo.lock/toolchain/二进制/驱动 SHA-256；
- CPU、亲和性、cgroup CPU/内存、实际挂载、系统内存与实验目录空闲量；
- 声明的构建命令、target/profile/features、可选 `--build-receipt` 集中构建原始收据。

源码和二进制哈希是现场观察；构建来源匹配依赖根协调者的冻结构建记录，驱动不会通过
文件名推断编译特性。`provenance_stable` 只证明运行前后源码/diff和冻结二进制一致。
`process_completed` 只表示进程正常退出；必须另核对既有探针的完整输出与全部正确性断言，
不将缺输出或非零退出视为成功。wait4 块计数与 `/sys/fs/cgroup/io.stat` 不能冒充逐操作
物理盘字节，RSS 也不是分配次数。

每次使用新 fixture；OS 页缓存没有清空，首次打开也紧随写入，不能标作物理冷读。
`perf-check` 的 SQLite 触发器与完整状态比较有观测成本，按其原有说明解释。重复进程的
P50/P95/P99只在样本数足够并明确总体后报告，不能由单次四个增长检查点构造尾延迟。
本入口不提供吞吐达标阈值，不重新启用已停止的历史规模验证流程。

`tools/perf-check/src/bin/resource-phases.rs` 另通过公开 Engine API 拆分真实保存阶段：
`create_workspace`、`stage_bytes`、`prepare_workspace_commit` 捕获、`execute`、
`finish_workspace_commit`，逐事务输出 JSONL。`execute` 包含规划、正文及元数据准备，
不能仅凭该时间归因到其中一项；同步单进程也不测服务端 mutex 等待。
检查点单独计 State clone、重开和完整性检查，并核对永久收据/所有者、原请求重放不改变
State、最老正文和实时命名空间。clone-only 不含析构、比较或编码，不代表真实规划复制总成本。

集中构建时增加 `--bin resource-phases`，运行驱动选择该二进制及
`--workload phases-fixed` 或 `phases-growing`，`--history` 为 1..128 次、
`--payload-bytes` 为 64 或 128。建议先 8 次，再按预算测 64；使用新输出目录。
检查点会重开并影响随后进程状态，原始记录保留每次历史大小和重开位置，
不能把所有不同历史规模的事务混为固定规模的 P99。

`tools/perf-check/src/bin/namespace-costs.rs` 用一个全新夹具测当前命名空间的局部写入成本。
N 个 128 字节文件先在同一个 workspace 批量暂存，一次提交建立 `group` 目录；夹具耗时
和分配独立列为 `fixture_seed`。每次进程只测一个 case，推荐 N=16/128 两档：

| case | 被测暂存文件 K / 命名空间操作 | 行为 |
| --- | --- | --- |
| `empty` | 0 / 0 | 无变化提交，命名空间与正文版本保持 |
| `content-one` | 1 / 0 | 覆盖排序最后一个已有文件 |
| `content-new` | 1 / 0 | 插入不存在的新键 |
| `content-four` | 4 / 0 | 覆盖前四个文件，代表小批量 |
| `rename` | 0 / 1 | 将整个 `group` 改名为 `moved` |
| `delete` | 0 / 1 | `delete_tree` 移除整个 `group` 的实时条目，保留历史正文 |
| `rename-conflict` / `delete-conflict` | 0 / 1 | 旧 workspace 排入操作后，竞争提交覆盖最后一个文件；旧 execute 必须返回 `NAMESPACE_CONFLICT` |

`operation_setup` 记录创建/暂存/操作入队；竞争提交另列 `fixture_competing_save`，均不混入
`measured_commit` 的 capture、execute、finish。execute 仍包含实际规划、正文/元数据准备，
不能按其总时间单独推断锁等待或某个私有函数耗时。拒绝场景没有 finish，输出 `null`，
核对 canonical State、实时目录和 workspace 均不变、操作收据仍缺失。
成功收据与所有者、原请求幂等重放、重启后的 canonical State/workspace、全部实时正文、
种子历史完整正文和最终完整性检查均在被测操作外验证。

仅该探针替换全局分配器为转发到 `System` 的计数包装：成功的 alloc、alloc_zeroed 和
realloc 各计一次，requested_bytes 包含 realloc 的完整新大小；dealloc 不计。
每阶段记录进程全体线程的快照差值，计数原子操作采用 Relaxed，记录/JSON 构造在阶段外。
这是成功分配请求量，包含容器/运行时工作线程及观察器开销，不能解释为 live bytes、
峰值分配、实际驻留或产品使用了新 allocator。前后必须使用完全相同的工具和计数开销。
进程总 RSS 还包括 fixture/oracle，不能归因给单个被测阶段。

集中构建增加 `--bin namespace-costs`，直接 CLI 为 `namespace-costs 16 content-one`。
capture 使用该二进制及 `--workload namespace --namespace-files 16 --namespace-case content-one`，
不传 `--history`；每个 case/规模/重复分别指定新输出目录。先用 16 文件起步，再按实际
空间/时间预算跑 128 文件与重复，最大输入为 256 文件；输入上限只保护本云实验资源。
不把仅小批量或较大目录的收益推断为其他工作负载已经加速。

生成证据保存在 `test/evidence/` 的既有忽略规则下，保留原始输出；验收说明沿用
`docs/validation/`，源码不增加另一份项目进展报告。
