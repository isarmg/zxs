# 备份原生 generation 的选择身份与激活隔离

[Engine](../components/engine.md) · [任务表](../../TASKS.md) · [真实围栏归档](backup-generation-control-archive.md)

## 已有选择身份与本片范围

在现有 `PortableAuthority` 的原生 generation 解析中加入严格独立的 `Backup` core，格式为 `backup-registry-generation`。它携带原来全部成员实际选择的完整证书、原 `membership-control-archive` 根、未发布完整路由快照及相应输入摘要；使用既有 `membership-generation` 域编码和计算 identity，不把普通 Add/Abort、SYSTEM 进度或孤立的目标完成文件当作新 authority。

该 core 的外层及证书都有严格字段闭包。证书重新核原 transition、所有原成员及真实选择 receipt 的规范关联；新增冷态投影检查将完整源配置、被冻结的源和原末端、每个指定目标的身份/物理绑定/根名/逐级前缀与原任务逐项绑定。所有目标的完整文件和目录仓库地址转成相同的相对单位表，父子仓库及版本集合不分散到不同目标。完整旧 State 的序列、规范摘要及 native snapshot 根必须与 source fence 原出版边界相同。

这些是**选择身份与材料准入**，不是已经发布的新活动 generation。现已在隔离目标生成下述首笔 native Candidate；本片推进到全部原成员的真实 ACCEPT/FINALIZE，但仍不生成 Publication。类型中没有偷偷把原 Config 作为新的有效拓扑：`effective_config` 对 Backup 明确失败，原生 registry/Current 转换、I/O 限定和普通激活仍拒绝。即使把这种 core 放进普通 generation 标记，也不能用旧 voters/旧 Config 启用目标 writer。原 Add/Abort/Exit/Readmission 行为没有被映射为 Backup。

本片按指示没有审查、编译、测试、静态检查或 CI。仅对编辑的 Rust 文件执行格式化，不是编译或产品验收。

## 后台任务与原任务恢复

原 registry/placement 后台任务在全目标 `GenerationControlPrepared` 后继续到 `GenerationAuthorityPrepared`：

1. 从每个目标自己的完整 State、路由快照及真实 fenced control archive 重新推导原生 core；没有旧源、SYSTEM 或其他目标回退。
2. 所有目标分别得到相同的精确原生对象并满足本地空间预留后，才开始写入。
3. 在隔离目录 `backup-generation-authority/objects/membership-generation/<prefix>/<hash>.json` 无覆盖持久化。不同旧字节或未知文件拒绝并保留。合法中断临时名最多 1,024 个，不自动删除、晋升或计入证明。
4. 全目标实际读回并经过原生 `PortableAuthority` 严格解析，才分别写 `generation-authority-prepared.json`。原源选择再次观察成功后才更新 SYSTEM 中的诊断进度。

这个 authority 材料目录不是 `metadata/pool-membership/generation.json`；它本身没有写活动 disk seal、Journal 帧、Candidate、Publication 或成功 operation receipt，也没有赋值 Config/State/Current 或获取活动目标 writer。后续首槽定案会另外在原 metadata 成员的保留 generation 日志追加真实帧，范围见后文。

`BackupDecidedTargets::inspect_generation_authority()` 为只读全目标重验。显式 `resume_authority_preparation()` 仅无覆盖补写同一已决定任务的隔离 authority 材料，调用者须另有目标写入权限；它仍要求所有真实围栏归档和路由输入齐全，缺失的旧原 journal 不能由计划或历史前缀合成。原先 `resume_generation_preparation()` 的较早阶段语义不变。

离线可信摘要仍须来自独立保存的原决定，不可从同一待验目录自取。解析 native core 本身并未解决外部信任、真实目标身份、实际数据验存或活动 I/O 准入。

## 资源与未完成范围

- 证书沿用 32 MiB 上限；新 native authority 对象沿用 64 MiB 原生元数据上限，外围清单与完成标记各不超过 1 MiB。
- 每次只保存第一份待写对象和当前目标推导结果，不同时收集八个全量副本。既有 State、路由、control-archive 读取各自预算仍生效；每目标均独立计费，不用去重或其他盘空闲空间冒充本盘容量。
- 实际 I/O/fsync 可能阻塞，取消检查点不构成任意故障下的硬关机承诺。
- 仍只支持完整固定 Genesis、单 primary 祖先链、替换一条原末端及明确选择的一至八个全副本目标；复杂旧分叉、异盘父子合流与外来高等级一次完整链正式接纳均未完成。

