# R86 frozen combined-candidate paired verification

Candidate: 99f02cb978bff53f28d1ad8414037a3f52d74cd2
Tree: 96d02c531530729ede98b524af819198cbdb3c8a
Baseline: e2f33d9882cf7263412d9370b6a703e0b216357d
Tree: bee9d4686088a4af6c659a983c6e711f5f62d272
Carrier branch: verify/namespace-build-once-20261029

Publish verification.yml as the carrier .github/workflows/ci.yml;
run.py as tools/namespace-adoption-bench/run.py;
{baseline,candidate}.rs as test/engine/bench/namespace_adoption/{baseline,candidate}.rs.
This carrier is not a product ancestor.

The Rust adapters remain byte-identical copies of the independently reviewed deduplicated verifier. This build-once carrier changes only verification tooling and its workflow: authenticated artifact reuse, pair-level execution and explicit aggregate accounting. The exact GitHub comparison contains188 changed paths; all public receipt/read/query/model/workspace/repository API implementation files used by the adapter are unchanged. Required public re-exports remain present. Source evidence: api-applicability.json.

The fixture remains one P01 member, target1,97 directories,N initial files and20 single-file edits:22 receipts/commits, exact namespaces andN+20 current/historical versions. Verification uses metadata-only operation_record and real publication/payload-verified open_content once per unique version, not22 complete-health scans. Progress is flushed to stderr outside measured work. Assertions must actually pass remotely; source applicability is not runtime proof.

Three paired untraced repetitions AB/BA/AB and a separate traced BA pair at500/1500/3000. Rust1.99.0, actual production features without fault-injection/test-support, bounded per-phase/global time/disk/address-space budgets and raw evidence are unchanged. See the reviewed R82 harness README for metric scopes and remaining allocator/cache/daemon-lock adoption gates.

These new results apply only to this frozen combined candidate, including admission-order/two-walk removal and the other combined changes. They do not isolate a single change and must not relabel the13b664e9 R82 measurements. Old R82 success/timeout artifacts remain intact. Later product changes require separate validation. No local product build/test/benchmark ran; only syntax/source checks. No adoption threshold or adoption decision is invented here.

## 精确产物复用与执行编排

验证载体独立于产品祖先。`verification.yml` 只在独立 carrier 发布为 `.github/workflows/ci.yml`，产品正常 CI 不变。两个冻结产品分别以 Rust 1.99.0、release、显式 `x86_64-unknown-linux-gnu`、通用 `x86-64` ISA 构建一次；仍使用各自根锁与离线本地来源，不共享不同产品的 target 目录。两个适配器字节不改。

`artifacts.py` 将源码 SHA/tree、完整来源目录摘要、锁／来源／配置字节、实际 Cargo 特性与 profile、Rust/C 工具链、代码生成环境、ELF 与动态依赖、适配器与二进制 SHA256 封入清单。当前 run 的 build-job 输出再钉定清单摘要；每个消费 job 核对相同 run/attempt/carrier、脚本和适配器、二进制、Ubuntu 24.04 x86_64 及真实动态依赖版本。复用只覆盖二进制，不缓存测试或验证结果。

三档规模各拆为四个独立 pair job，共十二个 job：0/1/2 为无追踪 AB/BA/AB，3 为独立 strace BA。每对仍在同一 runner 按固定顺序执行，双方和每轮使用新逻辑池；逻辑目录不代表独立物理盘。初始发布、20 次保存、97 目录、22 个永久结果与全部历史命名空间、N+20 唯一正文的真实 open_content 核验、P01/target1 均保留。verify 是语义校验，不新增计时值，不宣称运行完整提交健康扫描。

每个测量子进程仍有 20 分钟、8 GiB 地址空间及至少 8 GiB 空闲盘预算，3000 档启动前要求 24 GiB 空闲盘；所有 pair 共用构建开始前设定的同一个 150 分钟绝对整体截止（包含构建时间），每个 pair job 上限 180 分钟，保留上传时间。一方失败或预算耗尽后，另一方在剩余预算内独立尝试，未执行阶段明确记录。失败和部分原始日志全部上传；缺档、缺轮、超时、空间不足或身份校验失败不会被标为完整成功。`summarize.py` 对十二对逐项核账，独立展示失败与未完成；只有两方五阶段都完成且实际断言通过的无追踪对才进入汇总计时。

