# SMB 共享不可变 commit 快照

## 范围与边界说明

在 `68123e1f` 的 typed-path consumers 基础上，移除剩余两条内部
`Tree → JSON entries clone → Vec<Entry> decode` 路径：

- `finish_publication` 在返回的**已发布 commit** 中按 `entry_id` 选择文件，
  保留打开描述符的旧路径与发布快照路径不同的身份语义。
- namespace 删除、类型、非空和 replacement 检查借用该 workspace 的
  **`base_head`**，不会因并发提交而切换到当前分支的命名空间。

`SharedRecords::get_shared` 只增加已有记录 `Arc<T>` 的引用计数。
`Backend::snapshot_at` 在 engine admission 锁内保持当前账户、SMB binding、
branch/repository/history/commit 校验，然后返回已存在的 `Arc<Commit>`。
离开锁后借用快照条目；没有重建整树、序列化、解码或再次验证 Entry。
`AuthorizedBackend` 与 `ManagementBackend` 保留各自现有的账户刷新与运行状态准入。

外部命名空间请求仍在入口解析为 `LookupPath` / `PortablePath`，预条件查询
复用缓存 key 和发布时保证的排序。既有 JSON namespace staging 适配器仍是
独立边界，本次不宣称已清除该桥的全部重复解析。HTTP Tree wire format 不变。

共享快照仅保存数据，不授予后续读写权限；内容读取、workspace/fence、
发布时重新授权、操作/路径 reservation 和失败清理没有改变。
`Arc::make_mut` 与容器写入均遵守原有 COW；没有新增 unsafe、全树防御副本、
泛型接口或生产计数器/测试 hook。

## 行为验证

- 与既有 committed record 共享指针和 entries allocation；后续真实发布与
  engine drop 后旧快照保持原值。
- `SharedRecords` 双向 COW、非 `Clone` payload 与 map drop 后生命周期。
- foreign/missing commit、missing branch、archived read、migrated branch
  与原 Tree 错误一致；注销、密码变更的排队竞态和 SMB binding 变更仍被拒绝。
- 在 workspace 创建后、预条件读取前，以真实后端提交替换类型、删除子项或
  创建缺失项；四种检查仍依据旧 base，失败后 workspace/record/reservation 清理。
- 普通丢回复先按原操作恢复；随后实际重命名 commit 的 publication 比较仍按
  entry_id 成功，即使描述符保留原路径。此处直接测试 `finish_publication`，
  不把同一 workspace 中 rename+upload（原有 dependency guard 拒绝）说成支持场景。
- 原有丢回复后新增 FD 写入、读取快照/内容失败、replacement 与 case-only rename
  测试继续覆盖可重试状态与文件版本语义。

## 局部对照测量

```sh
cargo test -p storage-server --lib --release --offline --locked -j2 \
  measure_shared_snapshot_against_publication_and_namespace_tree_round_trips \
  -- --ignored --nocapture
```

真实 Engine 发布每组 10/100/1,000 个条目的相同夹具。旧路径使用保留的
HTTP Tree、entries JSON clone 和 Entry decode；新路径使用实际 snapshot_at。
publication 对照按 entry_id 查询；namespace 对照在两条路径上运行相同的
类型/稳定身份/非空谓词，单独隔离被删除的往返成本。夹具构造与发布不计时，
先确认结果一致，再交替执行 101 对样本，以 black_box 保留结果。

这是局部 snapshot acquisition + 谓词比较，不包含上传、内容读取/哈希、fsync、
提交或网络，不代表完整 SMB 请求吞吐。publication identity 查询与目录非空
检查仍可能线性扫描借用切片；没有为此增加第二份 namespace/index。

### Release 同夹具结果

Linux x86_64、Rust 1.99.0（b940084d7）、仓库 release profile（thin LTO，
单 codegen unit）。同一二进制三轮，各组 101 对；所有数值单位为 ns。

| 轮次 | 消费者 | entries | 旧 p50 | shared p50 | 旧 p95 | shared p95 |
| ---: | :--- | ---: | ---: | ---: | ---: | ---: |
| 1 | Publication | 10 | 27,041 | 221 | 28,863 | 390 |
| 1 | Namespace | 10 | 26,901 | 300 | 29,885 | 461 |
| 1 | Publication | 100 | 169,304 | 451 | 275,234 | 1,082 |
| 1 | Namespace | 100 | 173,641 | 671 | 304,729 | 1,643 |
| 1 | Publication | 1,000 | 1,739,124 | 3,866 | 2,124,563 | 6,440 |
| 1 | Namespace | 1,000 | 1,798,143 | 5,488 | 2,239,297 | 11,547 |
| 2 | Publication | 10 | 16,394 | 140 | 18,698 | 191 |
| 2 | Namespace | 10 | 16,445 | 170 | 20,952 | 220 |
| 2 | Publication | 100 | 181,393 | 581 | 330,376 | 1,062 |
| 2 | Namespace | 100 | 174,102 | 681 | 301,613 | 1,172 |
| 2 | Publication | 1,000 | 1,811,042 | 4,306 | 2,330,863 | 8,253 |
| 2 | Namespace | 1,000 | 1,905,483 | 6,359 | 2,756,482 | 12,258 |
| 3 | Publication | 10 | 16,815 | 140 | 22,914 | 201 |
| 3 | Namespace | 10 | 16,815 | 171 | 25,549 | 280 |
| 3 | Publication | 100 | 280,041 | 921 | 344,608 | 1,672 |
| 3 | Namespace | 100 | 265,669 | 1,112 | 322,665 | 1,853 |
| 3 | Publication | 1,000 | 1,675,488 | 3,866 | 1,763,740 | 5,217 |
| 3 | Namespace | 1,000 | 1,688,036 | 5,288 | 1,984,302 | 7,341 |

共享执行器存在其他构建/测试负载，未绑核；保留全部轮次及 p95 波动。
每轮都先验证旧、新谓词结果相同。该数据不包含分配器或峰值内存测量。

## 验证结果（2026-10-26）

源码与测试候选：`11283305c6e1de9322109dd62f2e3d22a7bf70d1`。

- `storage-contract`：73 通过，doc tests 0 例通过。
- `storage-server` native：library 449 通过 / 4 ignored；binary 6、HTTP 54、
  login 3、startup 6、telemetry 1 通过；doc tests 0 例通过。
- `storage-contract` / `storage-server` native 与 release
  `--all-targets --all-features` 严格 Clippy 均通过（`-D warnings`）。
- `cargo fmt -p storage-contract -p storage-server --check` 与
  `git diff --check` 通过。
- 本地只沿用已记录的六项 Unix bind `EPERM` 排除，以及两项此前等待 daemon
  socket 的 startup 排除；后两项的诊断被原 fixture 隐藏，未宣称直接证实其
  错误来源。具体列表见 [typed-smb-baseline.md](typed-smb-baseline.md) 和
  [typed-path-consumers.md](typed-path-consumers.md)。没有修改测试默认启用状态、
  权限、socket 行为或 CI；这些八项仍需正常 CI 完整执行。

三轮 release 局部对照均通过；release 全 library 行为检查为 449 通过 /
4 ignored，仅排除同样六项已知 Unix bind 环境限制。