**最终活动切换仍未完成**：新的有效多下级 Config/CurrentRegistry、首笔 Publication、对应出版封印、serving locator、完整新绑定历史数据库、未来备份策略延续、目标 writer 和源围栏安全解除都还需要共同闭合。Candidate 与本片新增的首槽 ACCEPT/FINALIZE 不能替代这些条件。旧正文、旧数据库和全部永久收据不删，源池仍可能停写，已作 Backup 选择后不能假装可任意改作 Abort。不能把 `GenerationAuthorityPrepared` 显示成迁移成功或 H03 外来盘接纳成功。


<a id="backup-first-candidate"></a>

## 首笔原生 Candidate 与完整 State

`membership_transition/backup/generation/first_candidate.rs` 从原决定自己的严格证书、完整源 State 和共同整单位路由投影生成真正的 `membership-generation-candidate`。它不接受任意调用方提供的最终 State，也不把普通 Add 的空版本 Candidate 改名为 Backup：

- 固定首笔序列为原 `MembershipFence + 1`，前快照仍是原已发布边界；原 operation、principal、请求摘要、generation identity、事务 ID 和所有原元数据成员确定不变。
- 先用与 `GenerationInputsPrepared` 相同的投影重建全部路由，重新核原 authority 绑定的 routing snapshot；然后仅推进 sequence，并在完整 State 加入该操作的确定性结果模板及原主体。拓扑只推进一次，业务代数、旧 receipts/events、HEAD、D/undo、退役/终止证明、数据库相关图和私有原生字段不改。
- 每个 AVAILABLE 保留版本都进入 Candidate 的真实 `versions` 向量，携带完整新 route、binding 和 protection obligations。PURGED/MIGRATED/RETIRED 不虚构正文，它们的描述、处置和永久证明继续存在于 full-State。版本向量不是已完成复制的证书。
- 沿用真实 native snapshot 分页、history-root catalog 和 candidate 编码，保留所有对象依赖。原元数据成员集合不由目标数替换或缩小；新隔离目标不因存有这些文件自动获得 voter 身份。

结果模板中的 `PUBLISHED / TARGET_MET` 仅处于**尚未发布的 Candidate 快照**，是未来发布流程必须满足的预定结果，不写入活动 State、不返回为 `OperationResult`；单独的候选准备阶段不追加日志。只有将来真实完成全部原成员 ACCEPT/FINALIZE/PUBLICATION 及逐版本本盘证明后，才可采纳该结果；当前所有活动 Backup 转换仍拒绝，不存在采用这个模板的成功入口。

### 后台持久化与冷态读取

原 registry/placement worker 在全部 `GenerationAuthorityPrepared` 后继续写入 `backup-generation-first-candidate/`。所有目标先各自重放实际 source archive、核验实际 native authority 和完整 published State，得到相同 Candidate/manifest，并各自满足容量预留，才开始无覆盖安装。每个目标用持有的目录 FD 读回、严格 native Candidate 解码、逐摘要解析完整 snapshot，并走完整 State 语义准入；所有目标成功后才写 `generation-first-candidate-prepared.json`。诊断阶段为 `GenerationFirstCandidatePrepared`，不是已发布或可写。

`BackupDecidedTargets::inspect_first_generation_candidate` 只读重建并核验该整批材料。`resume_first_candidate_preparation` 是显式有写操作，只补同一原任务缺失的隔离文件；独立持有的原 decision hash、完整目标集合及原物理绑定仍是前提。它不打开原源盘/SYSTEM、不发新选择、不把另一目标的 State 填入损坏目标。未知文件、不同字节、失联、容量不足或任一原证明不完整时，保留原物和明确失败。

### 预算与未完成边界

每目标新增 Candidate + native snapshot/history 闭包最多 8,192 对象、128 MiB；manifest/completion 各最多 4 MiB；单对象仍遵守原 native metadata 上限。真实读取按每次 decode/比较计费，包括重复引用，上限 24,576 次/384 MiB。安装还逐目标保留原 material store 的 16 MiB 及目录/原子临时文件预算。失败保留合法原子临时文件但不把它们作为证明，不清理旧源或旧 receipt。

**当前仍缺真实 Publication、有效 fanout Config/CurrentRegistry、最终绑定数据库/serving locator、未来放置策略、目标 writer 与源围栏解除。** 上述候选材料 API 不写 ACCEPT/FINALIZE/PUBLICATION，不安装 generation marker/seal/journal，不开普通业务写者，不接 helper/正常配置自动执行。后续原成员首槽日志写入是下一节单独说明的能力，仍不是 Publication。不能称整体迁移完成。按用户指示未审查、编译、测试、静态检查或 CI，所有新代码仅为未验证实现。


<a id="backup-first-finalize"></a>

## 全部原成员的真实首槽 ACCEPT / FINALIZE

