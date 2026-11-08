# 备份原生 generation 的选择身份与激活隔离

[Engine](../components/engine.md) · [任务表](../../TASKS.md) · [真实围栏归档](backup-generation-control-archive.md)

## 已有选择身份与本片范围

在现有 `PortableAuthority` 的原生 generation 解析中加入严格独立的 `Backup` core，格式为 `backup-registry-generation`。它携带原来全部成员实际选择的完整证书、原 `membership-control-archive` 根、未发布完整路由快照及相应输入摘要；使用既有 `membership-generation` 域编码和计算 identity，不把普通 Add/Abort、SYSTEM 进度或孤立的目标完成文件当作新 authority。

该 core 的外层及证书都有严格字段闭包。证书重新核原 transition、所有原成员及真实选择 receipt 的规范关联；新增冷态投影检查将完整源配置、被冻结的源和原末端、每个指定目标的身份/物理绑定/根名/逐级前缀与原任务逐项绑定。所有目标的完整文件和目录仓库地址转成相同的相对单位表，父子仓库及版本集合不分散到不同目标。完整旧 State 的序列、规范摘要及 native snapshot 根必须与 source fence 原出版边界相同。

这些是**选择身份与材料准入**，不是已经发布的新活动 generation。现已在隔离目标生成下述首笔 native Candidate；已有全部原成员的真实 ACCEPT/FINALIZE；本片继续生成保留代际中的真实首笔 Publication/seal，活动切换仍未接通。类型中没有偷偷把原 Config 作为新的有效拓扑：`effective_config` 对 Backup 明确失败，原生 registry/Current 转换、I/O 限定和普通激活仍拒绝。即使把这种 core 放进普通 generation 标记，也不能用旧 voters/旧 Config 启用目标 writer。原 Add/Abort/Exit/Readmission 行为没有被映射为 Backup。

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

**最终活动切换仍未完成**：新的有效多下级 Config/CurrentRegistry、serving locator、完整新绑定历史数据库、未来备份策略延续、目标 writer 和源围栏安全解除都还需要共同闭合。首槽 Candidate、ACCEPT/FINALIZE 及本片保留代际的 Publication/seal 不能替代这些活动条件。旧正文、旧数据库和全部永久收据不删，源池仍可能停写，已作 Backup 选择后不能假装可任意改作 Abort。不能把 `GenerationAuthorityPrepared` 显示成迁移成功或 H03 外来盘接纳成功。


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


<a id="backup-first-publication"></a>

## 保留代际的真实首笔 Publication

本片不再用材料完成标记代替出版：同一已决定后台任务在原盘全员真实 `FINALIZE` 之后，继续写真实 `CommitProof`、`Publication`、原成员日志帧和 `PublicationSeal`。新的 `GenerationFirstCandidatePublished` 仅表示**保留代际首笔出版完成**，不是服务切换成功，仍不向活动 State 插入迁移结果或解除原写停。

### 真实逐路正文与独立目标

- 原源围栏能力另外持有保留下级链每个来源盘的原 I/O lease、物理根/封印和已有 writer 锁；不把被替换的旧末端当新目标的替代副本，不用在线子集减少义务。
- 每个 AVAILABLE 版本的 Candidate route 必须恰好覆盖完整保留祖先链和全部明确目标。父子同层同盘和完整单位仍来自已决定实体表，不改变路径、HEAD、旧数据库或旧收据。
- 每一条实际 route 均从自己的盘读取 `version.json`、准确 checksum manifest 和完整正文。描述、checksums 和正文使用各自持有的同一 FD，整文件 BLAKE3 与逐块校验后复核 inode/大小/时间戳及当前名字，再执行文件及父目录同步；容量计划或一次成功的进度缓存不是验存。
- 原源使用已有真实 container resolver。目标使用其自身已核的 `histories/<history>/versions/<version>/content/<creation_name>` 保留 native package 正文，不回退到旧源或另一目标，也不从未激活的名字解析器任取内容。带直接上游逐级前缀的完整 `activation-image` 和全部对象库继续由共同决定门禁独立重验；它们未安装为活动 serving locator。这仍可能保留同盘的 package 与实体 image 两份正文，未做清理/去重。
- 每个证明绑定精确 Candidate、实际全员 Finalize、version 摘要、目标 disk/slot 及完整 route，写入普通 native 版本证明位置。已有不同首笔证明时使用该 Candidate 的 `protection-proofs`，旧证明不覆盖；新目标仍没有活动 `disk.json`/I/O admission。

### 同一个原决定的发布与恢复

