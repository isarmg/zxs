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