本片将原 registry/placement 后台任务继续到 `GenerationFirstCandidateFinalized`。这是实际原盘原生日志记录，不是目标 sidecar、计划收据或材料完成标记；同样**不等于 Publication、目标 writer 激活或源解围栏**。

1. 从每个隔离目标自己的原始归档和完整 State 再次重建、实际读回相同 Candidate 及其所有依赖。通过类型保存原完整 Config、共同 registry 决定和 native authority，不允许调用方替换最终 State、原 voter 集合或第一序号。
2. 通过现有 `BackupChoiceContext` 重读全部原成员的实际共同选择、原 seal 和截至 MembershipFence 的原应用日志。原源目录锁及 I/O lease 全程保留。每个原成员先检查容量和已存在的首槽前缀，随后在普通不可变对象存储安装并读回完整 authority / Candidate / snapshot / history 依赖，不覆盖旧字节或使用旧 flat-layout 回退。
3. 每个原 metadata 成员使用原已准入 root/authority，在 `metadata/membership-generations/<authority>/journal.bin` 写实际 `ACCEPT`，sequence 固定为原 fence + 1、首帧前驱为零。新目标不因此成为 voter。这里没有写 `generation.json` 或活动 disk seal，正常 Engine 仍沿原配置保持 fenced；原 application-log 不追加、不截断、不改旧 receipt。
4. 必须取得每个实际写入返回、且重新从原盘读取一致的 ACCEPT 后，才用这组真实收据构造原生 `Finalize`。全员保存同一个不可变 Finalize 对象，然后各自追加实际 FINALIZE，最后再次读全体源盘对象与帧。仅全部匹配后才推进诊断阶段。

`JournalWriter` 不向调用方暴露。新的窄 journal owner 只允许本 Candidate 的 ACCEPT 和 FINALIZE，不允许 Publication、后继 slot、额外记录或不同对象；它继续使用原 DiskIo 的动态撤权与身份门禁，不把准备目标或 unbound root 借作旧 metadata 成员。即便重试返回已存在收据，也必须重复真实文件和父目录同步，并核当前名字仍指向持有的日志文件。任一 I/O 失败或 panic 消费该 writer 状态，只能重新打开并按原前缀准入。

### 中断恢复与只读观察

SYSTEM 保留的原任务在冷开重建旧权威和旧 fence 后，可重复同一后台流程。它不重新选择 Candidate 或缩小成员集合；只在原成员仍持有一致实际共同决定、全目标内容/元数据仍完整时，补同一首槽。写到一半的 ACCEPT/FINALIZE 尾部可沿既有持久日志修复原语截回已经认证的完整前缀。**完整 FINALIZE 之后出现任何截断后缀、第三条记录、未知记录、候选/序列/Finalize 向量冲突均拒绝并保留**，不猜测为可丢弃的 Publication。

只要任一原成员已有完整 FINALIZE，全部原成员的实际 ACCEPT 就必须仍在；丢失的票不能依据别人的 Finalize 或计划帧重新制造。已确认的原票不改变，必要的相同不可变对象可由全目标精确材料无覆盖补缺。不存在 source-offline 的替代投票路径。

`Engine::inspect_backup_first_finalize(operation, principal)` 在原 worker 完全结束后只读重验同一 actor/task、全目标和全原成员，返回 `BackupFirstFinalizeReport`。它不创建/修复日志、不补票；离线 `BackupDecidedTargets` 仍只提供隔离材料读取/续写，不暴露这个原成员写入能力。SYSTEM progress 或报告字段不能作为业务成功或权限凭据。

### 资源与仍未闭合

- 每原成员追加首槽最多两帧，真实扫描总上限 16 KiB；仍使用原生 frame parser 的单帧分配上限，坏 length 不会变成无界分配。只有已认证的两阶段前缀允许显式恢复。
- Candidate 原 8,192 对象 / 128 MiB 上限保留。每原盘保守预留完整对象及 authority 的两倍字节、两份 native metadata 最大对象、16 MiB 固定余量及每对象 8 KiB 目录空间；不借其他盘空闲容量。每次元数据读取、编码和同步仍可能阻塞；取消检查点不等于硬实时关机保证。
- **没有写 Publication / publication seal，没有真实逐路正文 CommitProof，没有切换活动 Config / CurrentRegistry / serving locator / 新绑定数据库，没有启用目标 writer 或解除源 fence。** 新 Finalize 不能把候选内的 PUBLISHED 模板变成活动结果；源池仍可能停写。现有完整固定 Genesis、单 primary 原链、一条原末端、显式 1–8 全副本目标及父子同层同盘范围不扩大，外来完整链正式接纳仍关闭。
- 按用户要求本片未审查、编译、测试、静态检查或 CI；未实际触发任何备份、日志、磁盘或服务操作。