1. 全员真实 ACCEPT/FINALIZE 和候选依赖必须齐全。先预留每个实际目标/来源根的证明、原子临时及原成员出版元数据空间，再执行第一笔新证明写入。
2. 只从已同步并独立读回的实际 CommitProof 形成 payload receipt 向量，按 version/disk 固定排序。所有原 metadata 成员先持久化完整同字节 Publication 对象，随后在所选 generation 的独立日志追加第三帧；新目标不投票。
3. 各原盘实际同步返回的 receipt 再从原盘新读回；全部到齐后用既有 `verify_publication_seal` 核实际链并保存不可覆盖 seal，再在每个真实 route 保存同一原生 local publication proof。
4. 冷续作仍只用原 operation/actor/决定/目标集合。出现任一完整或撕裂的第三帧后，早先全部 Finalize、Publication 对象和真实 CommitProof 必须仍在；丢失这些前置证据时拒绝，不借另一成员的后续凭据重新造票。可补同字节未完成的其余第三帧、seal 和 local publication。
5. 首槽仅允许 ACCEPT/FINALIZE/Publication 三帧。仅在已经重验完整原任务及正文证据后，明确 Publication 写路径可截断第三帧的已知撕裂尾部；重试 ACCEPT/FINALIZE 不修掉这个尾部。三帧之后有任何尾部或后续槽均拒绝保留，不把未知日志重置为新任务。

`Engine::inspect_backup_first_publication` 是原 fenced Engine 的只读观察，需要全部原成员和全部原目标，重新流式验每条 route，观察原日志、对象、seal 和 local proof，不创建/修复/同步，也不返回活动成功结果。离线目标读者仍不能代替原成员提供新票。

### 预算与明确限制

- 新逐路证明向量最多 65,536 项，每份 native JSON 沿用 64 MiB 上限。每个根单独预留实际序列化大小、原子临时、目录项与 16 MiB 余量；不以其他盘空闲替代。过大的 local proof 在第一笔新证明写入前拒绝。
- 真实核验成本随保留正文总量 × 完整目标/来源 route 增长；发布前后会重验，普通保存路径未加入此全量扫描。停止检查点仍不承诺任意故障 I/O 的硬关机时限。
- 没有安装有效 fanout Config/CurrentRegistry、最终绑定范围数据库或 serving locator，没有活动目标 writer、策略延续、source-unfence 或 helper/H03 正式接纳；普通 Backup activation 继续明确拒绝。即使本报告存在，也不能当作运行态完成或把未完成目标注册为普通成员。
- 按用户要求，本片未审查、编译、测试、静态校验或 CI，未对真实池触发上述操作；仅生产源码、文档和 TASKS 更新。

<a id="backup-target-publication"></a>

## 所有目标保留真实首笔 Publication 与原决定离线续存

本片接在真实首笔 Publication 全员日志及逐 route 正文证明之后。后台原任务将原生 Publication 和全原成员 seal 保存到**每个原选定目标**的 `backup-generation-first-publication` 独立域，最后写 `generation-first-publication-retained.json`。没有 AVAILABLE 版本的历史也必须具备这份公共终态，不能因为没有版本 local proof 就省略 Publication。只有全部目标读回原对象、完整候选和自己的真实正文/CommitProof/local publication 后才到 `GenerationFirstPublicationRetained`。

- Publication 必须等于原确定 Candidate、完整 OperationResult 和所有原成员 ACCEPT→FINALIZE→Publication 链。原成员 slot 与原 Config 绑定，首 ACCEPT 从零前驱开始。payload receipts 必须精确覆盖每个 AVAILABLE 版本的全部保留原链和全部选定目标，绑定不能减少、替换或遗漏；沿用原生确定排序和 65,536 条上限。
- 两份 native 文件分别限 64 MiB，manifest/完成描述分别限 16 KiB。全部目标先各自验存和容量检查，再做原字节无覆盖写入；未知、冲突或损坏保留拒绝。临时原子文件只保留、不提升为证据。完成标记不单独授权任何后续活动。
- `Engine::inspect_backup_target_publication` 同时读取原成员真实 Publication 和全部目标公共闭包。原 `inspect_backup_first_publication` 仍仅报告真实保留代际出版，不暗示新公共闭包已齐备。
- `BackupDecidedTargets::inspect_first_generation_publication` 额外要求独立保存的 `BackupFirstPublicationHash`，并复用打开时独立保存的原 decision hash。摘要解析只检查表示；调用者必须从先前成功的可信 live 观察独立保存，不能从待检查目标发现摘要再自证。每次只走原选定目标持有的 fd，重建完整原 Candidate/State/历史依赖，再验公共文件和该目标自己的真实正文及原 local proof，不访问失联源或 SYSTEM，不用另一副本掩盖损坏。
- `read_published_version_range` 在原有不超过 4 MiB 范围读取前后执行上述完整门禁。返回的仍是冻结恢复内容，不是活动文件、当前命名空间或普通 SMB/HTTP 读取能力。
- `resume_first_publication_replication` 是显式授权写入的离线同任务续存，**不是重新决定**。至少一个原选定目标必须已有独立摘要认证的完整公共证据，全部原选定目标的既有正文和 local proof 都必须在；任何声称完整但损坏的目标直接拒绝，不跳过。不补造原成员日志票、早期 CommitProof 或 local proof。若崩溃早于任何公共完成标记，保持 `RecoveryRequired`，由原源任务恢复，不用计划摘要推算“已经投票”。

