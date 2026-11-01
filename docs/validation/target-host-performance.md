# 目标主机性能基线的测量边界

本入口保存受限云环境的可复现实验方法与证据索引，不是 Ultra 288V 主机验收或性能承诺。
工具为[集中采集器](../tools/target-host-performance.md)与公开 Engine API
[阶段探针](../../tools/perf-check/src/bin/resource-phases.rs)。原有工作负载仍见
[perf-check](../tools/perf-check.md)和[恢复测量](recovery-deduplication.md)。

## 来源与实际环境

2026-11-01 在独立目录获取公开 `https://github.com/isarmg/zxs.git` main：
`f22ae18af224ae013eb33a363f183aa811266b9e`，tree
`4aba68098e489e7bace517e41d8857ef90216827`。
工作分支为 `codex/target-host-performance-20261101`；未使用或修改环境自带的 `zxs-linux`。
初始基线中产品 `src/`、根清单、锁文件、配置和持久化/协议均未改动。
后续仅修改 `business/workspace.rs` 的局部 CPU/分配路径，见下方工作区比较；
新增测量入口与产品行为变更分别提交，配置、格式和同步协议保持。
Foundation 依赖仅通过已发布产品接口正常构建使用，不进行依赖源码审改。
随后仅 fetch 核实新的 `origin/main=d08e30707d9e6606798bfa264f894e95ca0ba143`；
没有将其合入本分支或重新测量，本文数据全部绑定上述 f22 基线与明确的工具补丁。

| 项目 | 云环境现场观察 |
| --- | --- |
| OS | Debian GNU/Linux 13.6，x86_64，KVM |
| CPU | Xeon Platinum 8370C，可见 5 CPU；`cpu.max=400000 100000`，配额为 4 CPU |
| 内存 | `/proc/meminfo` 总量 18,882,662,400 字节；cgroup `memory.max=17179869184`，上限 16GiB，无 swap |
| 临时池 | `/workspace` overlay，同一文件系统的三个复制目录；不证明物理冗余或磁盘独立带宽 |
| 空间 | 初始工作盘可用约 29.8GiB；每次记录实际空闲字节，临时池未放入 `/tmp` tmpfs |
| 缓存 | 新 fixture；无清空 OS 缓存；首次打开紧随写入，不能标作物理冷读 |
| 并发 | 单探针进程；既有 Engine 有界准备线程，原 perf-check 另含持有 FD 的读线程 |

