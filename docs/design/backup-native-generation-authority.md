# 备份原生 generation 的选择身份与激活隔离

[Engine](../components/engine.md) · [任务表](../../TASKS.md) · [真实围栏归档](backup-generation-control-archive.md)

## 已有选择身份与本片范围

在现有 `PortableAuthority` 的原生 generation 解析中加入严格独立的 `Backup` core，格式为 `backup-registry-generation`。它携带原来全部成员实际选择的完整证书、原 `membership-control-archive` 根、未发布完整路由快照及相应输入摘要；使用既有 `membership-generation` 域编码和计算 identity，不把普通 Add/Abort、SYSTEM 进度或孤立的目标完成文件当作新 authority。

该 core 的外层及证书都有严格字段闭包。证书重新核原 transition、所有原成员及真实选择 receipt 的规范关联；新增冷态投影检查将完整源配置、被冻结的源和原末端、每个指定目标的身份/物理绑定/根名/逐级前缀与原任务逐项绑定。所有目标的完整文件和目录仓库地址转成相同的相对单位表，父子仓库及版本集合不分散到不同目标。完整旧 State 的序列、规范摘要及 native snapshot 根必须与 source fence 原出版边界相同。

这些是**选择身份与材料准入**，不是已经发布的新活动 generation。本片进一步在隔离目标生成下述首笔 native Candidate，但仍不生成 Publication。类型中没有偷偷把原 Config 作为新的有效拓扑：`effective_config` 对 Backup 明确失败，原生 registry/Current 转换、I/O 限定和普通激活仍拒绝。即使把这种 core 放进普通 generation 标记，也不能用旧 voters/旧 Config 启用目标 writer。原 Add/Abort/Exit/Readmission 行为没有被映射为 Backup。

本片按指示没有审查、编译、测试、静态检查或 CI。仅对编辑的 Rust 文件执行格式化，不是编译或产品验收。

## 后台任务与原任务恢复

原 registry/placement 后台任务在全目标 `GenerationControlPrepared` 后继续到 `GenerationAuthorityPrepared`：

1. 从每个目标自己的完整 State、路由快照及真实 fenced control archive 重新推导原生 core；没有旧源、SYSTEM 或其他目标回退。
2. 所有目标分别得到相同的精确原生对象并满足本地空间预留后，才开始写入。
3. 在隔离目录 `backup-generation-authority/objects/membership-generation/<prefix>/<hash>.json` 无覆盖持久化。不同旧字节或未知文件拒绝并保留。合法中断临时名最多 1,024 个，不自动删除、晋升或计入证明。
4. 全目标实际读回并经过原生 `PortableAuthority` 严格解析，才分别写 `generation-authority-prepared.json`。原源选择再次观察成功后才更新 SYSTEM 中的诊断进度。

这个目录不是 `metadata/pool-membership/generation.json`；没有写活动 disk seal、Journal 帧、Candidate、Publication 或成功 operation receipt，也没有赋值 Config/State/Current 或获取活动目标 writer。

`BackupDecidedTargets::inspect_generation_authority()` 为只读全目标重验。显式 `resume_authority_preparation()` 仅无覆盖补写同一已决定任务的隔离 authority 材料，调用者须另有目标写入权限；它仍要求所有真实围栏归档和路由输入齐全，缺失的旧原 journal 不能由计划或历史前缀合成。原先 `resume_generation_preparation()` 的较早阶段语义不变。

离线可信摘要仍须来自独立保存的原决定，不可从同一待验目录自取。解析 native core 本身并未解决外部信任、真实目标身份、实际数据验存或活动 I/O 准入。

## 资源与未完成范围

- 证书沿用 32 MiB 上限；新 native authority 对象沿用 64 MiB 原生元数据上限，外围清单与完成标记各不超过 1 MiB。
- 每次只保存第一份待写对象和当前目标推导结果，不同时收集八个全量副本。既有 State、路由、control-archive 读取各自预算仍生效；每目标均独立计费，不用去重或其他盘空闲空间冒充本盘容量。
- 实际 I/O/fsync 可能阻塞，取消检查点不构成任意故障下的硬关机承诺。
- 仍只支持完整固定 Genesis、单 primary 祖先链、替换一条原末端及明确选择的一至八个全副本目标；复杂旧分叉、异盘父子合流与外来高等级一次完整链正式接纳均未完成。