这些 API 不创建目标 runtime seal、CurrentRegistry、有效 fanout Config 或 writer 租约，不修改活动 State，不解除源围栏，不删除旧数据或缩减原副本义务。离线验读证明的是原保留代际已独立留存和当前目标材料完整，不证明已离线原源的当前健康。Serving locator、最终绑定范围数据库、未来策略和完整运行态切换仍未完成；H03 外来一次性全低级链不得由此放行。普通设置和 helper 不自动调用不可逆原决定接口。

本片按当前指示未审查、编译、测试或运行 CI，代码和声明均不能替代实际故障验收。

<a id="backup-effective-configuration"></a>

## 原首笔 Publication 下的有效配置语义与原生注册图

本片将原生 `BackupCore` 的配置推导从“返回不支持”改为**原决定精确选择的实际扇出配置**，不是把旧配置当新配置，也不是放宽公共配置加载。推导只接受已认证 Backup certificate 中的原完整 Config 和一次选择的目标/物理根：全部原盘按原顺序保留，原身份、级别、容量、metadata voter 和 Genesis I/O authority 不变；新目标只追加绑定代数 1、原指定直接上游和级别，不成为 metadata voter、不凭数量猜副本策略。新目标不在推导对象的 I/O 准入集合中。

普通 `Config` 的 API/文件解码与初始化仍执行原单后继校验，不能靠写配置文件绕过原生决定加入多个盘。只有原生 Backup 内部推导允许完整无跳级扇出；下一步活动 writer 准入还需独立原生许可。旧末端即使退出新的数据 route，也仍是保留的登记盘；如果它是原 metadata voter，其票不能顺便删除、Disabled 或 Stop。后续更改 metadata membership 必须走自己的协议。

后台原任务在 `GenerationFirstPublicationRetained` 后自动继续配置解析。新增 live `inspect_backup_published_configuration` 与离线 `BackupDecidedTargets::inspect_published_configuration`：先读原实际 Publication/全目标公共证据及逐目标真实正文证明，再从目标自己的完整原源快照和确定性首笔 native snapshot 生成配置。所有保留版本，包括 Retired/Purged/Migrated 的无正文历史，placement 都必须等于同一完整显式 route、绑定和拓扑代数；缺低级祖先、跨 primary 或分拆整单元均拒绝。

新生成的 `PreparedDiskRegistry` 包含真实原生编码的 Genesis 父图及精确后继图，保留所有原登记与固定 voter，只加入原决定的新目标；其 source 绑定原首笔 snapshot、sequence、topology generation 和 Backup authority，不从磁盘目录数量或完成标记猜登记。图预算为 4,096 个对象/64 MiB。`BackupConfigurationReport` 说明配置摘要、完整盘定义、明确数据 route 和注册图根；`GenerationConfigurationResolved` 仅是此闭包已解析，不等于图已安装到 Current。

**尚未开放活动 CurrentRegistry、target writer、serving locator/最终绑定数据库安装和源解围栏。** 注册图可从原精确已出版材料确定性重建，不是已写到活动根的声明，第一笔旧 Candidate/Publication/receipt 一字不改。原生 Backup 的普通 registry/writer/I/O 激活门禁保留，不能把返回的配置定义另存成新 Genesis 来启动。当前新增分支只涉及固定 Genesis 源，复杂旧注册生命周期/旧分叉和正式外来一次完整链接纳仍拒绝。没有真实磁盘操作；按要求未审查、编译、测试或 CI。

<a id="backup-published-registry"></a>

## 目标原生注册图的真实安装、原任务续存与冻结命名空间读取

