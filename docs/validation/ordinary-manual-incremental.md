# 普通 Manual 文件的段保存

本切片在已有真实 FILE 版本的普通文件上复用现有 `NativeWorking` 段 RPC、
Engine extent 写入器和 SYSTEM/FILE 发布，不新增客户端协议或后台自动版本策略。
默认 Manual 的空文件仍走原有 Unborn 保存与显式首次提交。

## 数据与入口

- 已有真实版本的 `ManualClean` / `ManualIncremental` 文件由默认共享的
  `DefaultShare` / `Session` 进入原生段读写。打开不复制整个基线，返回的描述符仍是
  不可写入内容的目录占位句柄，段写和截断由已有有界请求完成。
- `ManualIncremental` 保留原 `DraftBase`、工作 revision、稳定 payload ID、可见长度与 manifest commitment，
  再附一个真实 immutable extent manifest 与 `retained_base_bytes`。
  `DraftPayload::Ranges` 不含正文 ContentHash，既有 Full 稿保留原来的 JSON 字段。
  未覆盖段读取那个准确的不可变版本；缩短后重新扩展的尾部读零，不能重新露出原基线。
  显式零写也是实际 replacement extent，不能省略后又回退到旧基线。
- extent 仍由原写入器独占创建、验证、同步并保留 inode、长度、hash 证据。
  只有原生发布成功才确认一次保存；连续保存仍以第一次准确的 DraftBase 为基线。
- 一次保存只写并验证本次触及的 64 KiB 物理块，不读取完整基线或合成稿计算正文 hash。
  打开只认证既有基线 checksum sidecar 与命名实体。读取基线时只校验命中的原 checksum
  chunk，读取增量时只校验命中的 extent，返回的就是该次校验的缓冲区。
  原 checksum chunk 可大于 extent；这是有界范围认证，不是性能计数或高频策略。
- HTTP Range 沿用原鉴权和区间响应，ETag 明示 `working-manifest-…`，不伪称正文 hash。
  仅实际按范围认证的 Engine reader 能进入这一流式分支；外部原始 FD 不能借 manifest 准入。
- 显式提交才完整读取合成稿，形成保留同一源的不可序列化 `ReadDraftDigest`；一次业务
  求值获得正文 hash/块摘要并与 exact base 比较，原 Version constructor 复用这份事实。
  相同正文只清稿，不分配新版本。不同正文沿用原 FULL 复制发布器，生成独立实字节文件。
  最终复制仍按原边界校验独立 FULL，不宣称只读一遍。冷恢复重放显式提交决定也仍需
  真实原稿的完整正文证明；普通 save/稿 reader capture 不做这个完整读取。
  manifest、稀疏段、hash 或假路径都不能充当新的不可变 Version。
- 现存 `ManualDirty` 完整稿的读回、后续完整稿保存和显式提交不迁移、不删除。
  原 HTTP 完整上传也仍可写入该表示。原生新完整替换使用同一 extent 写入器。
  Automatic、worktree、备份入口没有在本切片增加高频或自动全量行为。

首个可用范围仍是现有 Draft 发行范围：初始格式、根级普通文件、单 owner 副本与
准确选中的 FILE 分支。没有添加被拒绝旧格式的兼容路径、孤儿清理或长尾故障矩阵。

## 正常验收

新增精确测试：

`storage-server --lib` 的
`smb_rpc::tests::native_dispatch_tests::incremental_manual::ordinary_manual_ranges_and_tcp_commit_retain_exact_base_draft_and_version_bytes`

它复用生产 `DefaultShare` / `Session` 与真实 `EngineBackend`，不建立 AF_UNIX socket。
首次提交和最终显式提交通过真实 loopback TCP HTTP 登录、认证和 Router。
它依次断言：

1. 空池的真实 NativeWorking 保存保持 Unborn，显式 HTTP 首次提交得到真实 V1。
2. 打开 V1 不复制 extent；第二写者由真实路径 reservation 拒绝。
3. 跨块小写仅产生两个 extent，Flush 重试返回原收据，FILE 历史不增加。
4. 再次小写、显式零写和缩短后扩展保持原基线，旧读句柄固定在原保存。
5. 断连、Engine 冷开后，原 V1 与保存稿分别读回准确不同字节。
6. 稿持久身份没有 ContentHash；HTTP 跨 extent/基线/零尾的 Range 字节、长度和 manifest ETag 准确。
7. HTTP 显式提交及同请求重试得到准确 V2；V1 inode/字节不变，V2 是独立完整 inode，
   不是任何 extent 的 inode；版本数量由 1 增至 2，完整 hash 等于真实 V2 字节。
8. 再保存一次相同正文仍是无正文 hash 的新 range 稿；显式提交及重试清稿，冷开仍仅两版。

实际物理 extent 的名称 ID、device/inode、单链接、长度和完整 hash 都由该用例检查。
它不是 Unix 帧、SCM_RIGHTS、Samba daemon、浏览器操作或 Windows 客户端的执行证据。

## 精确命令与证据边界

以下命令可独立复现实测或扩充定向覆盖。主验收使用云端生产原生分派和真实 loopback TCP；
不依赖 Unix transport，不替代正式 Samba 客户端或大文件性能验收：

```sh
cargo test --locked --offline -p storage-server --lib --features core \
  smb_rpc::tests::native_dispatch_tests::incremental_manual::ordinary_manual_ranges_and_tcp_commit_retain_exact_base_draft_and_version_bytes \
  -- --exact --test-threads=1
cargo test --locked --offline -p storage-server --lib --features core \
  smb_rpc::tests::native_dispatch_tests::native_dispatch_uses_real_engine_and_cold_unborn_content_without_transport \
  -- --exact --test-threads=1
cargo test --locked --offline -p storage-contract --lib file_draft::tests:: -- --test-threads=1
cargo test --locked --offline -p storage-engine --lib --features fault-injection \
  business::file_draft::tests::whole_working_selection_checks_all_cas_before_open_and_retains_real_published_drafts \
  -- --exact --test-threads=1
cargo test --locked --offline -p storage-engine --lib --features fault-injection \
  business::file_draft::tests::born_working_batch_uses_one_prior_and_exact_sources_without_independent_clears \
  -- --exact --test-threads=1
cargo check --locked --offline -p storage-server --no-default-features --lib
cargo check --locked --offline -p web-app --lib --target wasm32-unknown-unknown
```

定向 CI 新组 `ordinary-manual-incremental` 执行两条准确 server 用例和六条小型 contract
状态测试；所有已有重叠组仍保留各自的测试清单、预算与超时，不以子集替代完整选择计划。
