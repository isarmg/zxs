# 最后元数据成员退出后的显式接续

本说明描述 TerminalResume 的源码协议与验证目标，不表示测试已经通过。源码／测试由独立作者编写，构建、测试、提交和远端验证由整合方统一执行。

## 权限与边界

- 终态不是 N=0/Q=0 的活动池。旧 `ValidatedConfig`、固定 N/Q、全部旧期刊、永久操作收据、registry 历史身份和删除 occurrence mask 保留在认证控制闭包中。
- `VerifiedTerminalAuthority` 只能从已完成的 SYSTEM terminal authority 和完整 exact archive 验证得到。反序列化、一个 hash 或调用者提供的 Config 都不能构造该权限。
- `TerminalManagement` 不提供普通写入。仍登记的 data-only 目录可以保留。新 API 接收的是登记目录和已打开的目录描述符，不假设一目录等于一真实设备，允许同一文件系统上的不同目录。
- 恢复活动写服务必须至少有一个全新 metadata voter，才能形成非零的新固定 N/Q；这是协议条件，不是要求所有新目录保存元数据。可在同一请求中加入任意多个仅存数据的新目录（仍受配置和 API 数量上限约束），逐目录保留用户指定角色，不静默升级为 voter。新增 binding generation 为 1，不能复用历史 slot、逻辑 ID、路径或目录实体。所有旧定义保留，旧 Disabled/Detached 不得被顺带启用。
- 不擦除被退出目录，不写其恢复文件，不回读停止目录。独立控制归档不复制用户 payload。

## 持久步骤

1. 已完成 terminal 退出提供原固定 N/Q、原冻结 State、完整历史、独立旧 archive、TerminalRegistry 和不可伪造的认证 token。
2. `prepare_metadata_resume(self, target, principal, operation_id, directories)` 校验原设置、非空新组、历史身份／别名排斥和明确新目录 FD。先把 exact proposal 写入 SYSTEM `resume.json` 并完成文件／目录 barrier，再保留新目录 reservation 与 writer lock。
3. `synchronize` 先为全部 data-only 新目录持久化与普通 Add 相同的 pending seal、`membership-data-ready`，完成文件／目录同步及读回。证明精确绑定原请求、physical identity、目标配置角色和旧 archive；每个目录保留自己真实的目录 FD 与 writer lock，不靠布尔标志冒充 ready。
4. 每个新 metadata voter 独立安装并完整重读旧 archive，然后写自己的不可变 decision。它绑定新 intent、该成员身份、terminal authority、proposal 与旧 archive，不把别的目录或 SYSTEM 的副本当成本成员副本。全体 voter 决定构成 `ResumeCore`，每个 voter 独立持久化并读回完整证书。data-only 不写投票、控制 archive 或 journal，不计入 N/Q；但其派生 ready 证明由同一 immutable intent 的完整 added identity 集合绑定，证书安装前、激活时和冷恢复时均读取实际本地证据。此时仍无普通成功 OperationResult 或可写 Engine。
5. `activate` 复用既有 Generation/PortableAuthority、archive 安装、journal、publication 和 namespace admission 流程。SYSTEM 先写 `resume-generation.json`，再保存 `resume-bootstrap.json` 的精确旧 head/下一条记录计划，之后才扩大 bootstrap 地址、写活动 marker、seal 和第一笔真实 publication。
6. 原 terminal 永久结果和新 resume 永久结果同时保留在新 State 中，两个操作各占一个拓扑变化。新结果 kind 为 `terminal-metadata-resume`。它的 `current_health=Unknown`，不把控制发布当作旧 payload 健康证明。
7. 必须读回真实新固定 Q 的 publication、各成员自己期刊收据、seal、完整 snapshot，并核规定结果和 namespace admission，才能写 `resume-complete.json` 并交出 Engine。新 selector 永远不能授权 Genesis。

## 断电重入与 bootstrap

`metadata_resume_recovery(system_root)` 只从 SYSTEM 精确认证路由，返回无 pending、`ResupplyDirectories` 或 `CompleteGeneration`。前者重新提供相同原目录 FD 调 `reopen_metadata_resume`，后者调 `complete_metadata_resume`。HTTP phase、相似路径或不同 actor 不替代该路由或会话验证。

bootstrap `reload` 保持严格。新 `PreparedRootExtension` 的字段密封，可序列化但不是授权。普通 Add 的 helper 必须收到独立认证的实际 observations，重算 exact next；terminal 有单独入口，必须消费完成的 `VerifiedTerminalAuthority` 与新 `ResumeCore`，通用 Add 无法从 terminal ledger 扩根。