此前 `GenerationConfigurationResolved` 只在内存推导完整原生注册图。本片接原后台决定，在真实首笔 Publication 已留在全部目标后，将**准确 Config 字节和原生注册父/后继图的每一页**无覆盖安装到每个原目标的 `backup-published-registry`；全部实际页和原 Publication/body 重新读回后，才写各自 `published-registry-installed.json` 并报告 `GenerationRegistryInstalled`。此处“安装”是目标私有证据域的实际落盘，不是安装普通 Engine 的活动 CurrentRegistry。

### 不以派生图替代实际盘页

- 同一原 decision/Publication、完整源/首笔 State 和原生 effective Config 共同决定唯一 manifest。目标文件名由已准入 encoder 的 kind/hash 生成；不接受文件内任意路径或从本地 descriptor 自行选择新的权威。
- manifest 精确列举 Config、原生 registry root、`state-page` 与 `metadata-index`；读取先核完整目录清单与准确字节，再用既有严格 registry/map 解析器解码**同一个目标自己的**父图和后继图。缺父页、缺子页、额外未知文件、不同对象、重复身份或 Config/注册分区不一致均拒绝，不从内存 encoder 或另一盘补成“读成功”。
- 目标先全量容量预检，再逐盘无覆盖安装。每盘单独保留完整图、目录/原子临时空间和原有 16 MiB 余量；不借其他盘空间。任何原选定目标缺失都不缩小集合；原子临时文件只保留，不提升为证据。
- `resume_published_registry_installation` 是单独授权的同原任务离线写入，须独立保存的原 decision/Publication 摘要及全部既有真实正文/原生证明先通过。尚未写完成标记的匹配部分可以补缺；**已经声称完成的盘缺页或损坏直接拒绝**，不把它降回未完成来自动修造。所有旧标记、旧 receipt、源字节保持原义，不新增投票或业务操作。
- `inspect_published_registry` 与 `Engine::inspect_backup_published_registry` 都不写盘；前者只访问原目标，后者额外观察全部原 metadata 成员的真实 Publication，报告后再核同一原证据。系统进度缓存不是完成凭证。

### 冻结命名空间读接口

`BackupDecidedTargets::read_published_namespace(expected, slot, scope, offset, limit)` 读取原决定中指定目标自己的首笔已发布完整 native State。scope 仅允许合法 `br_` 分支 HEAD 或 `dbr_` 目录 HEAD，不接受任意历史 Commit/Tree 或外部 entries。每次都先核全目标真实 Publication、图和完成标记，再从指定目标读全 snapshot；不能用目标零或失联源替代该目标缺页。分页后再次核原 Publication 和全目标图。

返回页绑定真实 Publication hash、registry commitment、目标 slot、准确 HEAD 和 scope；保留同一 State 的软删除 D 节点与内容 Available/Purged/Migrated/Retired 状态，不把删除文件或已退役正文伪装可写/可读内容。无 State/private auth/Config 可变对象或 `CurrentNamespaces` 转换外泄，调用宿主仍须逐请求授权。分页上限 256 项，entry/D 内容最多 4 MiB，另有不超过页项数的有界 ID/状态元数据；offset 不能越过冻结条目末尾。每次是全目标完整性检查，**不是普通目录热读或 O(页大小) 的快速查询**。

原生图独立唯一对象上限 4,096 / 64 MiB，Config、manifest、完成描述各限 4 MiB。父图及后继解码总预算 8,192 次读取 / 128 MiB，重复引用也计费；目录清单/准确字节比较与既有完整 Publication/快照/正文预算另外成立。后台可观察与取消检查点不承诺故障 I/O 的硬时限。

### 未完成，不可解除围栏

本片不安装 runtime `disk.json`、普通 `metadata`、活动 Engine CurrentRegistry 或 writer 资源，不最终绑定 serving locator/范围数据库，不执行源取消或 unfence；既有 `require_isolated`、Backup 普通激活拒绝和 H03 门禁全部保留。目标仍为冻结已发布备份读视图，不是正常业务系统的新成员。固定 Genesis、单 primary 原链、完整显式 1–8 目标、父子同层同盘的支持范围不扩大；复杂旧生命周期、跨旧目标合流与完整外来链正式接纳仍未完成。

按本轮指示未进行审查、编译、测试、静态检查或 CI，也未对真实盘执行这些 API。生产源码存在不代表已通过故障、资源或部署验收。

<a id="backup-published-serving-images"></a>

## 已发布实体、对象库与同目标 serving locator

`GenerationRegistryInstalled` 之后，原后台任务现在继续安装 `published-serving-image`。这不是把先前 `activation-image` 改名为成功，也不重写已经决定的 Candidate、Publication、旧 marker 或任何原收据。每个目标以**自己实际保存的首笔已发布完整 native State**重建 FILE/目录 SQLite、全部保留版本描述及内容状态、父子目录森林和相对实体地址，再从自己的已认证历史包复制 AVAILABLE 正文。文件全部版本、对象数据库、备注/标签等仍置于同一个实体和同一个目标盘；嵌套目录仓库维持原同层同盘约束，每一级地址使用原决定的直接上游前缀。

