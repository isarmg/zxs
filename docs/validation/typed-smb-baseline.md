# SMB open 与 dirty 的精确 snapshot 查询

## 边界与范围

`Session::open` 的非空外部路径用 `LookupPath::parse` 替代原来的
`validate_lookup_path`，保留大小写和 NFD 等价查找语义。基线查询直接传递
`&LookupPath`、分支和 workspace 的冻结 `base_head`，返回 `Option<Entry>`。

`AuthorizedBackend` 仍刷新当前账户；`ManagementBackend` 仍经过运行状态准入；
`EngineBackend` 在原 engine 锁内复验账户和 SMB binding，复用 HTTP Tree
原有的分支、仓库、history/commit 校验和错误优先级。发布时已确立的条目顺序
允许查询直接借用 commit entries 做二分，只复制命中的单个条目。

删除的生产路径是可写 open 的 `BackendAction::Tree` 调用、整个 entries JSON
数组 clone、`from_value::<Vec<Entry>>` 和线性查找；身份比较也不再复制字符串。
没有 JSON fallback 或默认 Backend 兼容实现。HTTP Tree、workspace/fence、
FD 交付、未知写入和关闭/恢复流程不变。

初次 `Session::find` 仍有单条目 SmbLookup JSON 往返。下面的早期隔离阶段仅改
open；当前类型化 Entry 候选还复用了 dirty 的两个冻结快照查询，见后文。
这些单次查询耗时不能当作完整 open、dirty 或 SMB 吞吐。

## 可复现测量

```sh
cargo test -p storage-server --lib --release --offline --locked -j2 \
  measure_snapshot_lookup_against_tree_round_trip -- --ignored --nocapture
```

测试通过真实 Engine 的 workspace/namespace/stage_bytes/commit API 发布每组夹具：
N−1 个目录及最后一个 `target.txt` 文件。旧路径调用仍保留的 HTTP Tree 实现，
复制 JSON entries、解码并线性查找；新路径调用实际 typed Backend 方法。
同一 Engine、identity、commit、查询路径，先比较完整返回条目，再交替测量
101 次并保留返回值（`black_box`）。夹具创建和输入解析不计入查询耗时。
已有 Tree 投影计数器同时报告整树复制条目数；它不是堆分配器测量。

## 早期隔离结果（2026-10-26，非当前候选）

Linux x86_64 容器、Rust 1.99.0，仓库 release profile（thin LTO、单 codegen unit）。
对应代码 `a714697a`，真实文件夹具 `d0b4d262`；每组 101 对样本，单位 ns。

| 发布条目数 | 旧路径 p50 | typed p50 | 旧路径 p95 | typed p95 |
| ---: | ---: | ---: | ---: | ---: |
| 10 | 14,022 | 240 | 17,807 | 271 |
| 100 | 164,327 | 591 | 251,839 | 1,042 |
| 1,000 | 1,452,534 | 1,161 | 1,653,246 | 2,273 |

每次旧调用的 Tree 投影数为 1，投影复制的条目数分别为 10/100/1,000；
typed 调用两项均为 0，匹配返回值与旧路径相等。此计数仅覆盖 Tree capture 的
整树复制，不等于总分配次数或峰值内存；后续 JSON clone/解码由被删除的调用链
说明。当前数据支持本步 baseline 查询成本下降，不推导完整 open 的加速倍数。

## 早期隔离验证边界

- 真实冻结快照、等价 NFD/大小写路径、缺失、其他 history 的 commit、迁移分支。
- 注销、密码变更的确定性排队竞态，当前 SMB binding 不匹配。
- 既有可写 open/并发修改、选择 fence、未知写入、FD 与恢复用例。
- 本地 `storage-server --lib`：439 通过；6 项因沙箱拒绝 Unix listener bind
  (`EPERM`) 失败，随后显式排除这 6 项重跑其余全部通过；3 项 ignored。
  未把受限测试记为通过。提权执行器初始化失败，未执行受限测试，待真实 CI。