新增通用 ISA 参数属于本轮明确记录的构建输入；旧 13b664e9 / run40824412438 的结果及 baseline verify 超时证据保持原状，不能归给冻结 99f02cb 候选。本任务不代替主任务的正常产品门禁、Backend 争用、namespace-cache/heaptrack 或 ADR0011 采用验收。

清单绑定 attempt，失败恢复须启动整个 workflow 的新运行；单独重跑失败测量 job 会因旧 attempt 产物被拒绝，不能据此绕过清单身份。

## 本轮 GitHub 运行证据

[run 40835674738](https://github.com/isarmg/zxs/actions/runs/40835674738)，终态 `failure`，测量汇总 `incomplete`。启动 `2026-10-29T07:28:41Z`，终态更新 `2026-10-29T08:34:05Z`。

工具实现提交 `73615f38e6f69d92420b3c3573101a5e013c97c5`；独立 carrier `0e274a82e53652d99b6d8676986dbacc56bad179` 没有父提交。工具的 15 项边界回归于 GitHub 实际执行通过；两版固定 Rust 1.99.0 release 构建、原始源码范围核对和封装均通过。本节只登记本轮冻结输入的实际结果，不表示正常产品 CI 或 ADR0011 采用门槛通过。

| 文件数 | 完整对照 | 失败 | 未完成／缺失 | 状态 |
| --- | --- | --- | --- | --- |
| 500 | 4/4 | 0 | 0 | `completed_measurements` |
| 1500 | 4/4 | 0 | 0 | `completed_measurements` |
| 3000 | 3/4 | 0 | 1 | `incomplete` |

十二对全部实际启动，最终 11 对完整、1 对未完成；110 个子阶段成功，2 个子阶段因预算中断，后续 8 个子阶段未执行。唯一缺口是 3000 档独立追踪 BA 对：候选先执行约 1200.347 秒，基线在候选超时后继续执行约 1200.349 秒，双方初始批量发布均触及 `phase_time_budget`，退出码 `-15`。两份 stdout 未产生完整测量值，原始部分 strace 和时戳仍保留；不能作为完整负载计数或正文校验通过。没有改负载、减少重复或放宽预算。全部十二对使用同一绝对截止 `1793267940`（2026-10-29 09:59 UTC），本轮未耗尽该整体截止。完整的 22 次验证进程共实际读取并比对 34440 份池内唯一版本正文。

以下先在每个对照中计算候选耗时÷该对基线耗时，再对三个比值取中位数；1 为持平，较小表示候选该项耗时较低。绝对值中位数与配对比值分别统计。仅双方五阶段及完整语义断言全部成功的无追踪对进入统计；各阶段绝对值与全部逐对数据保存在汇总 JSON。初始批量发布的 `build` 与 Cargo 编译分开计价。

| 文件数 | 初始批量发布 | 20 次保存 | 首次查询批次 | 重复查询批次 | open |
| --- | --- | --- | --- | --- | --- |
| 500 | 0.960（3 对） | 1.017（3 对） | 26.807（3 对） | 6.415（3 对） | 3.419（3 对） |
| 1500 | 0.937（3 对） | 0.827（3 对） | 15.010（3 对） | 5.174（3 对） | 1.887（3 对） |
| 3000 | 0.936（3 对） | 0.877（3 对） | 24.386（3 对） | 3.795（3 对） | 0.896（3 对） |

以下为三轮无追踪 `open` 子进程的 VmHWM 中位数，单位为 Linux `/proc` 的 KiB；这是整个该进程的驻留高水位，不是分配次数或峰值活动堆。

| 文件数 | 基线 KiB | 候选 KiB | 候选／基线 |
| --- | --- | --- | --- |
| 500 | 59920 | 36272 | 0.605 |
| 1500 | 151628 | 86172 | 0.568 |
| 3000 | 429156 | 127452 | 0.297 |

已完成的每方每轮实际校验 22 个永久结果、owner/sequence/提交链、22 个历史命名空间与 HEAD、N+20 份唯一正文和 P01/target1 的唯一 placement。`receipt_api=operation_record`，`full_health_api_calls=0`；未完成的阶段保留缺口，不从其他轮或旧提交补用通过结论。

独立 strace 覆盖每个阶段整个子进程，包含其初始化／恢复；Rust `report` 计时窗口保持原样：批量发布在 initialize 后开始，edit/read 在 Engine::open 后开始，open 直接计 Engine::open。追踪观察者耗时不并入三轮无追踪统计，系统调用计数不冒充相同 Rust 计时窗口。

| 文件数 | 无追踪 AB（0） | 无追踪 BA（1） | 无追踪 AB（2） | 独立 strace BA（3） |
| --- | --- | --- | --- | --- |
| 500 | [原始对照 0](https://github.com/isarmg/zxs/actions/runs/40835674738/artifacts/12880673447) | [原始对照 1](https://github.com/isarmg/zxs/actions/runs/40835674738/artifacts/12880673394) | [原始对照 2](https://github.com/isarmg/zxs/actions/runs/40835674738/artifacts/12879899889) | [原始对照 3](https://github.com/isarmg/zxs/actions/runs/40835674738/artifacts/12881476736) |
| 1500 | [原始对照 0](https://github.com/isarmg/zxs/actions/runs/40835674738/artifacts/12881126790) | [原始对照 1](https://github.com/isarmg/zxs/actions/runs/40835674738/artifacts/12880828424) | [原始对照 2](https://github.com/isarmg/zxs/actions/runs/40835674738/artifacts/12880564823) | [原始对照 3](https://github.com/isarmg/zxs/actions/runs/40835674738/artifacts/12881958637) |
| 3000 | [原始对照 0](https://github.com/isarmg/zxs/actions/runs/40835674738/artifacts/12883037712) | [原始对照 1](https://github.com/isarmg/zxs/actions/runs/40835674738/artifacts/12883280989) | [原始对照 2](https://github.com/isarmg/zxs/actions/runs/40835674738/artifacts/12883098441) | [原始对照 3](https://github.com/isarmg/zxs/actions/runs/40835674738/artifacts/12883210140) |

构建与汇总：[全部构建日志／输入证据](https://github.com/isarmg/zxs/actions/runs/40835674738/artifacts/12881445273)；[两版二进制／manifest／ELF／适配器／构建证据](https://github.com/isarmg/zxs/actions/runs/40835674738/artifacts/12881001301)；[十二对终态核账及全部有效计时](https://github.com/isarmg/zxs/actions/runs/40835674738/artifacts/12883163611)。本轮 artifact 保留 14 天，具体过期时间以 GitHub API 为准。

清单 SHA256：`beee9eff05de3ec7dc6a034e49aa13792f138eccfb5ee1dff54ae60e60ad11d0`。
baseline binary SHA256：`5fe3d3ce0acc943b5b3553f162c9bff0d8d9be0d1771c2e52c3d413d706ab36d`。
candidate binary SHA256：`932089692cd6cea0d600309eb9b53c2fefef7376c34989fa301522a2c0fe6ca6`。

实际 engine library 与适配器 binary 的 features 均为 `[]`，没有故障注入或 test-support。manifest 保存实际完整 Cargo 编译目标图、release profile、Rust/C 工具链、通用 ISA 参数、动态依赖解析及版本。每个消费 job 的独立身份审计、真实 PID／起止戳、stdout／stderr／phase JSON 与部分结果保存在各对 artifact。

这些数据仅适用于冻结 99f02cb 候选。读取和 open 存在回退，不能据内存下降或工具改造成功宣称产品采用完成。VmHWM 为进程驻留高水位；VmRSS 为采样时驻留内存，两者均不是分配请求数或峰值活动堆。rchar 不是设备读取。真实冷 OS 缓存、分配／heaptrack、Backend 锁争用、namespace-cache 逐出及正常产品门禁仍由对应任务验收。
