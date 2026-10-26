# R5：恢复路径的有界去重

## 范围与边界

- 磁盘和原始离线包在读取边界生成 `AuthenticatedMetadataObject`：严格 JSON、引用 domain hash 和对象大小仅在此认证。解析器继续检查完整 graph schema、路由、页格式和实体唯一性；页长度、修复、导出和增量复用共享已冻结 canonical bytes。
- `AdmittedState` 只由完整 snapshot admission 产生，字段私有且无可变借用。候选精确历史图与该状态投影 canonical equality 成立后，省去第二次 `HistoryManifest::validate`。原始包、独立历史图和无 snapshot 的 checkpoint 路径保留语义准入。
- 已扫描 journal 的成员、slot、帧哈希随已有 sequence/candidate 分组传递；publication receipt 查找仅遍历当前候选组。最终 seal 校验、四阶段写入、原始 snapshot/graph 修复顺序保持不变。

未增加全局缓存、TTL、第三方包、磁盘/wire 字段或预算调整。测量工具只启用已有本地 tracing 的 `fmt,json` 特性；新增依赖边已记录在 Cargo.lock。所有验证类型保持 opaque，不增加全内容 Debug。

## 验证

- `cargo test -p storage-engine --all-features`：625 项通过，6 项既有手动/容量测量项默认忽略。覆盖 raw import、错 hash、重复 JSON 字段、已认证但不同/非法 manifest、缺失/分散副本、fork、torn-tail、成员帧差异和跨多个 decision slot 的 checkpoint。
- 另行单独运行 `distributed_history_larger_than_real_reader_limit_roundtrips -- --ignored --nocapture`：1 项通过。逻辑 manifest 为 70,580,229 bytes（超过 67,108,864-byte 单对象 reader limit），1739 个图对象中最大为 54,293 bytes；没有放宽 reader 或页格式。其余 5 个既有手动测量项未运行。
- `cargo clippy -p storage-engine -p perf-check --all-targets --all-features -- -D warnings` 通过；修改的 Rust 文件 rustfmt 和 `git diff --check` 通过。
- 独立只读源码复审覆盖最终生产提交 `de187851aff545ad9684916407e1724f820beacb`，无未解决 blocker。复审不替代运行测试或性能测量。

## 测量方法

测量源：remote payload 基线 `3fd13d3e205e59b7b8f840ae164ad947e9a06533`，基线仅叠加相同观察器与 fixture（本地 commit `ff394c0326f0b2fe4ff652c4e188491e9d840276`）；R5 为上述 `de187851…`。两个独立 target，Rust/Cargo 1.99.0，release、jobs=2、单次构建 `CARGO_INCREMENTAL=0`；仓库默认增量配置未修改。

[fixture](../../tools/perf-check/src/bin/recovery-startup.rs) 保留原 fixed/growing 实际工作负载与默认资源预算，去掉不相关的 clone/write/cursor 微基准。每个 variant/mode 创建一份新 Development fixture，三个元数据成员；在 64/256/1024（growing 止于 256）重复打开同一个 pool 三次。计时包围完整 `Engine::open`；采集器初始化、缓冲日志向 stderr 输出、JSON 解析和随后断言均在计时外；事件生成/写入内存缓冲仍在计时内，双方一致。每次确认 canonical State、最旧幂等 receipt 不改变状态；每个采样点确认最旧正文与 live namespace，进程末尾运行完整 integrity check。

正式四个进程串行、全部 exit 0，共 30 个完整 open 样本。无并行构建/其他 benchmark。以下是同一 fixture 的重复打开，不是三个独立数据集，也不是生产硬件/物理冗余测量；未做 profiler 归因或统计显著性声明。

## 完整启动结果

单位：秒；每格按第一次/第二次/第三次打开排列。选中序列直接取真实 checkpoint selection 事件，双方相同。

| 工作负载/历史 | baseline | R5 | 选中 checkpoint sequence |
| --- | --- | --- | --- |
| fixed 64 | 0.975 / 0.404 / 0.401 | 0.888 / 0.346 / 0.352 | 无 / 65 / 65 |
| fixed 256 | 9.021 / 4.592 / 4.679 | 8.142 / 3.412 / 3.453 | 65 / 257 / 257 |
| fixed 1024 | 162.616 / 162.860 / 161.770 | 134.528 / 130.615 / 135.394 | 257 / 257 / 257 |
| growing 64 | 2.199 / 1.107 / 1.119 | 1.899 / 1.034 / 0.897 | 无 / 65 / 65 |
| growing 256 | 170.658 / 78.606 / 84.748 | 122.933 / 49.212 / 46.402 | 65 / 257 / 257 |

- fixed1024：同一旧前缀下，中位数 162.616 → 134.528 秒，下降 **17.3%**。
- growing256：首次复用旧 seq65 后重放后缀，170.658 → 122.933 秒，下降 **28.0%**；后两次已覆盖当前 seq257，平均 81.677 → 47.807 秒，下降 **41.5%**。
- process peak RSS（包含工作负载构造、全部打开和检查，不是只测恢复分配）：fixed1024 96,580 → 99,524 KiB；growing256 187,324 → 199,608 KiB。观察到一定内存代价。

### checkpoint 证据与限制

fixed1024 的六次打开都选中 seq257，而不是当前 seq1025；每次 load warning 为空、save warning 为 `CHECKPOINT_INVALID: replay checkpoint coverage differs`。这是旧 coverage 通过 load、新 checkpoint 构建失败后保留旧前缀；`save()` 本身不比较旧 checkpoint。当前通用错误不能唯一定位 8 MiB、entry 数或文件类型分支，不能据此宣称已证明某个预算耗尽或并发修改。其他正式样本的 load/save warnings 均为空。

growing256 的后两次确实选中当前 seq257，仍需几十秒。该改动只消除已证明的重复认证、编码、语义准入和全日志查找；逐历史根展开、snapshot 重建及历史容器修复的剩余工作没有被全面增量化，**未消除全部历史规模二次方成本**。

## 复现与原始证据

[证据目录](../../tools/perf-check/results/recovery-dedup-20261026/) 包含逐次 JSONL/trace/warning、exit code、CPU/peak RSS、源码/tree/fixture/binary hash、完整 baseline/R5 overlay（gzip）和完整性摘要。`observer-verification.json` 逐字核对：基线 recovery 源文件移除唯一观察事件后与 remote 基线相等；双方观察事件相同。overlay 的 provenance hash 针对解压后的原始 patch。

在基线 checkout 应用对应 overlay 后分别构建（也可 checkout 可达的等价最终源码）：

```sh
CARGO_TARGET_DIR=/path/to/isolated-target CARGO_BUILD_JOBS=2 CARGO_INCREMENTAL=0 \
  cargo build --release -p perf-check --bin recovery-startup
/path/to/isolated-target/release/recovery-startup fixed 1024 >fixed.jsonl 2>fixed.stderr
/path/to/isolated-target/release/recovery-startup growing 256 >growing.jsonl 2>growing.stderr
```

归档的 Python harness 保留本次环境绝对路径，复用时需调整 root；两个 variant 必须使用相同 fixture 字节。`summarize.py` 仅在四个进程均成功、样本齐全且 binary hash 与 provenance 相符时产生完整摘要。
