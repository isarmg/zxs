# 多级备份盘的真实只读导航验收

本测试切片基于已冻结的 `b53e06032e10b02b71743cd2cc7257bc030f7590`，tree `f8b65f60428a1e494cb1dc0a397cc1b793f1ba3c`，与公开候选 `78848330282b3bfc9ad0e0bb97aa940e524a666a` 同树。它只增加浏览器验收、host 反例和 runner 接线，不改 Web 产品、后端、磁盘配置、持久格式或备份路径 writer。旧候选的 Chrome 通过不作为本新增场景的通过证据。

## 真实场景与断言

现有 runner 在自己新建的私有临时目录内初始化 P01/B01/C01 三层配置；`browser-fixtures` 使用三副本，写入两份真实版本并冷重开验证当前正文。这个 fixture 不接受用户池路径。新的 `backup_browse::run` 在原磁盘读取失败／显式重试场景之后、原主盘文件与标签场景之前执行：

1. 从真实 P01 根进入 `browser-fixtures`，通过原 entries 响应记录种子文件完整身份、条目与仓库内路径、当前节点和版本，然后点击“所有硬盘”，要求一次新磁盘列表读取。
2. 分别点击真实 B01、C01 行。每步点击服务器返回的目录条目，不在浏览器拼接或伪造盘名前缀。只能沿该夹具唯一的前缀链进入 `browser-fixtures`；前缀为空、歧义、超出预算或未到达时明确失败。
3. 每个页面将新请求的 slot/path、完整未分页响应与实际 DOM 行顺序、类型、面包屑对应。要求备份响应 `backup=true`、`writable=false`、无可写目录上下文、无目录分支管理范围，每个条目只读；真实目录写控件及选择框禁用，导航仍可用。
4. 点击种子文件进入详情。核实实际位置仍以所选 B01/C01 和服务器路径组成；新真实 history 响应与 DOM 节点都包含相同 file/repository/node/version，文件默认分支与写控件不可写。这里只读列表和历史元数据，不发正文请求。
5. 每次回到“所有硬盘”都要求新真实列表；最终重新进入 P01，核文件条目与初始完全一致，再恢复根列表。记录连续的根→目录→文件页面链，不能只凭同名行或一次详情读取标记导航成功。后续原 history/tags/Unknown 场景不改变基线。

请求观察器返回同一原始 `Response`，不修改状态、头、字节或生产状态。保存每次匹配读取的顺序、参数范围、原始响应文本、解析值、页面投影和最终截图；不记录认证头、请求凭据或任意查询参数。额外的同源 API mutation 或 content/download 请求会在发送前阻止并令场景失败；这只约束此浏览器测试未发送写入／正文请求，不能证明服务端拒权。真实 HTTP 错误、网络错误、解析错误及观测预算耗尽均保留并失败，清理不覆盖其他 fetch owner。

## 现行路径事实与未完成范围

当前 `src/server/src/engine_backend/file_manager/backup_namespace.rs::default_backup_path` 对未设置显式路径的所有备份层使用主盘前缀。因此本夹具静态推导的 B01、C01 路径均为 `P01/browser-fixtures/browser-smoke.txt`。浏览器必须记录实际响应，不能将这一路径说成三级已经实现 `B01/P01/...`。用户要求的逐级直接下一级盘名前缀及对应递归路径 writer 是独立后端工作，本切片没有实现、伪造或验收它。

同一 file/version 的元数据观察不证明 C01 本盘正文可读。现有 fixture 的冷重开全局 `open_content` 也不等于逐盘物理回读。本场景不新增下载、上传或正文编辑，不验证 SMB 目录隔离、离线盘正文可达性、外来盘可信发布、历史-only 文件、备份路径写入或崩溃恢复。相关独立验收与证据不能互相替代。

## 预算与验证入口

八个目录页面是此固定单前缀链夹具的导航预算，不是产品支持的最大层级。观察器至多记录 64 个磁盘／entries／history 请求；单响应最多 1 MiB，总响应记录最多 4 MiB。它不缓存到产品状态，不建立定时器，不扩大生产服务限额；全部作用于独立测试浏览器和临时池。

源码阶段验证命令：

- `node --test test/tools/browser-smoke/unit/backup_browse_probe_test.mjs`
- `python3 -m unittest discover -s test/ci -p test_backup_browse_probe.py -v`
- 固定 Rust 1.99.0 的 rustfmt，JS 语法与 `git diff --check`。

host 用例覆盖原 Request/Response 不变、401／网络错误、JSON／字节预算、越权请求未发出、清理与迟到读取、错误 slot/path/旧响应/重复读取、部分页面、可写备份上下文、错误 DOM、实际前缀选择、同名异身份、错误历史详情、缺少完整根导航和最终基线未恢复。它们验证测试本身，不是实际 Chrome/WASM、真实后端或物理磁盘通过。

本切片的完整 runner 编译、严格 Clippy、官方 static 和同 SHA 生产 WASM/Chrome 仍需单独排程执行。通过标准为同一个新候选实际运行新增 B01/C01 场景及原 disk retry、卡片、正常标签、Unknown 等回归，原始日志出现新增 PASS 且无意外网络请求。不能使用既有候选的 PASS 或 host 夹具代替。

## 观察断言的身份边界补充

独立审查发现首个测试切片 `89d2acec2f62257e9ab2cf62ab8f47a681e0745e` 的 host 正例省略了真实文件 wire 的部分必填字段，而所选文件断言只比较 file/repository/node/version/size。后继将 host 正例补为 `ListingEntryResponse` 与 `ContentDispositionResponse` 的完整当前正文文件形状，包括 location、content ownership 和 read-only 字段；主盘根与仓库页的 context 也分开表示。

针对这个当前文件夹具，观察器额外核对 entry_id、repository_path、container=true、主盘可写／备份只读，以及备份 historical_only=false；历史读取还须对应同一仓库内路径。虚拟备份前缀目录必须有条目身份且不携带仓库／文件位置上下文。旧断言错误接纳另一条目身份和无身份前缀的反例已在 host 中复现，修前日志保留。

这些是本场景的身份与权限观察断言，不是重新实现完整 wire DTO 解析器。完整字段类型、未知字段、内容归属关系和所有历史-only 变体仍由生产 `files/page/entry.rs`、`files/page/ingress.rs` 校验并由其测试承担。本次 host 反例不证明旧产品或公开 788 的浏览器失败。