目标主机仍为用户明确的 Ultra 288V、32GB、1TB 系统盘、12×15.6TB E1.S。
CPU 已由 [Intel 官方规格](https://www.intel.com/content/www/us/en/products/sku/240961/intel-core-ultra-9-processor-288v-12m-cache-up-to-5-10-ghz/specifications.html)
核实为 8 核 8 线程（4P+4 低功耗 E）、支持 AVX2；云 CPU 的 AVX-512 不作为目标要求。
盘型号、控制器/交换/上行拓扑、耐久、尾延迟、系统盘类型和网络速率未知。
不按盘数或核数外推本次结果，也不引入额外文件系统或物理设备准入限制。
整机内存、spool、共享链路和巡检的变量预算见[目标资源模型](../operations/ultra-288v-resource-model.md)。

## 集中构建与原始材料

工具链为根文件固定的 Rust 1.99.0（`b940084d7`），Cargo 1.99.0（`5f94df478`）。
安装位于独立工作区；构建使用同仓本地依赖与根锁文件：

```sh
CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 \
cargo build --release --locked --offline --target x86_64-unknown-linux-gnu \
  -p perf-check --bin perf-check --bin recovery-startup
```

新阶段探针随后以相同目标、特性和 profile 单独构建 `--bin resource-phases`。
唯一共享 target 为 `target/host-performance`；源码冻结后串行运行，构建与负载不重叠。
release 为 `lto="thin"`、`codegen-units=1`、`strip="debuginfo"`，未设 `target-cpu=native`，
编译指纹确认 storage-engine features 为 `[]`，没有 fault-injection。
新工具的准确补丁及 SHA-256 随阶段构建收据保存，不能把该工具的新增源码冒称干净 main。

| 二进制 | SHA-256 |
| --- | --- |
| perf-check | `827dd41e1086ae853d99404b42ed49dc71d9ca7500284199b530dcf41eef68ad` |
| recovery-startup | `b07dc6327678d0d1aefd6310bf493b5c4886b1b860ce06e294c021fed061a2ba` |
| resource-phases | `61b83b13c4355f548ee9205cdb75c20135130f7fce3c82972c1f99677cd47823` |

原始归档位于本云实例 `test/evidence/target-host-performance/20261101-cloud/`，按既有规则
忽略生成结果；归档包含冻结二进制、build 收据、source patch/hash、采集器精确版本、
stdout/stderr、资源采样、wait4 统计与 `SHA256SUMS`。文中摘录不代替原始数据。
运行目录分别为 `probe8-r1`、`fixed64-r1`、`growing64-r1`、`diagnostic` 和阶段探针目录。
采集器默认 60 秒、采样 RSS 2GiB、空闲预留 4GiB 是本次实验保护，均不是产品达标阈值。
每次轮询后 sleep 250ms，有采样开销和盲区；外层结束是观察到退出的时点，
采样、输出与 OS 调度均可增加滞后，250ms 不是严格上限。

## 既有探针的基线

每档一个进程、一次新 fixture，不将不同增长检查点当成相同规模的重复样本。
三份均显式写完整内容，Development 不建立真实掉电或故障域证明。

| 负载 | 外层完成秒 | 子进程 CPU 秒 | 峰值 RSS KiB | 最终 State JSON 字节 |
| --- | ---: | ---: | ---: | ---: |
| perf-check，8×1KiB 版本，另含 4MiB 范围读 | 4.007 | 0.953 | 24440 | 未统计 |
| recovery fixed，64×128B 版本，1 个活文件 | 22.303 | 6.384 | 28416 | 210330 |
| recovery growing，64×128B 版本，64 个活文件 | 33.071 | 10.257 | 35196 | 748758 |

fixed/growing 均有 65 份操作收据、65 个事件、64 个保留版本。
末 32 次事务（历史大小 33..64）中位时间分别为 252.686ms、405.212ms，
其中 commit 调用中位时间为 238.405ms、387.534ms。这是混合增长段的描述，
不是固定历史规模的 P95/P99 或单纯正文吞吐；外层时间还含初始化、重开与校验。

| 64 版本重开 | 未命中检查点，ms | 后续命中，ms | 后续命中，ms |
| --- | ---: | ---: | ---: |
| fixed | 4654.333 | 461.194 | 479.033 |
| growing | 7382.812 | 977.180 | 907.124 |

以上命中状态来自 `recovery_checkpoint_selection` 事件，不能只凭耗时猜测冷热。
各次都无 checkpoint load/save warning。重启状态、最老永久收据幂等重放不改变 State、
最老正文与活文件数正确，最终完整性检查通过，所有进程退出码为 0。
perf-check 另验证 4MiB 历史正文范围、持有已打开 FD 期间保存及保存后旧正文可读；
其四个 SQLite 计数检查点各插入 1 条历史行、0 update/delete。
原工具的 SQL 触发器、完整状态比较和全文件 `open_content` 预验具有观测成本，
不能冒充生产 HTTP/SMB 分段范围读取的耗时。

## 阶段测量的解释

阶段入口逐次分别计工作区创建、stage、准备捕获、execute 与 finish。
capture/execute/finish 与服务器调用的公开 API 相同，但本探针没有 HTTP/SMB 认证、
socket 传输、服务端锁竞争或真实客户端；因此没有锁等待和网络吞吐数据。
`execute` 同时包含 CPU 规划、复制、校验与元数据准备，不从总时间相减臆造其中细项。
`finish` 包含复核、发布、索引与工作区完成，不等于单独的 fsync 时间。
检查点 clone-only 单独计时，排除析构、比较与编码，也不包含触写后的 COW。

下表为新阶段工具的独立负载，不与原恢复工具构成优化前后对照。输入均为 64 次、128B、
三份，检查点为 1/32/64；下列中位数取历史 33..64 的 32 次事务：

| 阶段，ms | fixed | growing |
| --- | ---: | ---: |
| create workspace | 5.887 | 6.681 |
| stage | 7.407 | 8.416 |
| prepare capture | 0.025 | 0.114 |
| prepare execute | 184.438 | 304.392 |
| finish publication | 66.335 | 93.499 |
| create through finish | 263.440 | 416.162 |

以同一段事务的累计阶段时间除以累计总时间，execute 分别占 67.98%/72.62%，
finish 占 26.13%/23.43%。阶段中位数相加不等于总时间中位数。
两个进程外层完成时间为 22.306s/31.332s，CPU 为 6.041s/8.786s，峰值 RSS 为
27844/33488KiB。所有 128 笔事务和 6 个检查点的收据/所有者、重放不变、正文、
命名空间、重开和完整性断言通过；起步 fixed 8 的 8 笔事务、3 个检查点也通过。

fixed 在 1/32/64 检查点的 clone-only 为 5.040/18.440/24.032µs；growing 为
5.342/54.603/42.263µs，每点仅一次，不拟合趋势。当前 State 仅约 0.2/0.75MB 规范 JSON，
不足以判断大历史的分配或驻留规模；在这个小负载下，没有先改变事件容器的收益证据。
下一轮应先量化 execute 内的重复元数据工作与 I/O，再选择局部优化。

## 同步及字节成本的边界

独立诊断对同一冻结 perf-check 执行 `strace 6.13 -f -c`，输入为 4 个小版本，
另有工具自带大文件、重开、读及检查。退出码 0：整个进程记录 fsync 12436 次、
fdatasync 106 次、openat 7796 次、read 14666 次、write 720 次、mkdirat 4622 次、
clone3 17 次。mkdirat 的 4232 次错误是原始计数，不解释为性能失败。
这些次数覆盖整个工作负载，不是每次保存的预算。
`-c` 表格默认报告 syscall system time；本次未用 `-w`，不能拿其百分比归因端到端墙钟。
跟踪结果与正常延迟样本分离，未取消任何同步。

正文静态调用链预算：新版本大小 S、份数 r，无复用/故障时，构建哈希读 S、复制读写 rS、
准备证明读 rS，逻辑读取合计 `(1+2r)S`、写入 `rS`；保留 FD 证明不足时，决定与证明前
复验再读 `2rS`，合计 `(1+4r)S`。三份分别为 7S/13S 读、3S 写。
此项为产品接口的逻辑字节推导，不是块设备实际 I/O 或达到的吞吐。
文件/目录同步、不可变对象碰撞比对、完整性、撤权、永久收据和发布决定点均保持。

直接 Engine 工具没有 daemon 的真实周期巡检 owner 和成功 coverage permit，不能证明生产
旧依赖复用路径的性能。源码已有受该许可约束的依赖复用实现；编号性能/恢复文档中仍有
未启用的旧描述，需按实际入口与许可状态核对，不据旧文字重做优化或删除读验。

<a id="workspace-allocation"></a>

## 工作区分配优化与完整场景比较

产品范围仅为 `src/engine/src/business/workspace.rs`：冲突比较借用原来相同的五个字段并
保留迭代顺序；物理名称/位置已有键避免进入会触发 COW 的 `entry`；空命名空间列表时，
在 workspace 调用点跳过上下文重放。此时内容替换保留旧 entry/file/display-path 身份，
新增文件只追加，旧 file_id 全部仍存活。此前的继承首次写入及 node_id 更新照常执行，
通用 `directory::namespace_locations` 不变，所有写准入、所有者、fencing、基线和发布阶段保留。
变化不使整个提交成为 O(delta)，当前 entries 草稿、最终完整验证和其他 State 成本仍存在。

独立 `namespace-costs` 以一次批量提交建立 16/128 个 128B 文件，三份复制；分别测
空保存、覆盖一个文件、新建一个文件、覆盖四个文件、目录改名/删除和两种冲突拒绝。
夹具、竞争提交、正文/恢复/收据 oracle 与被测 capture/execute/finish 分开。
32 格全部退出 0、来源稳定、完整输出与全部正确性断言通过；两版相同探针源码 SHA-256 为
`c1438c0f8863a09e396484018ac5b6d1b47bda14c73f0d6be56cb7a66a319608`。

| namespace-costs 构建 | 来源 | 二进制 SHA-256 |
| --- | --- | --- |
| baseline | `983b5cc1` + 归档工具补丁，产品仍为 f22 | `96b945cb17cfb64545fbfc35985aacac902c0a4069afad9bd0b3c3843e61ac18` |
| candidate | `98ad8767` + 归档 workspace/测试补丁 | `6789ea70a54f09f951bfda15fed0ef45eb28e133b821d112eb1ccc6030ec0d8b` |

target/profile/特性与初始 release 相同，storage-engine 无 fault-injection；构建收据及准确
source.patch 在归档。前后 scratch 与实际 fixture 绝对路径等长，每档新进程和新池。
下面是 **execute 阶段 Rust 全局分配请求次数**，每格一次，不是整机内存或生产吞吐：

| 场景 | 16 文件：前 → 后 | 128 文件：前 → 后 |
| --- | ---: | ---: |
| empty | 16918 → 16599 | 55575 → 41248 |
| content-one | 35734 → 34987 | 127646 → 122308 |
| content-new | 33098 → 32904 | 151009 → 148146 |
| content-four | 63766 → 63070 | 161949 → 154177 |
| rename | 90961 → 90886 | 695461 → 694333 |
| delete | 19612 → 19465 | 58127 → 57007 |
| rename-conflict | 494 → 353 | 2552 → 1509 |
| delete-conflict | 490 → 349 | 2548 → 1505 |

128 文件 empty 的 execute 请求字节为 10962864 → 9959879，三阶段总请求次数为
66411 → 52084；两种 128 文件冲突各减少 1043 次、139035 请求字节。
逐阶段 System GlobalAlloc 观察器只计成功的 Rust alloc/alloc_zeroed/realloc；realloc 计完整
新请求大小，不扣释放。原生 SQLite malloc、mmap 和线程栈不在其中，另有 wait4 RSS。
分立 Relaxed 快照可纳入边界后台分配；请求字节是累计流量，不是存活/峰值内存。

三阶段总请求次数有 15/16 格下降，128 文件 rename 为 950633 → 950967（+334 次，
+2622 字节）；不能把 execute 的下降扩展为所有提交总成本下降。不同 UUID 图分布、
后台线程及同步等待仍影响样本。没有阶段峰值 RSS 或目标主机内存节省的结论。

### 墙钟负观察及少量交替复核

首轮 16 文件 empty 三阶段为 130.42 → 257.14ms，128 文件 content-new 为
937.15 → 1115.76ms。保留这两个慢样本，再各做三对 AB/BA/AB，共 12 轮；
使用原冻结 bits，不重编译或改变检出。每轮单独保留真正 binary build source 的原始收据/
补丁及 SHA，与当前 operator checkout 分列，没有把候选检出冒称旧二进制构建来源。
中性固定宽度输出名和实际 TMPDIR 等长，12 轮的正文/收据/重启/完整性断言均通过。

| 三次每版，三阶段总时间 ms | baseline 中位数 [min,max] | candidate 中位数 [min,max] |
| --- | ---: | ---: |
| 16 文件 empty | 142.232 [122.020,220.720] | 150.919 [136.887,201.136] |
| 128 文件 content-new | 1086.009 [1009.214,1108.034] | 1120.162 [1047.989,1160.853] |

对应 execute 中位数为 96.077 → 93.813ms、985.057 → 1029.685ms。
16 文件 empty 的三次 execute 请求次数稳定为 16918 → 16599；128 文件新增的
三阶段总请求次数范围为 172025..172330 → 169794..170091。
复核没有复现首轮成倍变慢，但总中位数仍分别高约 6.1%/3.1%，样本范围重叠。
保留候选的依据是局部分配和重复工作减少；不宣称延迟加速，也不排除较小延迟回归。
这些少量云样本不能构造 P95/P99 或目标硬件达标结论。

### 全局计数观察器与三种时间口径

这里的 32 格矩阵和 12 轮冻结配对均无条件启用同一 `CountedSystem`，贯穿 Rust 子进程
生命周期，包含夹具与 oracle。初始 `perf-check`、`recovery-startup` 和 `resource-phases`
没有安装这个计数观察器；它们的不同工作负载与时间不能直接拼作此处的前后对照。
capture 的 Python 进程也不由该 Rust 分配器计数。

每次成功 alloc/alloc_zeroed/realloc 在 System 返回后新增成功判断及两次 Relaxed 原子
fetch_add；另有阶段快照读取，dealloc 仅转发。Relaxed 仍有硬件成本和多线程竞争。
两版使用同一观察器统一口径，但分配次数、时序与线程交错不同，不能假定开销抵消。
已有证据没有 counter-off 对照，不能计算观察器开销百分比或从墙钟差中扣除估计开销。

| 时间 | 实际边界及范围 |
| --- | --- |
| namespace 三阶段与其和 | 各次 PhaseTimer 在开始读计数之前取 Instant，结束先取 elapsed 再读结束计数。包含 API 内 I/O、同步及观察器影响；capture 还包含闭包内 request.clone。三阶段和不连续，不含阶段间解包/Receipt 构造、create/stage、日志、预先 canonical、重启/校验和网络/SMB |
| capture 外层 wall_seconds | 源码在打开三个输出文件之前取开始时间，wait4 观察到退出后取结束时间；包含 Popen、整个夹具/操作/oracle、退出清理与观察滞后。前置冻结/provenance 和后置 source/binary 哈希不在其中；原 wall_scope 字段的 Popen start 是较窄的文字描述，应按源码边界解释 |
| 初始 resource-phases 的 create_through_finish | 一次连续时钟从工作区创建前到 finish 返回，包含 stage、request 构造和时钟开销；不包括其后的检查点，也没有真实传输或 mutex 等待观测 |

分配调用数和字节数各自独立读取，计数区间与 ns 并非完全重合的同一原子快照；后台线程
可能跨越边界。不能将差值全部归因一个私有函数。Python 轮询的名义 250ms 粒度不适用于
Rust 内部 Instant 阶段时钟，外层 wall 也不能替代小操作的阶段时间。

约 6.1%/3.1% 是每版三个样本的**边际中位数之比**。N16 empty 三对 candidate−baseline
分别为 +79.116、−83.833、+8.686ms；N128 content-new 为 −60.045、+74.844、+110.948ms。
后者两个边际中位数之差为 +34.153ms，而对内差中位数为 +74.844ms，不能混称同一估计。
各场景只有三对、各两正一负；范围重叠不证明无回归，也不证明回归。已有数据既不能
认定稳定的该幅度生产回归，也不能排除回归，分配请求下降与时延结论继续分列。

如集成决定必须区分较小延迟差，下一项可证假设可收窄为：不累计分配计数时，
N128 content-new 的产品前后阶段差是否仍有相同方向。最小提案只取这一场景、
预先固定三对 AB/BA/AB（六个新进程），用同边界 timing-only 工具、普通 System 分配器、
相同 target/profile/特性及完整 oracle，并重新绑定两份来源/二进制；分配字段标未测。
不自动扩大样本或设置新达标阈值。该提案尚未实施或运行；即便执行，也不能将新时段
timing-only 与旧 counted 数据直接相减当观察器开销，后者需要另行登记同产品来源、
同窗口的 counted/uncounted 对照。本候选保持独立分支，尚不据此合入 main。

### 正确性与静态验证

在同一冻结产品/测试源码、明确 x86_64 target 下：7 个新增私有 hot_path_tests、
15 个 composition_tests、35 个 branch_alias_operations、26 个 workspace_commit_replay、
3 个 workspace_namespace_preconditions 通过，共 86 项。新增组合覆盖继承 alias 首次写入、
另一明确分支 alias、未选冻结目录、物理名称、永久重试和重启后两份 FULL。
测试 profile 未优化，含 cfg(test)/既有 fault-injection，与无 hook release 测量分列。

两个既有 ignored 未执行：`unreadable_workspace_request_is_not_a_missing_mirror` 需要真实
无绕过能力的 EACCES 进程；`workspace_evidence_io_failure_is_not_absence_or_corrupt_mirror`
需要专用 LD_PRELOAD I/O 故障 harness。未计为通过，没有改系统权限或凭据来运行它们。
产品库与新探针严格 Clippy、指定 edition rustfmt、Python 语法及 diff 检查通过。
没有运行完整 workspace、Foundation 审改、真实 Samba/SDK 或物理掉电验收。

新增原始目录为 `namespace/` 与 `frozen-pairs/`，含全量 stdout/stderr、请求计数、
独立来源校验器、配对 runner、原始收据/补丁及逐轮 operator/binary 来源证明。

## 待所有权分配的候选与排除项

下面仅为独立设计与实验入口，没有实施产品行为变更，也没有 before/after 收益声明：

| 文件与行为 | 预期减少 | 实现前所需验证 |
| --- | --- | --- |
| `engine/publication.rs::run_payload_groups` 单组使用现有 worker | 一次 spawn/join 和临时线程栈 | 单组 panic/error、文件/路由顺序、多组 join/上限及真实保存占比 |
| `history_window/query_indexed.rs` 复用同冻结观察内已有 owner witness | 一次版本成员流扫描；候选实际认证保留 | 损坏回退、取消、旧快照、跨发布、独立窗口 oracle |
| `file_manager/listing.rs` 与 `directory_scopes.rs` 使用已准入排序定位子树/身份 | 无关命名空间扫描；目录分支扫描/排序仍保留 | Unicode 前缀、分页/计数/选中项、目录身份与授权重查 |
| `smb_rpc/{handles,session,transport}.rs` 直接序列化已验证正文 | 服务端中间数字 Value 数组 | 逐字节 wire、旧客户端独立解码、错误/FD/完整性/撤权 |

SMB 当前 64KiB 正文块意味着每 MiB 至少 16 轮本机串行 RPC；均匀字节的纯数字 JSON
数组长度约为原始字节 3.5703 倍。这是静态编码/调用次数推导，未测真实 Samba 吞吐。
客户端严格解码仍先构造完整 Value，仅改 typed response 不能消除这份分配，故排除该捷径。

还排除两项无收益的直接改法：`events` 仅改成 `Arc<Vec>` 仍在每次追加时复制前缀；
JournalWriter 当前 sequence 成功后清除已保存 tail，不存在已证实的全历史查找。
已实现的记录共享、按文件有界准备、保留 FD 证明及一次清单解码不重复计为本任务成果。
递归已有目录同步的复用需要持有目录身份、成功同步证明与失效/失败方案，不能直接删 fsync。

## 验证范围与剩余验收

已完成：精确目标和特性绑定的集中 release 构建、独立临时池负载、原始退出状态与资源记录、
真实重开/正文/永久收据和完整性断言、独立 syscall 计数。
这些是当前小规模云环境的软件证据，不代表全仓回归或性能目标已达到。

工具验证包括 Python 语法/CLI、阶段入口的指定 edition rustfmt 与严格 Clippy。
另四项采集器负向检查验证非法规模不启动、失败子进程不报成功、超时终止自己启动的
实验进程组、已存在输出不覆盖。它们的预期非零退出属于测试输入，不是产品性能失败。
`contract-check --root .` 退出 0：257 个 schema/语义向量、3 个规范编码向量、
2 个校验清单向量、5 个严格 JSON 拒绝及 2804 个本地链接通过；其范围仅为文档契约。
本轮没有非预期失败；没有用这些检查推定全仓回归、真实恢复或主机验收通过。

未执行：真实 Samba/VFS SDK 互操作、HTTP/SMB 混合并发及撤权负载、长期大历史、
周期巡检开启/许可状态下的前台干扰、目标主机的物理同步和故障恢复测量。
本轮未部署、未触碰真实数据盘、未改变权限/凭据，也未设置新的硬性验收阈值。
后续产品热点改动先分配文件所有权，再按同一精确源码/目标/特性比较 before/after；
格式、协议、同步阶段变化另行证明，不由测量目标授权。

### CI 预检记录与接线修复

提交 `054ba6cc` 的 [run 41325360732](https://github.com/isarmg/zxs/actions/runs/41325360732)
仅执行 Static job，因 `tools/target-host-performance/README.md` 位于 docs 之外而失败；
其余 Tests/SDK/WASM/Standalone 均跳过，不构成 CI 通过。工具说明移至
`docs/tools/target-host-performance.md` 并调整链接，没有放宽来源门禁或修改历史收据。
本地完整 `bash .github/scripts/check.sh static` 在进程内 umask 022 下通过。
本实例默认 umask 077 会使打包 mock 的解压目录成为 700、触发原 755 断言；未修改测试，
仅对该预检子进程设置创建掩码。产品/测试/计时工具源码与已测候选不变，性能实验未重跑。
