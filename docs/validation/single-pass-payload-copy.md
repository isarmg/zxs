# Single-pass payload preparation and export

## Scope and boundary

The recorded **local** measurements use production baseline
`3e414b5c81e72fbc55d59fbaf38a77519f9fe586`. Both local builds also use the recovery
test-call correction from `9298855066b014cbe017a8d890e336d678c64cf1` and the identical
new fixture. These overlays do not alter the measured paths; the original local
logs and results retain that baseline identity.

The **remote CI** workflow instead uses the published, fetchable commit
`9552cf36910b6b644d97d149f692e5c6e2cee841`. Its payload preparation, disk copy,
container installation and online-read production files match the local baseline;
the recovery test-call correction is already included. CI overlays only the
payload fixture and its module declaration, without replacing recovery tests.

- Each fresh payload copy computes the existing complete-file hash and checksum
  manifest from the bytes actually copied, compares the version commitments,
  sets the read-only mode and synchronizes the output before publishing its name.
  `VerifiedCopy` privately owns that output descriptor and borrows the exact
  `FileVersion`; installation consumes it and holds the descriptor through the
  move. The duplicate fresh-body readback is removed.
- Existing prepared bodies, already installed targets, startup/recovery inputs
  and published replicas remain disk-input boundaries with actual byte checks.
  An already present target is verified even when a fresh copy is available.
- Online export checks seal, publication proof and checksum-manifest metadata,
  then verifies the body while writing a temporary output. Source failures permit
  replica fallback. Destination, synchronization and cleanup failures stop the
  export and retain their cause. `NOREPLACE`, file sync before rename and parent
  directory sync after rename are retained.
- File-branch selection and merge precondition scans, offline export, wire/disk
  formats, dependency identities and the per-chunk plus complete-file hash
  commitments are unchanged. This change adds no verified-content cache.

## Behavior checks

New cases cover fresh installation retaining the ordinary single inode, rejection
of an existing corrupt target despite a fresh verified source, external resumed
body corruption, source growth after preparation planning, corrupt/truncated/
oversized replica fallback, corrupt manifest and missing seal fallback, all-bad
replicas leaving neither final output nor temporary copy, and destination
no-overwrite preserving the original error and bytes. Existing preparation,
recovery, publication and export commitment tests continue to run.

`cargo test --offline --locked -p storage-engine`: **622 passed**, 0 failed,
6 ignored (including the two explicitly invoked measurement fixtures).
The initial implementation passed all 19 performance-tool Python tests, including
four trace-parser cases, plus workflow and architecture checks. The workflow-only
provenance follow-up adds four passing lightweight tests; it does not rerun the
Rust build or measurement. Changed Rust files pass rustfmt;
`git diff --check` passes. Production `cargo check --lib --no-default-features`
and `cargo clippy --all-targets --all-features -- -D warnings` pass. The initial
clippy attempt hit executor ENOSPC; the completed rerun used one build job and
`CARGO_INCREMENTAL=0`. ENOSPC, file-sync and parent-directory-sync failure
propagation were reviewed in source; no new injected failure tests dynamically
exercise those paths. No physical power-cut or disk-fault claim is made.

## Same-fixture local measurements

Rust/Cargo 1.99.0, unoptimized test profile, Linux cloud overlay filesystem,
64 MiB payload, three alternating baseline/candidate rounds. Setup and final
integrity checks are outside the measured operation. Preparation includes all
three routed copies and their installations; export includes the complete public
`Engine::export_version` call. These are page-cache/cloud-filesystem results,
not HDD throughput claims.

| Complete operation | Baseline `rchar` | Candidate `rchar` | Baseline median | Candidate median |
| --- | ---: | ---: | ---: | ---: |
| Fresh prepare, 3 replicas | 402,654,189 B | 201,327,597 B | 397.35 ms | 214.45 ms |
| Online export | 134,239,247 B | 67,130,383 B | 140.65 ms | 115.33 ms |

The read-counter differences are exactly three payloads (192 MiB) and one payload
(64 MiB), respectively. `rchar` includes metadata and a small sampling overhead;
it measures bytes returned by reads, not physical device traffic. `wchar` is
unchanged: 201,353,946 B for prepare and 67,108,928 B for export. Median elapsed
time decreased 46.0% and 18.0% in this run.

