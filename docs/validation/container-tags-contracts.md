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

## 新合同

GET 必需明确 repository_id/path，响应包含 file_id、context、tags、generation。
POST 提交完整替换集合、独立 expected_generation 和原 operation_id；响应结果为
`FileContainerTagsReplacement`，完整 request、结果集合、changed 与代数必须一致。
跨请求身份与 CAS 由真实消费者验证，离线向量只验证局部一致性和共享 DTO。

新反例包括重复、空白、控制字符、16 项和 128 UTF-8 字节边界、缺失字段、非法代数、
结果集合/代数不一致、旧节点响应。Snapshot/PagedHistoryRoot 必需 container_tags 根，
format 是 `container-tags-radix-map-roots`；原格式及缺根输入明确拒绝，不改原池/备份字节。

## 尚未执行

本轮只静态编辑和审阅，未本地编译、测试、格式化或运行验证脚本。

当前缺少新格式的真实正向 history/tags HTTP 捕获。完整生产候选在 GitHub 定向执行
`http_engine` 后，应从同一 SHA 的 `capture_wire` 工件取回新历史及标签 GET/CAS 响应，
新增或更新明确标注的新格式 fixture，再执行 OpenAPI/共享 DTO 与真实冷重开回归。
旧捕获继续保留为拒绝证据，不作为缺新捕获时的兼容出口。静态合成向量不替代此验收。
