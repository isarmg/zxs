# 容器标签契约消费者与证据边界

本次是新格式切换，不保留节点标签兼容、自动转换或迁移。文件容器以稳定 FileId
持有一个标签集合及独立代数。HTTP GET/CAS、共享 Rust DTO、OpenAPI、核心 schema、
Web 和契约工具必须同批接通；历史节点不再携带 tags，也不接受 kind=tags。

## 固定输入的性质

- `docs/contracts/vectors.json` 是合成契约向量，不是实际持久化收据。
  原有历史窗口的拓扑、分页、字段、预算和错误场景保留；其中旧标签种类的合成元数据
  节点改为合法 merge 种类，仅用于 DTO 形状测试，不表示真实旧节点可转换或迁移。
- `test/tools/contract-check/fixtures/history-window/` 是合成窗口 DTO 输入，包含固定根、
  cursor 和预算，不是新的 HTTP 捕获。节点标签字段退役后，保留所有节点和关系场景。
- `test/tools/contract-check/fixtures/merge-tags-backup/file-merge-tag-history.json`
  是旧服务实际捕获，原字节保留，现由 OpenAPI 和共享 HistoryWindowPage 严格解码明确拒绝。
  不能擦掉其旧字段、重命名种类或清空集合后冒充当前服务的真实响应。
- 其余七份备份固定捕获继续作为原场景的正向 schema 输入，未更改其字节。
- `file-container-tags-history.json`、`file-container-tags-snapshot.json` 与
  `file-container-tags-operation.json` 是新格式真实 HTTP 原字节，来源见下文。

## 新合同

GET 必需明确 repository_id/path，响应包含 file_id、context、tags、generation。
POST 提交完整替换集合、独立 expected_generation 和原 operation_id；响应结果为
`FileContainerTagsReplacement`，完整 request、结果集合、changed 与代数必须一致。
跨请求身份与 CAS 由真实消费者验证，离线向量只验证局部一致性和共享 DTO。

新反例包括重复、空白、控制字符、16 项和 128 UTF-8 字节边界、缺失字段、非法代数、
结果集合/代数不一致、旧节点响应。Snapshot/PagedHistoryRoot 必需 container_tags 根，
format 是 `container-tags-radix-map-roots`；原格式及缺根输入明确拒绝，不改原池/备份字节。

## 真实 HTTP 捕获

三份新正向输入取自 [GitHub run #1300](https://github.com/isarmg/zxs/actions/runs/43219203395)
的 `targeted-tests-0-f3dd9403ccaf62302a93b8ae13846f1072733f33` 工件（13947791171）。
准确 head 为 `f3dd9403ccaf62302a93b8ae13846f1072733f33`，完整基线为
`39bcae218116b2d8f925120d9a8f34cea791df5a`。原 ZIP SHA-256 为
`d6099dfbc5dd63a953336fac5527ae7de57505202621b4c0121400502f0260cd`。

该工件记录 `http_engine` 的
`file_metadata_operations::tags_and_file_merges_publish_reviewed_node_graphs_without_new_bodies`
真实通过，包含历史、标签 GET、永久结果与冷重开断言。捕获写在冷重开之前，
单项整体通过证明后续冷重开断言也已完成。三个 `wire/` 文件逐字复制，
不改字段、ID、空白或编码：

| 文件 | 字节数 | SHA-256 |
| --- | ---: | --- |
| file-container-tags-history.json | 6238 | `49a2df224e2a1c3b403228c3fa1e0655d66993bc70c98f36b2286bbb5653c325` |
| file-container-tags-snapshot.json | 232 | `f1ca3b4a75815d65b3a018c0ba3a532ffed21a785ef685723b450f7d049c41ca` |
| file-container-tags-operation.json | 766 | `e9f706df311de4d338849c3ae8382db268c0005ccb2b5d01648ee8d48e624059` |

新增正向契约断言使用对应 OpenAPI GET/POST 响应 schema、共享严格 DTO，并核对三份
响应的 FileId、repository/path、集合和代数；历史为五节点六边，标签为容器当前集合，
不是节点字段。该固定输入绑定只能证明这组捕获的一致性，不替代运行时原请求校验。
旧 `file-merge-tag-history.json` 的原字节 SHA-256 仍为
`21cf9e64a0d677cffc4398500fc965c9f7bfd59272913e3a37ad455bcd6246d6`，继续明确拒绝。

## 后继契约消费者验证

[#1304](https://github.com/isarmg/zxs/actions/runs/43227554097) 在准确提交
`09b7715f3d9aa2f85820fd8dc561cc1a800936bc`、同一完整基线 `39bcae2` 的分片 2
实际通过全部 13 项所选 contract-check 测试，包含三份真实捕获的严格 DTO、OpenAPI、
跨响应身份/集合/代数绑定、旧格式拒绝和文档 schema 一致性。工件为 13948639188，
ZIP SHA-256 为 `4958c7bb6e3c139e97df66041db1dde13edca59a4c10bc1558acfa7ba3e21390`。
这项复验未修改三份真实输入或旧拒绝输入的字节。

## 未完成的验证

#1300 整体失败，不是完整标签验收通过。原备份事务 900 秒超时后，Web 原生/WASM、
browser Rust 和 contract-check 均列为 blocked；该 run 没有执行新固定输入的离线复验。
上述 HTTP 单项通过不覆盖其余失败、完整恢复或真实 Chrome。

#1304 的 contract-check 通过不等于整批通过：同分片仍有旧 Web 动作清单、contract
单元测试编译、复制夹具及 SMB 模块选择失败；其他分片的原备份事务仍然超时。
Web 原生/WASM 编译已通过，543 项 Web 行为通过、1 项上述清单断言失败。
本地未编译、测试、格式化或运行验证脚本。静态合成向量不替代真实 HTTP 证据，
真实捕获和离线契约执行也不替代真实 Chrome、完整池恢复及其他未选择目标。
