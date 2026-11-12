# 文件容器生命周期记录：真实来源读取

## 当前范围与格式决定

最初切片把永久收据中的删除、撤销删除和“导入已删除状态”接到文件详情页。
当前候选新增同一文件容器真实改名／移动的 PathChanged，不再制造路径内容节点。
这仍不是完整操作时间线，标签与 Merge 等其他操作不被猜测或补造。
新事实、复制保留、FILE 投影和整体格式断代以
[路径事实 ADR](../adr/container-path-facts.md) 为准。新格式拒绝旧池、旧备份和旧路径节点，
不自动迁移、删除或覆盖旧字节。下文的历史 CI 记录不代表本候选已通过。

成员身份是 `(operation_id, source_id)`，source 是真实 fdel 或 fpath。
删除和恢复可共用删除记录 ID，但保持各自操作身份；一个批次也可有多个不同路径事实。
界面卡片 ID 同时包含 file、operation 和 source，不把它当版本或写目标。

## 真实数据流与来源边界

`GET /api/file-manager/files/{file}/history/lifecycle` 使用独立
`storage_contract::file_lifecycle::{Request, Page, Event, Detail, SourceId, Cursor}`。
已有认证提取器和有界读取槽不变。EngineBackend 在当前凭据持锁检查后捕获只读状态，
锁外读取，再持锁复验凭据、历史根和观察状态序号。响应编码继续持有读取许可。

Engine 从 `state_projection::lifecycles::LifecycleSource` 获取目标文件永久收据成员，
经删除保留来源与 `persist::path_changes` 的严格准入链读取事实。
删除来源扫描复用 FILE 投影的原收据归属分类，不重建无关版本／目录索引。
Snapshot 与页面另共用完整永久账本准入，绑定 operation 表键、序号、主体与本地事实，
并解码每个复制／移动的必需保留封套；无关文件的坏 Graph 封套也不能被绕过。
这证明最终 State 的封套与身份，不从最终 State 倒推全部历史变更；实际前后变更仍由
写入和冷恢复的精确候选比较证明。
普通 MetadataView 不携带永久操作，私有读取快照仅额外保留所需收据和主体表。
响应不含删除路径、作者或完整分支前沿；PathChanged 的 nested detail 则包含真实
before／after repository 和 PortablePath，不依赖当前路径重建过去位置。

删除／恢复审计包括本文件事实和真实复制封套里捕获的扁平前缀，不追逐当前可变源文件。
新 Graph copy 同样写入必需保留封套；缺字段或无法证明来源返回 PublicationInvalid，
不把未知前缀显示成“没有记录”。当前 D 缺对应永久事件也失败。
复制的原路径事实保存在 SYSTEM 封套及 FILE 的无路径来源关联中，但当前页面只显示
本容器直接 PathChanged，不显示复制来的路径事实。
普通 namespace Copy／Separate 只派生当前正文新容器，不宣称复制完整历史。
无法完整映射新审计的跨池 foreign admission 明确拒绝并保全来源，不静默丢弃事实。

Delete 使用原 D created_at；PathChanged 使用永久事实实际记录的 Unix 整秒时间。
Undo／Import 没有独立时间，明确返回 null，界面显示“时间未记录”，不借用旧 D、
浏览器时钟或序号。日历无法表示的合法时间保留精确 Unix 秒。
Import 不伪造一对恢复／删除，记录序号也不被称为跨来源时钟顺序。

## 身份、分页与失效

请求必须提供当前 history_root；repository/path 成对解析，沿用真实历史上下文规则。
游标为 `binding:operation:sequence:source` 四段，绑定 file、root、实际上下文、请求位置、
页大小、观察序号和最后完整成员。旧三段游标与旧 flat 事件明确拒绝。
服务端必须在完整来源中找到精确锚点；响应拒绝重复 `(operation, source)`、逆序、
cursor 成员重放、越界序号、跨身份页、缺失可空时间及错误 next。
同一 operation 的不同 source 可跨页，不能只用 operation 作 UI key。

历史图根不证明永久操作表。任意发布使 State.sequence 变化后拒绝继续旧页，需刷新第一页；
这不是旧根租约，浏览不取得写权限。
Web 目标包括 session epoch、navigation generation、完整 Entry、file/root 和解析上下文。
回包先处理原会话失效，再核当前目标和面板代数；换目标、关闭和卸载中止并隔离迟到完成。
每页替换前页，不无限累积假完整时间轴。原错误不变成空成功／末页，不改变其他写依据。

## 成本与容量

- 请求 1–64 条，界面固定 16 条；编码响应上限 64 KiB。字节上限允许非空短续页，
  next 必须是最后完整成员；单条超限返回 HTTP 429 ResourceLimitExceeded，不截断路径。
- 删除与路径保留事实共享 65,536 成员上限，复制封套最大 12 MiB；响应不带 D 前沿。
- 每页仍扫描全部永久收据，解码各复制／移动的完整必需封套，加上目标删除／恢复前缀。
  完整来源准入先于取页，不仅检查廉价 ownership；单收据／封套仍完整解码。
  分页约束返回量，不保证 O(page size)、统一 CPU 上限或实测低延迟。