**最终活动切换仍未完成**：新的有效多下级 Config/CurrentRegistry、首笔 native Candidate/Publication、对应代际 journal/封印、serving locator、完整新绑定历史数据库、未来备份策略延续、目标 writer 和源围栏安全解除都还需要共同闭合。旧正文、旧数据库和全部永久收据不删，源池仍可能停写，已作 Backup 选择后不能假装可任意改作 Abort。不能把 `GenerationAuthorityPrepared` 显示成迁移成功或 H03 外来盘接纳成功。


<a id="backup-first-candidate"></a>

## 首笔原生 Candidate 与完整 State

`membership_transition/backup/generation/first_candidate.rs` 从原决定自己的严格证书、完整源 State 和共同整单位路由投影生成真正的 `membership-generation-candidate`。它不接受任意调用方提供的最终 State，也不把普通 Add 的空版本 Candidate 改名为 Backup：

- 固定首笔序列为原 `MembershipFence + 1`，前快照仍是原已发布边界；原 operation、principal、请求摘要、generation identity、事务 ID 和所有原元数据成员确定不变。
- 先用与 `GenerationInputsPrepared` 相同的投影重建全部路由，重新核原 authority 绑定的 routing snapshot；然后仅推进 sequence，并在完整 State 加入该操作的确定性结果模板及原主体。拓扑只推进一次，业务代数、旧 receipts/events、HEAD、D/undo、退役/终止证明、数据库相关图和私有原生字段不改。
- 每个 AVAILABLE 保留版本都进入 Candidate 的真实 `versions` 向量，携带完整新 route、binding 和 protection obligations。PURGED/MIGRATED/RETIRED 不虚构正文，它们的描述、处置和永久证明继续存在于 full-State。版本向量不是已完成复制的证书。
- 沿用真实 native snapshot 分页、history-root catalog 和 candidate 编码，保留所有对象依赖。原元数据成员集合不由目标数替换或缩小；新隔离目标不因存有这些文件自动获得 voter 身份。

结果模板中的 `PUBLISHED / TARGET_MET` 仅处于**尚未发布的 Candidate 快照**，是未来发布流程必须满足的预定结果，不写入活动 State、不返回为 `OperationResult`、不追加日志。只有将来真实完成全部原成员 ACCEPT/FINALIZE/PUBLICATION 及逐版本本盘证明后，才可采纳该结果；当前所有活动 Backup 转换仍拒绝，不存在采用这个模板的成功入口。

### 后台持久化与冷态读取

原 registry/placement worker 在全部 `GenerationAuthorityPrepared` 后继续写入 `backup-generation-first-candidate/`。所有目标先各自重放实际 source archive、核验实际 native authority 和完整 published State，得到相同 Candidate/manifest，并各自满足容量预留，才开始无覆盖安装。每个目标用持有的目录 FD 读回、严格 native Candidate 解码、逐摘要解析完整 snapshot，并走完整 State 语义准入；所有目标成功后才写 `generation-first-candidate-prepared.json`。诊断阶段为 `GenerationFirstCandidatePrepared`，不是已发布或可写。

`BackupDecidedTargets::inspect_first_generation_candidate` 只读重建并核验该整批材料。`resume_first_candidate_preparation` 是显式有写操作，只补同一原任务缺失的隔离文件；独立持有的原 decision hash、完整目标集合及原物理绑定仍是前提。它不打开原源盘/SYSTEM、不发新选择、不把另一目标的 State 填入损坏目标。未知文件、不同字节、失联、容量不足或任一原证明不完整时，保留原物和明确失败。

### 预算与未完成边界

每目标新增 Candidate + native snapshot/history 闭包最多 8,192 对象、128 MiB；manifest/completion 各最多 4 MiB；单对象仍遵守原 native metadata 上限。真实读取按每次 decode/比较计费，包括重复引用，上限 24,576 次/384 MiB。安装还逐目标保留原 material store 的 16 MiB 及目录/原子临时文件预算。失败保留合法原子临时文件但不把它们作为证明，不清理旧源或旧 receipt。

**当前仍缺真实 Publication、有效 fanout Config/CurrentRegistry、最终绑定数据库/serving locator、未来放置策略、目标 writer 与源围栏解除。** 此片不写 ACCEPT/FINALIZE/PUBLICATION，不安装 generation marker/seal/journal，不开普通业务写者，不接 helper/正常配置自动执行。不能称整体迁移完成。按用户指示未审查、编译、测试、静态检查或 CI，所有新代码仅为未验证实现。