新 image 的严格 manifest 同时绑定 source fence、原 decision、真实 Publication、已发布 snapshot、原生 registry commitment、准确目标身份及完整 `EntityPlacement`。这些值从原生闭包生成，不接收调用者任意物理路径。locator 指向这个目标新实体内的 `.zxs-versions/<ver>/content/<creation_name>`，FILE 与目录 SQLite 也由相同已发布 State 编码，而非照搬旧源序列的数据库。旧准备材料和旧源独立保留，现阶段不尝试覆盖、硬链接、reflink、去重或提前清理以节约空间。

### 自动后台安装、容量和原任务续作

- 选取一项**新**不可逆 native decision 之前，先在所有目标检查额外完整正文副本加图片规划最大元数据/目录分配的保守容量界限；这不预造 Candidate、不变更票数，也不是文件系统空间预留。已有原决定按原任务继续，不能因容量不足重新选目标。
- 安装前再次针对所有目标做真实缺失文件/容量预检。每盘需要额外一套完整实体，不借其他目标的空闲容量；ENOSPC、掉盘或文件系统错误返回并保留原字节，不假装回滚已发布决定。
- 每盘无覆盖复制后实际重读全量目录、元数据、对象 SQLite、checksum 和正文；全部目标验证完成后，才写绑定**完整目标清单及每盘 image 摘要**的 `published-serving-images-installed.json`。后台状态随后到 `GenerationServingImagesInstalled`，不报告迁移成功或 writer 已激活。
- 未完成的同原任务匹配部分可以补缺。已经声称完成却缺失/损坏的 image 直接拒绝，不先降级标记再从旧 image/其他盘修补。旧完成标记不改义。保留受限数量的原子临时文件，但不把它们当实体或完成证据，也不移除它们。
- 显式 `BackupDecidedTargets::resume_published_serving_installation` 需要独立保留的原 decision/Publication 摘要及宿主对目标写入的授权。`inspect_published_serving_images` 只读，`Engine::inspect_backup_published_serving_images` 另重读全部原 metadata 成员的真实原 Publication。离线续作不访问已失联源或 SYSTEM，更不补造原票。

### 命名空间与实际正文绑定

新增 `read_published_entry_range(expected, request)` 接受 `BackupServingRead`：准确目标 slot、分支或目录 HEAD scope、预期 HEAD、entry ID、version ID 和读取范围。它不接受物理路径或任意历史版本替代当前条目。所有目标闭包先通过，然后只读指定目标自己的已发布快照，精确匹配同一 HEAD/entry/version；软删除 D、非文件条目与 Purged/Migrated/Retired 内容均不得返回正文。

返回 `BackupServingContent` 包含真实 Publication/registry、entry/version 与该目标实体相对 locator。正文从该实体的实际文件 fd 读取，必须同一轮读完整文件并核对长度/散列后才释放不超过 4 MiB 的请求范围，零长度范围也不跳过完整性。完成后重新观察全部 image、原 Publication 和 registry。没有从源盘、target zero、其他副本或旧 `activation-image` 回退。宿主仍须独立认证并逐 scope 授权；此 API 尚未接到普通 HTTP/SMB。

### 资源及未完成边界

同一现有实体规划器限定最多 100,000 项、96 MiB 元数据、16 MiB image manifest；对象 SQLite 完整集合仍受 4,096 库/64 MiB 预算。完成描述最多 64 KiB，保留原子临时文件最多 1,024 项。一次范围读取只缓冲请求范围加 64 KiB 正文块，但会重建并验证完整选定集，**不是普通热路径或 O(页大小) 查询**。同步 I/O 的取消检查点不提供硬关机时限。

本片补齐已发布 FILE/目录对象库与真实实体 locator 的读取前置，仍不安装普通 Engine CurrentRegistry、活动 `metadata`/`disk.json`、目标 writer 或最终 SYSTEM/盘根绑定，不安装未来写入策略或解除原源围栏。原生 Backup 激活拒绝、原 voter/副本义务和 helper H03 门禁保持；固定 Genesis、完整单 primary 原链与原明确 1–8 目标的支持范围未扩展。原源池仍可能停写，不能在生产池试运行这些未完成切换接口。

按本轮要求仅编写生产代码与文档，未审查、编译、测试、运行静态检查/CI，也未对真实盘执行 API。未验证源码和阶段完成标记不构成最终安全切换验收。