Median process peak RSS, including setup: prepare 20,244 → 20,764 KiB; export
21,200 → 21,376 KiB. This is a process-lifetime high-water mark rather than an
isolated-operation peak. The fixture streams its input and the implementation
continues to buffer one checksum chunk, not the whole body.

## Reproduction and syscall evidence

The two ignored tests in
`test/engine/unit/engine/payload_measurement.rs` are installed byte-identically
into both source trees. Run:

```sh
python3 tools/perf-check/payload-syscalls.py BASELINE_TEST_BINARY CANDIDATE_TEST_BINARY OUTPUT --no-trace
```

The output keeps each round's log and `summary.json`; `--rounds` adjusts the
sample count and `ZXS_PAYLOAD_MEASUREMENT_BYTES` selects the common payload size.
The test samples `/proc/self/io`, `/proc/self/status` and `Instant` only within
its own code; production has no measurement hooks or counters.

Local ptrace is unavailable. The one-off `payload-measurement.yml` workflow has
been retired after the [completed comparison](https://github.com/isarmg/zxs/actions/runs/40266410579).
Its `payload-copy-comparison-<candidate SHA>` artifact retains the raw traces,
per-round logs, build logs and JSON results. The comparison checked successful
payload reads of **6 → 3 passes** for preparation and **2 → 1** for export, with
unchanged **3 / 1 write passes**; these are historical measurements, not claims
about arbitrary later revisions.

The retained tracer resolves ordinary body, staged-body and temporary-output
descriptors and rejects incomplete marker pairs; its parser is independently
tested. `tools/perf-check/payload-provenance.py BASELINE CANDIDATE OUTPUT` records
actual source commits, the shared fixture hashes, Rust/Cargo verbose versions
and both executable SHA256 hashes before measurement. `baseline-overlay.patch`
includes tracked and untracked baseline fixture changes and is bound by its
SHA256 in the manifest. The scripts and fixture remain available for explicit
reproduction; normal CI and releases do not rebuild the old baseline.

## 已有副本准备阶段的单次验证

本次后续优化以 `7834d50e5436e6ff81948e2a68093afc373c45b9` 为基线，
不改变 `ContentHash` 或任何磁盘字节格式。实际调用链为：
`prepare_publication_bounded` 容量准入 → `prepare_payloads` →
`prepare_payload` → `install_version_content` → `PayloadPreparation::verify`。
原先同一已有正文在前三个阶段分别进行完整校验；最后一次读取负责建立决定证明。

现在容量准入的实际 manifest、逐块和完整 hash 验证产生私有 `VerifiedExisting`，
只持正文 FD、精确版本借用及正文/版本目录路径。准备 worker 接收并消费这份证明；
安装通过既有安全打开及 `FileIdentity` 比较，确认同步的是当前名字指向的同一 FD。
内部错绑是断言揭示的程序错误，外部叶节点替换则返回错误。
不建立 mtime/ctime、目录或设备状态缓存，不增加配置、通用 proof trait 或可写 FD 接口。

已有副本和决定证明使用同一进程级可选描述符预留池；首次使用按 NOFILE 软上限的四分之一
取值，最多 8192，无限上限也取 8192。这是工程默认值，不代表测得的可用 FD 或进程总量保证，
不会调整 OS 限制。已有副本每份预留一个正文 FD，无法预留的副本
在准备时重新实际验证并立刻安装，仍不再重复紧邻的安装 hash。
四个既有 worker 各可短时持有当前副本，所有准入证明在建立决定证明前释放。
仅明确 `NotFound` 才按待复制容量计费；损坏、权限、路径及 I/O 失败保留原原因拒绝，
不再被 `is_ok()` 吞掉或被 `NoSpace` 遮蔽。

已有正文必要的文件同步以及所有目录同步不变。两个实体同时存在时，验证过的 staged
正文不能为另一个已有 target 背书；另一个实体仍实际读取校验。
原 inode 原地改写不是这份短命证明的失效缓存协议：后续复制仍验证完整字节，
`PayloadPreparation::verify` 也仍在 ACCEPT 前真实全读，因此安装 token 不能取代决定证明。
生产中不存在独立的恢复 `Existing` 安装入口，已删除仅为旧分派保留的 variant；
恢复、离线导出、分支选择/merge 前置读取和未知公开调用方的实际验证保持不变。
独立完整副本和永久操作收据语义不变。

测试增加正确旧目标/staged inode、叶/父路径/盘路径替换及缺失、内部错绑断言、
另一个损坏 target、容量零余量下的有效旧副本、损坏 manifest/正文与 unsafe 路径的错误保留、
注入 128 描述符局部预算下的 129 副本回退，以及安装后原地改写仍被实际决定校验拒绝。
决定证明测试保留同一注入预算下的 32／33 边界，并覆盖 132 预算下保留 33 份证明、
不同 Engine 并存准备共用预算、错误／过期／销毁释放，以及有序匹配的身份与完整消耗。
这些结构与精确哈希／查找次数测试不构成生产吞吐量或 FD 容量测量。
原容器测试改经真实 manifest/正文解析构造器，不保留测试专用生产 variant。

### 安装文件同步的隔离故障验证

`test/engine/unit/container_storage/fsync_eio.c` 仅用于隔离测试子进程，
不链接产品或修改全局环境。它只对测试临时容器中匹配版本正文路径的普通文件
`fsync` 返回 `EIO`，不拦截目录或无关文件。先用正常构建生成 engine 单元测试二进制，
再显式运行（`ENGINE_TEST_BINARY` 为 Cargo 输出的该二进制绝对路径）：

```sh
tmp=$(mktemp -d)
cc -Wall -Wextra -Werror -shared -fPIC \
  test/engine/unit/container_storage/fsync_eio.c -ldl -o "$tmp/fsync-eio.so"
env LD_PRELOAD="$tmp/fsync-eio.so" "$ENGINE_TEST_BINARY" \
  existing_body_sync_io_failure_retains_its_cause --ignored --nocapture --test-threads=1
```

测试要求保留真实 errno/source、正文和无关文件原字节，并先成功同步父目录及无关文件。
这是用户态 syscall 故障注入，不是物理盘故障或掉电证明；未使用 ptrace 或提权。

### 本次验证与读量证据

`cargo test --offline --locked -p storage-engine`：**643 passed，0 failed，8 ignored**；
另行显式运行上述 EIO fixture：**1 passed**，记录一个目标正文 fsync 的真实 `EIO`，
父目录及无关文件正常，原字节未变。完整测试包含恢复、离线、分支/merge 与固定字节回归；另行
`cargo test --offline --locked -p storage-contract content_hash` 的3个固定摘要/磁盘字节与边界用例通过。
`cargo clippy --offline --locked -p storage-engine --all-targets --all-features -- -D warnings`、
`cargo check --offline --locked -p storage-engine --lib --no-default-features`、
architecture、修改文件 rustfmt 及 `git diff --check` 均通过。

两个独立 worktree/独立 Cargo target，Rust/Cargo 1.99.0、jobs=2、debug=0、incremental=0，
同一 `existing_publication_operation` fixture：64 MiB、3 个已安装副本、3 轮交替执行，
设置准备不计入测量；测量完整 `prepare_publication`，包括容量准入、安装与真实决定证明。

| 中位数 | 基线 | 候选 |
| --- | ---: | ---: |
| `rchar` | 805,399,569 B | 402,721,719 B |
| `wchar` | 34,793 B | 34,793 B |
| 本次共享环境耗时 | 536.28 ms | 280.02 ms |
| 进程生命周期 peak RSS | 20,900 KiB | 20,368 KiB |

正文读取从 12 次降至 6 次：每个副本省下准备和安装两次重复读取，保留预算一次与
决定证明一次；读计数差还包含少读的 checksum manifest。`rchar` 是返回给进程的读取字节，
不是物理盘流量；RSS 不是独立操作峰值；并行共享环境耗时不作 HDD 或生产吞吐承诺。
必要文件/目录同步与独立副本写入语义未改变；本次未取得 syscall trace 或物理掉电证据。

[原始计数、源码/fixture/二进制哈希、独立 target 来源及测试日志](../../tools/perf-check/results/20261026-existing-payload/)
保留在同一归档；`baseline.patch.gz` 仅覆盖共同测试 fixture，候选的生产文件哈希记录在
`provenance.json`，`candidate.patch.gz` 保存测量时工作树差异。EIO shim 源码以根 `test/` 文件
和归档 SHA256 绑定，未进入产品构建。
