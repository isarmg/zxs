# Recovery deduplication: raw observations

Measured source: `de187851aff545ad9684916407e1724f820beacb`; remote baseline: `3fd13d3e205e59b7b8f840ae164ad947e9a06533` plus observation-only overlay. See [report](recovery-deduplication.md).

- `source-provenance.json`: source/tree/fixture/lockfile/binary hashes and isolated targets.
- `observer-verification.json`: exact baseline-only observation check.
- `*-r1.*`: four completed formal processes; `*-smoke.*`: separate observer smoke checks.
- `summary.json`: validated complete sample comparison; not a production timing gate.
- `*-overlay.patch.gz`: decompress to reproduce measured source; hashes refer to decompressed bytes.
- `run.py`, `capture-provenance.py`, `summarize.py`: captured local harness; adjust absolute workspace paths before reuse.

Each formal process finished integrity checking with exit code 0. Repeated opens within one fixture are not independent datasets. Warnings and actual selected checkpoint sequences are part of every sample; a selected old prefix is not full warm recovery.

## 原始快照证明与恢复结果分离

恢复读取现在保留精确 snapshot hash 与完整准入所得的 `AdmittedState`，形成只能由原磁盘认证入口构造的 `AdmittedSnapshot`。工作区冷准入的短期共享借用保留同一 hash；恢复消费者在候选 history manifest 比较前核对该 hash 与 `Candidate.snapshot_hash`。错误候选不能借用另一个 snapshot 的准入结果。pool、可信成员和持有的盘根仍属于该次恢复调用的既有上下文；这个类型不授予跨恢复、跨成员或磁盘健康缓存的复用能力。

checkpoint 边界、正常发表完成及缺成员但已证明发表的三条路径，均先使用原始证明完成既有候选／历史／barrier 检查，最后消费该证明为普通可见 `State`，并按原逻辑叠加 recovery publication 的操作结果。叠加后的 State 不再携带原 snapshot 的类型证明；不能仅凭 Engine 的 snapshot 字符串把可见结果当成磁盘原字节。

这一步只收敛真实恢复消费链的证据身份。仍逐次执行原磁盘读取、摘要认证、RawState 完整准入、历史投影、修复和固定成员屏障，没有对象缓存、差分回放、格式变化或新的资源上限；不声明启动时间、复杂度或驻留规模改善。

新增回归覆盖错误 snapshot hash 在共享借用前后被拒绝、消费证明后叠加结果不改变原磁盘状态，以及第一次准入后发生损坏时下一次读取仍失败。既有恢复、checkpoint 和 degraded recovery 的语义不因这层证明改变。
