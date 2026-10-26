# 正文摘要类型边界

## 范围与语义

从 `67c7d813` 开始的本批只处理完整文件正文的 BLAKE3 摘要。
`ContentHash` 私有持有 `[u8; 32]`，没有 Default、字符串 Deref 或可变字节访问；
未知调用方 `FromStr` 只接受 64 位小写十六进制，包含全零值。
`from_digest` 直接保留内部散列结果，不把内部生成值格式化后再解析。
它不证明当前文件仍具有这些字节；实际读回和复制必须继续校验正文。

真实调用链如下：

- HTTP `X-ZXS-Content-Hash` 在 header 边界解析；`UploadRecord.content_hash`
  的磁盘反序列化也严格解析。缺失、重复和畸形 header 仍拒绝为请求错误，
  畸形磁盘状态仍为 `UploadStateInvalid` 并保留源错误。
- transport 完整流散列、`UploadSource`、`prepare_upload_file` 的 expected
  commitment、完整复制、`StagedFile` 和 `FileVersion` 使用同一种正文类型。
- 普通读、工作区读、历史包复制、修复和离线导出比较定长值，底层完整性
  原语直接取 digest；不再 `from_hex` 自己持有的正文摘要。
- JSON/磁盘/canonical JSON 继续编码 64 位小写字符串；HTTP Content 的
  公开传输对象仍为字符串，在 EngineBackend 出口显式格式化。

`PreparedUpload.hash: Option<ContentHash>` 只在 execute 内拥有未完成复制的
清理 guard 时为 None；成功返回前均为 Some，字段私有。持久化 StagedFile
始终有摘要，未增加默认值、全零占位或旧格式迁移。

这不是 A1 的完整 model/persist/wire 分层整改；现有领域记录仍有 serde。
checksum manifest、canonical object/root、请求摘要、prefix persisted_hash
具有不同语义，仍使用原表示，未混入 ContentHash。client/header wire 与
其它协议 DTO 也未盲目统一。`validate_version` 只移除摘要文本格式重复检查，
存储名与摘要绑定、时间、身份、大小和正文验证继续保留；P6 全表扫描仍未改。

## 字节、复杂度与耐久性

BLAKE3 输入及域前缀未改。真实 canonical_publication fixture、snapshot
固定 root/hash oracle 均保持原文件；只有非法合成测试正文 hash 改为合法值。
新增 `abc` 固定 BLAKE3 向量、精确 JSON/canonical 字节对照，以及真实磁盘
解码拒绝并保留原证据的测试。新 HTTP 用例检查拒绝后无 spool 数据／收据，
随后合法请求仍能发布完整正文。

正文内存表示为 32 字节，不再需要 64 字节文本堆分配；未测端到端性能或 RSS，
不宣称吞吐改善。比较固定长度，边界输出仍需 hex 格式化。现有 I/O、锁、
fsync、独立完整副本、永久幂等收据与分支可读性协议没有变化；未使用 stat
cache 替代正文证明，没有新增生产测试 hook。

## 复现

```sh
cargo test --locked --offline -p storage-contract -p storage-engine
cargo test --locked --offline -p storage-server
cargo check --locked --offline -p storage-server --no-default-features
cargo clippy --locked --offline -p storage-contract -p storage-engine -p storage-server --all-targets -- -D warnings
```

执行时点结果保存在独立验收日志，分别记录通过、失败与未执行项。
本地复现使用固定工具链、jobs=2、debug=0、incremental=0 和独占 target。
真实 Samba SDK、浏览器、物理掉电不由这些单元测试替代。
