# 完整终态链的真实首笔 Publication

[首笔原生输入](terminal-chain-first-native-inputs.md) · [原目标决定](terminal-chain-target-decision.md) · [辅助工具](../components/helper.md) · [任务清单](../../TASKS.md)

基于真实父 `50ba0431`。此前只保存确定的 native Candidate；本片增加原目标的实际单槽 ACCEPT／FINALIZE／Publication 日志、真实 seal、两根证据和原请求冷续作。**它仍未接纳活动 Current、writer、locator 或新 I/O epoch，不消费原 Stop、不启动服务，也不等于终态链 readmission 或 H03 完成。**

没有运行审查、编译、测试、静态检查、浏览器验收或执行／查询 CI；仅格式化明确修改的 Rust 文件。没有执行辅助工具或操作实际系统／服务／磁盘。新增测试仅是未运行源码。

## 严格原始前置

`TerminalManagement::publish_chain_readmission_first` 只消费此前原 actor/operation/proposal/target 的完整来源选择、实际 `TerminalChainReadmission` vote 和双根完整首笔输入。来源和目标各自重放完整控制归档与终态链，并读回准确全 State/registry/history/Candidate。目标仍要求原停止目录、物理身份、原 seal、旧 I/O lifetime 已闭合、真实目录锁及 `.writer.lock`。

没有完整 first-input prepared 证据就拒绝，不隐式补齐输入或生成新提案。Detached、外来根、缺低级 Enabled 链、错误 actor/编号、目录替换和缺损材料仍不能进入。初始 Publication 配方在 SYSTEM 的 `chain-publication-selected.json` 保存后才允许创建目标日志；如果目标已有日志／完成记录却没有原 SYSTEM 选择，拒绝重新选择。

## 受限真实原生日志

`FirstSlotOwner` 只能由上述原生路径构造，生命周期借用 SYSTEM/目标 owner、实际选择和完整 first inputs。它向专属 `TerminalChainFirstJournal` 提供准确私有 `JournalIo`，不暴露 File、DiskIoBinding 或通用任意根写入回调。每次追加前后再核原 owner/物理/seal 和 source selection。

日志位于所选目标 `metadata/membership-pending/<selection>/first-candidate/first-publication/journal.bin`。沿用原生日志帧、hash 链、实际磁盘/slot Receipt、原子打开和 fsync 屏障，只允许同一 Candidate 的三个顺序帧：

1. 真实 ACCEPT 返回该目标实际第一帧 receipt，不用计划 receipt。
2. 用真实 ACCEPT 形成唯一 Finalize，完整对象在 SYSTEM 和原目标均不可覆盖保存、实际读回后才追加 FINALIZE。
3. 用真实 FINALIZE、准确 prescribed result 和空的新正文义务组成 Publication。两根对象与完整首笔输入再次核对后，追加真实 Publication；重新打开目标日志，核对实际第三帧与其返回 receipt，再产生原生 PublicationSeal。

独立 `verify_publication_seal` 核完整三帧链、全成员集合、sequence、Candidate/请求/结果；此外要求准确原盘和 slot、单一投票成员和首帧零 predecessor。这里确有真实 native Publication，而不再只是快照里预期 Published 或一个自填的成功 JSON。

seal、Finalize、Publication 对象保存在两根原隔离 bundle；目标 `published.json` 完成读回后，SYSTEM 才保存 `chain-publication.json`。来源回执记录实际证据，不将该 OperationResult 插入当前终态管理 State。普通 PortableAuthority 仍拒绝此独立 core，活动 generation 标记／日志／Current／数据库别名均不改变。

## 中断与丢失不是同一件事

受限日志最多 16 KiB、三个实际帧，只接受准确 ACCEPT/FINALIZE/Publication 前缀。不接受后续槽或第四帧，三个完整帧之后有额外 torn tail 也拒绝。校验和、链或对象选择变化保留并拒绝。

在同一原请求中，只有尚未完成的已知下一帧 torn tail 可按原配方修复。已有较早完整帧只重新同步，不截断后续不确定尾部。失败使当前 writer 失效，必须从原 native 状态重新打开。某帧已经开始后，其先写入的完整 Finalize/Publication 对象若丢失，不从记录摘要重新造文件。

目标已经有完成记录时只重验现有原生对象、seal 和真实日志，不重做发布。SYSTEM 已有完成记录但目标记录或日志丢失，明确拒绝，不重建。目标已完成而 SYSTEM 回执尚未写完，可在完整目标和两根对象再次读取后补同一个源回执。两根回执异内容拒绝。

- `first-publication-pending`：原请求已选，真实目标可能已有 0–3 个帧，不能推断“尚未发生发布”。
- `first-publication-preserved`：SYSTEM 保存了原目标真实 Publication 证据；不等于运行态已接纳。
- source-only 查询重验 SYSTEM 完整输入、对象和 seal，不打开目标；`target_reverified_this_call:false`。目标失联仍可显示历史事实，但不称其现在健康或同步完成。
- 显式原发布成功／重验才返回目标 reverified=true。`active_writer_admitted`、`active_current_admitted`、`metadata_resumption_supported`、helper `completed_readmission` 与 `service_started` 仍为 false。

Publication 开始后首笔输入安装接口关闭，不能用准备入口重建已依赖的材料。原请求没有取消或换盘 API。

## 独立控制台与预算

新增 `POST /api/disks/terminal-chain-first-publish`，有独立 checkbox、确认弹窗与固定确认文字，清楚说明是实际不可撤销首笔发布、尚不能恢复服务。仅原查询返回的计划可被尝试，后端仍重新核原 actor/主凭据 revision/host/account/layout。请求不接受 caller Publication/seal/journal/State/Candidate 或 force/root/外来链。

保持手动 root、0.0.0.0、无登录、不自启；原 Origin/CSRF/IP Host、单任务 owner、停止确认与退出排空不变。网络无认证 root 风险提示保留。报错展示原编号／提案摘要，并要求只恢复原请求，不能把连接中断当作未发布。

每个证据对象最多 128 KiB；两根独立容量预留六倍证据上限及 2 MiB 目录／日志余量。完整输入仍按上一片 8,192 对象／128 MiB 集合和原控制归档预算，实际验证成本随完整 State/历史/终态链增长。没有性能或 fsync 次数测量。

五项新增实际临时根／三帧／原请求冷态／已知 torn 第三帧／完成后丢失／前置拒绝测试源码，及一项 HTTP 严格输入、既有 Origin/CSRF/忙碌/关闭边界扩展均未运行。

仍缺从完整 native core 与真实 Publication 到专用活动代际的认证转换、精确 Stop 消费与新 epoch、Current/serving locator、运行数据库绑定和实际 target writer。外来完整低级链 H03 也没有以此绕过最终权威证明；全部保持拒绝，不能把一个真正首笔 Publication 概括成整个重新接纳完成。

后继[冻结 Current 消费者](terminal-chain-published-current.md)已接实际三帧/两根完整输入的只读准入与独立控制台检查，不改变本片对活动 writer、新 epoch、Stop 消费和正式 readmission 的拒绝边界。