<a id="backup-published-scopes"></a>

## 已发布 SYSTEM 与盘根范围的真实编码、目标安装

在 `GenerationServingImagesInstalled` 之后，原后台任务继续生成**同一实际首笔 Backup Publication 的最终范围**，不是把原冻结前的 SQLite 更换标签。`published_scopes` 对每个目标分别重放其自身保存的完整固定 Genesis 前缀，再读取该目标自己的真实首笔候选完整 State、已安装原生 registry 和 Publication。源配置、前驱 snapshot、完整 State、原 decision、全部目标与物理根身份必须相同；不读源 SYSTEM、别的目标或旧 SQLite 补缺。

### 原盘过去不归给新盘

- 用既有 `PreparedDiskScopes` 保持所有原绑定的完整已认证本盘版本变化。原 terminal 的历史保持自己的 disk/slot/binding identity；首笔 Backup 只追加本次实际 placement 变化，不能把旧记录搬成新盘的过去。
- 新目标必须是 effective Config 原样保留全部原定义后追加的明确 1–8 个 fresh binding，且原 State 没有它的版本。已认证原前缀在这些身份下没有本盘行，本次实际 Candidate 才产生各自 `Entered`。没有借一个目标的库冒充另一个目标，也不以今天 placement 倒推源盘过去。
- SYSTEM 使用现有真实内存 SQLite encoder，从已发布 State 写完整当前业务族、原 operations/principals/events 和 State 代数；本盘 current/history 同样使用现有生产 encoder，并严格解码与类型对象比较。盘历史仍是既有本盘版本进入/变化/移出范围，不声称全部过去标签或目录事件数据库已经闭合。

### 真实独立写入与原决定恢复

每个目标私有 `published-scoped-databases` 保留 `system/index.sqlite` 和按真实 disk identity 分隔的全部配置盘 current/history 库，manifest 绑定 source fence、decision、实际 Publication、snapshot、registry 与该物理目标。随后仅将**该目标自己的** `.zxs-disk.sqlite`、`.zxs-disk-history.sqlite` 作为独立字节副本无覆盖写到其持有根；没有硬链接、复制源盘身份、打开 SQLite 路径写入或清理旧文件。

所有目标先做原证明、完整实体、精确范围/同名冲突与**私有集合 + 两份根副本合计**容量预检，然后逐盘写入、完整读回。全目标再次读回后才持久 `published-scoped-databases-installed.json`，绑定整批各自 manifest 和实体清单；原 live worker 再核全原 metadata 成员的实际 Publication 后报告 `GenerationPublishedScopesInstalled`。落盘异常保留此前字节，下一次只按同原任务续存匹配的缺项。已声称完成的目标缺库/异内容是故障，不降级为未完成自动修造；既有冲突文件/原子临时文件不删不覆。

`BackupDecidedTargets::inspect_published_scoped_databases` 为目标独立只读观察；`resume_published_scoped_databases` 是宿主须单独授权的同原任务写入，不能自行重选 decision 或接纳成员。两者都要求独立保存的原 decision/Publication 与原完整目标集。`Engine::inspect_backup_published_scoped_databases` 另核原 actor/task、持有原锁并在观察前后读取原 metadata 的真实 Publication。进度或完成 JSON 不代替实际 SQLite 和原生证明。

### 边界、成本和未完成事项

最多 32 个原固定绑定加 8 个明确新目标，每目标最多 81 个 SQLite、完整范围集合 128 MiB，单个盘 current/history 仍各受既有 32 MiB 上限，SYSTEM encoder 64 MiB；原历史 replay 的 2,048 publication、65,536 change 行和 64 MiB 中间范围上限不放宽。新首笔也占该 publication 预算，超限拒绝保留，不丢前缀。manifest/整批完成记录各最多 64 KiB。新不可逆选择前增加保守的最终 scope/根副本容量；之后实际每盘检查缺失字节与目录/临时文件余量，容量不是 filesystem reservation。

为不在内存保存所有目标的整套库，重建/预检/安装/验读逐目标执行；多轮会重复完整原生前缀和 SQLite 编码，成本与目标数、保留 publication 和配置盘数相关，**不是热查询复杂度或吞吐承诺**。同步 I/O 取消只在明确检查点响应。

本片不替换源 `index.sqlite`、原盘根库，不写目标 `disk.json`/活动 `metadata`，不安装普通 CurrentRegistry、runtime target writer、未来放置策略，也不解除源围栏。SYSTEM 与其他盘的最终活动绑定必须等待同一原生权威/Current 安装闭包，不能在内存仍是旧 State 时提前替换活动库。目标根数据库仍只是已发布状态的派生投影，无 disk seal/准入证明不能被独立读取器当正式成员；helper H03 完整外来低级链仍关闭。旧数据、原 receipts、原 image 与原冻结 scoped 数据均保持不变。

