# 发布固定向量：对象身份与逻辑条目分离

`test/engine/unit/journal/fixtures/canonical_publication.json` 是真实 Engine
发布捕获的回归向量，不是独立编码器证明。生产生成器调用生产编码器及读取器；
将其输出冻结后比较只能证明相对该向量的字节／哈希保持，不能证明生成器本身正确。
此前测试注释所称“独立 JSON/BLAKE3 工具变换”不适用于当前生成入口，已移除。

## 保存与恢复边界

- `state_bytes` 固定每个 Commit 的实际 namespace 根；`namespace_objects`
  以 `kind/hash` 保存所有这些根可达的不可变 namespace 页／索引的规范 UTF-8 字节。
  完整再生成器从实际发布成员读取并认证闭包，不从条目重新打包；
  既有向量的一次性补齐规则见下。
- `commit_namespaces` 另外保存完整路径排序的逻辑条目，用作逻辑内容对照，不用于
  生成对象或推导根。全量构建的贪婪分区与局部编辑留下的分区可能不同；逻辑条目
  相等不要求根哈希相等，同一页／索引内容的编码仍必须确定。
- 根 `test/engine/unit/canonical_publication_fixture.rs` 是共用的测试加载器。
  它认证地址与字节，检查规范编码与节点形状，再仅用本向量对象检查完整准入和
  全部逻辑条目；不借共享 fixture store 补全缺页，避免其他测试掩盖不完整闭包。
  通过后才把原对象注册到内存并安装到指定成员，历史和快照测试使用相同入口。
- `recorded_namespace_objects_keep_non_bulk_page_boundaries_and_exact_disk_bytes`
  用显式 65+65 分区与贪婪 128+2 分区的同一组 130 条目，检查根不同但内容相同，
  且恢复到磁盘的每个对象字节与记录相同。它检验加载器不重新分区，不能替代
  namespace 编辑器的独立平铺操作序列 oracle、局部读／编码预算或端到端验收。

## 生成与验证

既有向量缺少 `namespace_objects`，优先做一次性补齐，保留已有随机 ID、时间、
`state_bytes`、snapshot/history 字节与全部原哈希，不因增加对象表而重录整池。
只在授权 GitHub 验证分支，用仓库固定 Rust 1.99 工具链运行精确忽略测试：

```sh
cargo test -p storage-engine --lib journal::snapshot_partition_tests::enrich_canonical_publication_fixture_namespace_objects -- --exact --ignored --nocapture
```

该工具仅此一次从旧逻辑条目构建对象，并断言**每个**完整 `CommitNamespace`
与既有记录相等，之后只新增 `namespace_objects` 字段；写入前用新加载器验证
闭包和逻辑内容。若宽目录的历史分区无法重现，则明确失败且不写文件，不能偷偷
重写原根。补齐对象的来源是“按已记录根身份验证的重现”，不是从原池提取；
哈希相等建立原对象身份，不将这一过程描述为独立编码证明。正常加载器永远不
执行条目重建，不用缺省空表或兼容回退绕过缺少对象的问题。

另保留完整真实发布捕获工具，仅用于明确的未来格式变更，它会产生新的 ID／时间：

```sh
cargo test -p storage-engine --lib journal::snapshot_partition_tests::regenerate_canonical_publication_fixture -- --exact --ignored --nocapture
```

保留生成工作流的准确提交、run URL、输出文件及其审阅记录。产物变更仅应是
目标 fixture；补齐时进一步核对删除新增字段后与原 JSON 值完全相同。生成后
必须在该产物上重新编译并运行 canonical 固定向量测试、namespace 加载器回归、
history-package 增量／独立 oracle 和 snapshot 增量 oracle。

本源码切片只做静态检查与固定工具链 rustfmt；没有在本地执行 Cargo、测试、
仓库脚本或生成器。GitHub 生成和最终验证结果须另附，不能把源码审阅记为通过。