预备计划在 ledger 写入前持久化。若下一 immutable record 已持久而 head 仍指旧记录，重入仅可重放保存的那一条精确 successor，库存只可为 before/after。未知记录、缺历史、冲突字节和未知 atomic staging 一律保留并关闭，不通过删除或宽松 reload 猜选。已有后来合法 stop 的完整链可证明包含该 exact successor，但旧计划不得回滚 head 或清 stop。

terminal→活动 locator 记录全部新增目录的真实 metadata/data-only 角色，必须保留所有历史 stop、原 identity 和旧 locator 节点。新地址只是受限发现范围；真实成员与 publication 仍须另外验证。激活首先核验全部新增根的 decision/archive 或 data-ready，再开放受认证 ledger 允许的旧非目标目录。旧停止目标不重新探测。已完成启动先验证 SYSTEM selector，再限制探测集合，最后要求实际选中的 PortableAuthority 包含该 resume generation。

data-only 的半成品 ready 写入、缺少 marker、部分 marker／seal 晋升均使用原操作和原资源身份重入；未知 staging、损坏 ready 或被替换实体保留并拒绝。正常代际晋升继续写入数据目录实际正文证明所需的 config、generation authority 和后续 publication 依赖；本地正文验证／离线导出通过 `PortableAuthority::data_dependency` 校验对应 Resume 的 ready，不因它不投票而省略证明。后继 Add/Exit 继续携带这个来源。

## Namespace 与历史

恢复从原 archive 重放前代 current namespace，验证 terminal 自己的 occurrence mask，并把同一隐藏集合接到 resume registry。禁止直接用原 State 的 raw heads 重建可见目录，否则会复活已删除的独占项。旧 State、commits、versions、receipts 不因新成员加入而重写。

新组后续普通 Add/Exit 可携带 ResumeCore 及其完整 predecessor。控制归档的目录位置使用新 resume intent，归档内部内容仍使用原 terminal intent；两者不能混淆或伪造新旧 fence。

## 未发布请求的安全取消

fresh resume 在新的固定 Q 决定或 generation selector 之前，可以显式取消。取消必须取得同一不可变槽的 N−Q+1 个真实新 metadata Cancel 选择，保持原 actor/op/digest、全部历史证明及 data-only owner；不得删除或重写 pending 目录。完整协议、不可变后继 attempt、永久管理收据及真实资源交接见[未发布取消](terminal-resume-cancellation.md)。已有 generation selector 的取消请求只拒绝，不借冷恢复入口隐式激活原操作。

## 尚未闭合的功能与验证

- 已 Disabled 的相同 disk/slot/binding 作为终态第一组重新启用，尚无本批实现。普通 readmission 不能自动替代 terminal 认证流程。
- 在一次 resume 后再次退出最后成员时，terminal head 轮换必须验证当前 lineage、前一终态及接续的永久收据；先持久化新 authority 和 archive，再原子替换 head，保留旧证据。组合回归结果见根进度记录。
- 未声明支持损坏／丢失 authority、完整归档或未知 staging 的自动修复。
- 源测试覆盖删除与下线的可见性差异、完整旧历史／永久结果、新组固定 N/Q、同文件系统目录、两种冷恢复路由、每个真实 Record/Head barrier、独立新 archive 缺失、actor/op 改写和停止根不再接触。
- 混合组专用源测试增加逐目录角色／票数、真实锁持有、数据目录新正文与离线证明、data marker／seal 半晋升、root plan 的四个 barrier、ready 损坏先于旧非目标探测、staging 保留、缺 FD 与实体／角色重绑拒绝，以及 HTTP 冷恢复错误后继续原句柄和 Web 取消 metadata 勾选不被静默改回。
- helper 测试覆盖 exact plan 序列化、before/after 重入、普通 reload 严格性、历史 stop 保留、后来 head 不回滚、未知记录拒绝与通用 Add 不准解除 terminal。
- 本作者未执行测试、构建或程序，也未宣称物理断电测试、真实部署或功能验收完成。

## 复杂度与同步成本

独立 archive 校验／安装按真实完整旧控制历史和新 metadata voter 数计费；每个 voter 有独立文件与目录同步，不能以共享副本替代。每个 data-only 新目录增加一个有界 ready 文件、既有文件／父目录同步与读回，复用 Add 实现而不复制控制归档。精确依赖从已认证请求派生，不更改旧 Candidate、既有 Add 或 metadata-only Resume 的序列化字段／哈希。PortableAuthority 保留旧节点及 terminal 认证闭包，继续受既有 MAX_METADATA 限制，超限在开放活动写入前失败。bootstrap 校验流式读取不可变记录链，计划只增加一条记录，不压缩或 GC 历史。
