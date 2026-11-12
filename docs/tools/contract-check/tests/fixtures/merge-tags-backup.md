# 合并、标签与备份响应 fixture

这十一份 JSON 是受版本管理的契约测试输入，不是 `test/evidence/` 下可缺省的本地验收附件。它们来自真实服务端 HTTP 测试，保留原始响应字段、ID、时间及整数编码，未用手写的空数组或最小样本替换。更新固定输入时必须重新执行对应 `http_engine` 测试，不得用旧生成结果替代当前验收。

来源是 [HTTP 测试入口](../../../../../test/server/integration/http_engine.rs) 的 `capture_wire`：

- [file_metadata_operations.rs](../../../../../test/server/integration/http_engine/file_metadata_operations.rs)：旧 `file-merge-tag-history.json` 记录 content、rename、tags、merge 节点及多父合并关系，现用于拒绝旧节点标签合同；`scoped-backup-configuration.json` 覆盖文件继承目录策略、scope identity 和 generation。
- [backup_namespace.rs](../../../../../test/server/integration/http_engine/backup_namespace.rs)：`backup-prefix-configuration.json` 覆盖目录前缀；`backup-prefix-files.json` 覆盖主文件改名后的固定备份路径；`backup-prefix-retained.json` 覆盖删除后的历史副本及 nullable 字段；`backup-prefix-administrator.json` 覆盖唯一管理员的完整只读列表；`backup-mixed-routes.json` 与 `backup-route-disks.json` 覆盖历史/当前路由和唯一管理员可见的全部相关磁盘。

旧 `file-merge-tag-history.json` 的原始捕获字节现保留为新格式拒绝证据：当前 OpenAPI 和共享 HistoryWindowPage 必须拒绝其节点 tags 字段与 tags 种类。它不再是当前格式正向样例，不手工清理后冒充新捕获。其余七份备份响应仍作为正向样例。

新 `file-container-tags-{history,snapshot,operation}.json` 来自 #1300 的准确
`f3dd9403ccaf62302a93b8ae13846f1072733f33` HTTP 测试，通过后从 `wire/` 原字节取回。
它们覆盖无节点标签的五节点六边历史、容器 GET 集合与独立代数、永久 CAS 完整请求回执。
对应契约回归同时使用严格 OpenAPI/共享 DTO，并绑定三份输入的文件、当前上下文及代数。

独立 `cargo test -p contract-check` 直接加载这些文件，逐份使用对应 OpenAPI 响应 schema 按上述接受/拒绝预期校验。它不要求启动服务或预先生成 evidence。运行时业务语义仍由上述服务端测试验证，固定 fixture 的 schema 校验不能代替服务端测试。

## 更新方法

在完整生产候选的同一 SHA 上执行 GitHub 定向 HTTP 测试，并显式把
`ZXS_TEST_WIRE_EVIDENCE_DIR` 指向随该 run 上传的隔离附件目录。没有此环境变量时
`capture_wire` 不会生成真实输入，不能因测试通过就宣称已经捕获。

从该 run 的工件复制明确命名的当前响应；旧 `file-merge-tag-history.json` 不覆盖。
新格式历史与标签 GET/CAS 使用独立文件名，新增对应 schema/共享 DTO 正向断言，
并由 GitHub 定向契约测试复验。备份的七份现有捕获保留原场景和完整字段。
本任务不在本地运行编译、测试或捕获，也不以扩大整个测试套件代替定向验证。

只有 HTTP 测试通过后才更新；检查完整 diff 与场景覆盖，不要把 schema 失败通过删除字段、清空集合或跳过缺失 fixture 掩盖掉。每次运行产生新的 ID、时间和磁盘可用空间属于预期，测试不要求它们与另一轮运行相同。

这三份新真实捕获已经取回，离线契约复验仍待后继 GitHub 定向执行。#1300 的相关 HTTP 单项通过，但整体失败且 contract-check 被原备份超时阻断，不能称离线复验已通过。准确 run/head、原工件及文件 SHA-256 见[契约验证边界](../../../../validation/container-tags-contracts.md)。旧 file-merge-tag-history.json 不覆盖。