- 收据之间和事件循环检查取消，单收据成员遍历／解码仍只能前后取消。
- 复用原有界读取槽，无后台全量预取或新缓存；FILE／备份／复制的 ProjectionSnapshot
  仍保留完整成员和调用内移动收据解码复用。

此前仅收口读取成本的版本不改持久协议；本次候选确实改变 DTO、必需收据和格式门。
[删除图脱钩](shared-file-history.md#删除事实与内容图脱钩)已使 D 不再作为图节点；
本批改名／同身份移动仍需最终验证，其他非正文操作也尚未全面收口。

## 当前候选验证要求

本地只读写代码和静审，未运行编译、测试、formatter 或选择器。
最终准确 head 必须在 GitHub 定向验证：写源、Current 与冷回放完整证明、FILE、复制、
独立文件工作区改名后继续保存、显式多别名围栏、HTTP 分页／精确重试、Web native/WASM，
以及严格 schema 和 typed 消费者。原共享选择、每片预算和 900 秒备份事务不减少或放宽。
真实 Chrome、65k／12MiB 耗时与分配、完整操作时间线及未选恢复矩阵不由所选通过代替。

## 历史已执行证据

以下仅记录早先删除／恢复及来源优化切片，不回填为新路径事实候选通过。

已发布三个独立提交：`2cd2d69256fc2a601a82f7e9b68bd359a9248ece`（策略）、
`2a8414a6d27f9ee76922f12d169f156ac2875ec7`（contract/Engine）、
`0df1724a93d2b20c59a2a6b7299b6e500fc5e4b1`（HTTP/Web）。策略 #1281 已通过
123 条 Python 回归，未选 Rust 或编译项。contract/Engine #1282 通过 463 条 Rust、9 条 Python
和 4 项编译检查；仅新增 Import 夹具错误地以新路径寻找原删除事实，未运行到该用例的 endpoint 断言。
修复 `d01f670b308dbf94def41efe0ded4fed20c700ca` 按永久回执里的导入 D 身份校验，
保留原路径事实、原删除 D 与导入 D 的分别映射、独立操作 ID 和未知时间断言。#1287 在该准确提交
通过 23 条 Engine Rust 及 1 项 Engine 无默认特性产品编译，未选 Python，failed/skipped/blocked
均为空。真实 Import 移动后读取和冷重开、retained copy 的普通/当前注册表模式、graph-only 拒绝、
直接删除/恢复与共享来源回归均有执行证据。

HTTP/Web #1283 工件中 95 条 Rust、11 条 Python、4 项其他编译通过，其中包含真实生命周期 HTTP
删除/撤销、认证、注销及冷重开；Web 测试构建、native/WASM 检查被同一处未导出的 `h4` 阻断。
`ff23c0c7d6a1fa2143dc81954ae73e668f06db4a` 仅改为仓内已有的 `p role=heading` 与
`aria-level=4` 方式。#1285 在该准确提交通过 21 条 Rust（9 条生命周期、12 条详情模型）
和 Web native/WASM 两项产品编译，工件的 failed/skipped/blocked 均为空。
不把 #1283 中因编译失败而未执行的其他 Web 选择计作已通过。

准确工件：[contract/Engine 首轮](https://github.com/isarmg/zxs/actions/runs/43189053164/artifacts/13932541854)、
[HTTP/Web 首轮](https://github.com/isarmg/zxs/actions/runs/43189640771/artifacts/13931127877)、
[Web 修复](https://github.com/isarmg/zxs/actions/runs/43193731480/artifacts/13932730608)、
[Engine 夹具修复](https://github.com/isarmg/zxs/actions/runs/43196414519/artifacts/13932754692)。

后续收据来源候选新增扫描途中取消、无关坏收据拒绝及 Graph-only 与坏收据并存时的原错误优先，
并在真实初始、删除、恢复、冷重开四阶段与完整投影做差分。这项差分证明成员来源一致，
两入口共用事件解码核心，不将它当独立解码参考实现。定向范围保留原 FILE、批量恢复、
retained copy / Import、HTTP 授权和 Web 生命周期测试，另选现有移动收据复用的 5 项回归。
该来源片已在 `24a18efc4adc50c9a1c432032dd5df093154863f` 上通过
[#1309](https://github.com/isarmg/zxs/actions/runs/43246723472)，比较基线为 `9af37ef4`：
54 项 Rust、147 项 Python 与 4 项编译检查通过，failed/skipped/blocked 均为空，
完整选测聚合成功。包含生命周期 3、FILE 15、批量 undo 7、移动收据复用 5、copy/Import 2、
真实 HTTP 1、Web 生命周期 9 与详情 12；编译检查包含 Engine 无默认特性、server、
Web native/WASM。结果工件 `13957672148`，聚合工件 `13957018983`。
这些是该准确来源片的行为证据，不是大来源性能或后续删除图脱钩候选的通过证明。

这些准确提交的历史证据不覆盖后继新路径持久格式；本候选按当前 ADR 独立验证。