按本轮要求未审查、编译、测试、运行静态检查或 CI，未操作真实盘；上述为生产源码接线，不是已验最终迁移成功。


<a id="backup-common-databases"></a>

## SYSTEM 与全部原盘的共同数据库落盘

`GenerationPublishedScopesInstalled` 已覆盖目标私有完整集合与目标自己的两份盘根库，但源 SYSTEM 和原盘仍没有后继范围的本地副本。本片由**原 live Engine 的同一后台决定**补这一写路径，不先替换活动数据库再设法修复旧 State。

### 保留活动库，安装确切后继副本

- SYSTEM 写入 `backup-common-databases/index.sqlite`；每个原配置盘写入其**既有** `metadata/backup-common-databases/` 下的 `.zxs-disk.sqlite` 和 `.zxs-disk-history.sqlite`。目录清单严格只允许其当前 scope 的文件和 manifest；缺失原 `metadata` 不自行初始化。目标保留上一阶段自己的实际盘根两库，不借源副本代替。
- 编码来自全目标分别通过实际原生前缀、Publication、registry、实体和 scopes 读回之后，固定第一个目标的同一已发布 State/原生前缀。这个固定来源缺损直接失败，不换别的目标补造；没有读取源正在使用的 SQLite 来猜 State。
- 全部原盘，包括不再承担最终 route 的原末端和无当前正文的原登记，必须由原 Engine 的 DiskResources/原 writer lock 正规取得。每个根持有 I/O lease，逐次重核命名路径 dev/ino 与原 `disk.json` 字节；SYSTEM 也绑定原持有目录与原 writer.lock 身份，并持续复核命名锁和实际持有锁一致。不能凭目标清单在运行态登记新成员，或把 offline 原盘从共同安装集合删除。
- 所有库都通过既有独立 no-clobber 原子写原语保存，原活动 `index.sqlite`、原盘根 current/history、源正文/旧 receipt 不覆盖不删除。SQLite 不通过未约束路径打开写入。**这些名字是 generation-local 后继副本，不是已切换的活动别名。**

### 一个原任务、一份共同描述

共同 manifest 绑定原 fence、decision、实际 Publication/snapshot、registry、SYSTEM 物理身份、全部原绑定/物理身份/seal 摘要、各准确本盘库摘要及原选定目标 scopes。全部成员先容量/碰撞预检，再将同一描述无覆盖写入 SYSTEM `backup-preparation/common-databases.json`，之后落 SYSTEM/全部原盘的准确库。

所有本地实际字节、目标完整材料和原 metadata 全员真实 Publication 重新读回后，才在 SYSTEM、每个原盘的 metadata 和所有目标保存同一 `common-databases-installed.json`。原后台到 `GenerationCommonDatabasesInstalled`。任一个参与方已经有完成描述，就要求**整批数据已经完整**；后续发现缺库/异内容不能把其他参与方当未完成来补造。只缺后续完成标记的正常中断仍可按同一原描述继续同步，已有不同描述永久保留并拒绝。

冷开仍先恢复原 State/原 fence，再走原 registry task 的确定路径；不重选目标、原 decision、Publication 或历史归属。新增 `Engine::inspect_backup_common_databases` 必须原 actor/task、无活动后台冲突、全部真实原/目标根和原 Publication，纯读观察不修复。没有离线仅凭目标 JSON 写源库的接口。

### 仍缺的原子活动边界与预算

当前普通 Current 需要独立的原生活动准入证明，不能从 common manifest 推出来。本片因此不切换 Config、CurrentRegistry、`disk.json`、未来写策略或目标 writer，不解除源 fence。普通查询仍使用原活动 State/索引，原池可能继续停写；缺少最终选择/安装闭包时，不得调用普通 Add、metadata readmission 或 helper H03 绕过。

最多 32 原盘、8 目标和一个 SYSTEM。新增选择前每个 SYSTEM/原盘单独检查 80 MiB 保守余量，实际安装按一份最多 64 MiB SYSTEM 或两份各最多 32 MiB 本盘库加目录/原子临时/完成余量计算；不把统计当物理 reservation。共享文件系统/外部空间消耗可导致中途 NoSpace，保留原数据和未完成诊断，不伪成功。完整 scopes 仍受上一阶段 81 库/128 MiB 限制，manifest/完成描述各 64 KiB；已有原子临时文件最多保留 1,024 项，不删除或提升为证据。