- `cargo clippy -p storage-server --all-targets --offline --locked -j2 -- -D warnings`
  通过；doc tests 通过（0 例）；变更文件 rustfmt 和 diff 检查通过。

受限的六项为 `service_lifecycle::tests::sockets::shutdown_closes_idle_smb_and_http_listener_while_http_request_is_stalled`、
`smb_rpc::tests::cancellation_closes_blocked_rpc_sessions_and_removes_its_socket`、
`smb_rpc::tests::close_tests::{failed_vfs_close_detaches_only_its_writer_and_keeps_both_sessions_usable,lost_close_reply_retains_original_published_operation_without_resubmitting,poisoned_vfs_lock_detaches_without_publishing_or_discarding_unannounced_bytes}`、
`smb_rpc::transport::startup_tests::bind_and_register_prepared_socket_preserves_permissions_and_identity_cleanup`。


## 类型化 Entry 集成候选复测

候选源码：`cbc1942c9db19365eabac1f7c11bafaccf15ebb6`，包含 typed Entry、
NamespaceOperation enum、typed upload 适配器、open 和 dirty 单条目查询，
以及不重新解析的 `PortablePath` → `LookupPath` 转换。
环境和真实文件夹具与上文相同；同一 release 二进制做三轮，每轮每组
101 对样本。这里的“旧路径”是候选中的 Tree/JSON/线性选择参考路径，解码
也经过当前 typed Entry；不是使用早期旧 Entry 编译产物进行跨版本计时。

| 轮次 | 发布条目数 | 旧路径 p50 ns | typed p50 ns | 旧路径 p95 ns | typed p95 ns |
| ---: | ---: | ---: | ---: | ---: | ---: |
| 1 | 10 | 17,116 | 230 | 33,380 | 351 |
| 1 | 100 | 286,681 | 861 | 4,396,860 | 5,438 |
| 1 | 1,000 | 2,281,780 | 4,156 | 8,524,192 | 9,735 |
| 2 | 10 | 16,956 | 220 | 22,313 | 250 |
| 2 | 100 | 169,275 | 361 | 209,795 | 601 |
| 2 | 1,000 | 1,775,609 | 1,372 | 2,010,071 | 2,383 |
| 3 | 10 | 17,266 | 220 | 86,069 | 380 |
| 3 | 100 | 171,859 | 351 | 307,873 | 912 |
| 3 | 1,000 | 1,786,305 | 1,452 | 2,051,404 | 2,264 |

三轮均先断言完整结果相等。共享容器有并发编译/测试负载，没有 CPU 绑核；
p95 明显波动，保留全部轮次，不选择最漂亮的一轮或推导完整请求加速倍数。
这些测量支持单快照查询减少实际工作；投影调用计数不是合规证明，也不是
内存分配、峰值内存或端到端吞吐测量。

当前候选的 `storage-server --lib --release` 可执行选择：443 通过、3 ignored，
仅精确排除上文六个已确认 Unix bind 受限用例。NFD 拼写打开的同一 handle
已覆盖第二次 dirty/flush；无关文件发布后重开 workspace、同文件变化冲突、
选择 fence、注销/SMB binding 与恢复用例均通过。
`storage-server`/`web-app` release 的 all-targets/all-features 严格 Clippy 通过。

### 该候选保留的边界

`finish_publication` 仍按已发布 commit 中的 `entry_id` 而非原 path 定位条目，
保留丢回复和路径变化后的身份语义；不能机械替换为现有 path lookup。
后续应提供真正的 typed identity lookup，只复制命中的快照条目。
namespace 的整树 precondition 检查需要 typed 整树或专用准入接口，是另一项
工作；本轮不宣称已删除这些 JSON/Entry 重新解析。

上述两条内部整树往返已在后续 [共享不可变 commit 快照](typed-smb-snapshots.md)
变更中移除；原先的 path lookup 不承担 entry_id 查询语义。