后台会多次读完整原前缀、目标集合和实际数据库，成本随完整历史、原盘和目标数变化，不是热查询或硬时限保证。同步 I/O 仅在检查点响应取消。按用户要求本片未审查、编译、测试、静态检查或 CI，未实际操作盘；共同副本落盘不构成最终活动迁移验收。


<a id="backup-current-read"></a>

## Backup 首笔发布的专属 Current 只读准入

共同数据库副本不等于普通 CurrentRegistry 或活动 writer。原首笔 Backup Candidate 的持久表示没有生命周期 registry 字段，不能修改其旧字节、重算旧 receipt，或借 Add/Exit 的 `AdmittedDiskExitPublication` 解除门禁。本片独立接入 `disk_registry/current/backup`，只返回不可序列化、不能转换为普通 Current 或可写 lease 的内部借用。

### 从实际完整前缀重建，而非清空索引

每次读取在指定目标的持有根内验证原完整 native archive，逐笔消费实际 Candidate/Publication/seal、永久回执和已准入完整 State。私有累加器从原固定 Genesis registry 和空 State 起步，通过普通 `CurrentNamespaces::advance` 的精确写集规则重建命名空间、历史身份及递归备份策略索引。不能从最终 HEAD 简单创建空策略索引，也不只按收据 kind 认定变更合法。完整 archive 返回前累加器不产生任何读取或写入能力；原有 Current 无法准入的历史操作明确报错，不回退到裸 State。

完整前缀末端须同时匹配原任务冻结 State/hash 和首笔 Backup 的前驱 snapshot，再绑定该目标实际保存的首笔 Publication/seal、原确定 Candidate 完整 State 和实际 native registry。命名空间数量、每个原 HEAD 及实际 entries 均须保持；最终 registry 的 pool、authority、snapshot、sequence、topology generation、原 predecessor 和全部 Enabled 登记必须相同。首笔后继只能改变既定路由和控制字段，不能借读取入口换历史树。最终所有 scope 强制只读，没有 `into_current`、可写 State 或普通工作区 baseline 的输出。

### 真实读入口与原任务恢复

既有 `read_published_namespace` 和 `read_published_entry_range` 经此消费者解析准确 scope。不是给旧 JSON 增加一个“已准入”标签：选定目标必须有自己的完整原生前缀、真实首笔 Publication、registry 和完整 snapshot，不以源盘、目标零号或缓存报告替代。分页仍有 1–256 项及 4 MiB 上限；正文仍精确绑定 HEAD/entry/version，并在同一个实际文件 fd 完整核长/散列后返回有界范围。D、Migrated 和 Purged/Retired 的原拒绝语义保留。宿主仍须另做认证和逐 scope 授权，这些 API 尚未接普通 HTTP/SMB。

原后台任务在 `GenerationCommonDatabasesInstalled` 后，对全目标建立上述只读视图并核对同一完整 namespace 计数；前后再次观察 SYSTEM、全部原盘、全目标的共同库与原 metadata 全员真实 Publication，再记录 `GenerationCurrentReadAdmitted`。新增 `Engine::inspect_backup_current_read` 只接受原 actor/task、无活动后台冲突、原锁和全部真实根，按相同路径重新读取。进度记录不是权威，不保存可复用的“已准入”能力；冷恢复继续原任务并重做实际证明，不凭旧 phase 跳过验读或解除 fence。

### 限制与成本

当前只支持原固定 Genesis 完整前缀，仍受每轮 2,048 publication、32,768 对象读取/日志帧及 128 MiB 元数据预算。读取增加逐目标全前缀重放和完整 namespace 比较；持有一份上一 State 的结构共享克隆、派生 Current，以及原 archive 当前/前一 State；事件等非共享字段仍有复制成本。正文入口另持有已有 image 验读的完整 State。这不是 O(页大小) 的热查询，也没有缓存、吞吐或内存峰值验收；同步 I/O 取消仍只在既有检查点响应。全量状态解码之间按实际失去追踪的 `All` 写集路径处理，不伪造稀疏变更。

活动 SYSTEM `index.sqlite` 和原盘根库别名、Engine Config/Current、runtime target writer、下一笔 publication、未来规则及源围栏均未切换。旧源、原 receipt 和旧 Candidate 保留，H03 不因只读准入开放，原池仍可能停写。目标数据库和 Current 读借用不能替代未来共同活动资源/权限的完整证明。

按用户要求，本片仅编写生产代码和文档，未审查、编译、测试、静态检查或 CI，未在真实盘运行任何入口；不声称最终迁移或安全解除围栏已完成。
