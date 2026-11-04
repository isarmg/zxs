# 尚未形成旧侧多数的 handoff：certified abort 设计

状态：**共享选择槽及 Abort 历史只读解析已有源码；生产取消、取消首发布和恢复写仍未实现，未允许部署**。原设计只读基线 `3305c192`，2026-11-04；各源码阶段边界见第 9、10 节。本机未运行构建或测试。范围只限 pure-add；不修改原 Candidate、TransitionIntent、FrozenArchive、旧 journal 字节，不增加旧版本兼容/迁移。

## 1. 结论及真正边界

采用**唯一 AbortCore + 任意固定旧多数的独立 witness**，不把具体投票子集哈希进 generation identity。

- 旧成员集合 O、N、Q=floor(N/2)+1 来自 intent.origin.config，在线数、可用探针、新成员都不能改 Q。
- 每旧成员、每 intent 只有一个不可覆盖 decision slot。slot 只可选择 Joint 或 AbortCore G，两者不能分到两个可共存文件、按 proposal/证书/取消请求另开目录。
- 取得 Q 个真实旧成员的持久 Abort(G) 可认证取消。尚无 JointQ **不是充分条件**：N=4/Q=3、已有2票 Joint，剩余2票仍不足 AbortQ。
- 已有 JointQ，不论新Q、certificate 或首 publication 是否完成，都不能取消。目标在此以后永久失效仍可能阻断，本设计不能宣称闭合任意时刻的永久冻结。
- AbortQ 否决原 intent，但不抹除旧 fence。新 authority 的有效集合为原旧成员集，在**新 generation journal** 发布唯一 cancellation Candidate 后才恢复普通写。
- 取消证书不依赖新增根，也不重新跟随被否决 target 的路径。旧 fence/FrozenArchive 的既有安全要求不降低。
- witness 不预先固定投票子集：某个已投 Abort 的旧根失联，只要其它可用旧根总数仍能形成同 G 的 Q，即可补充不同成员的同 G 证据。旧成员变动、计划改投不是所需操作。
- 原新增根上的 reservation、pending seal、新盘投票等保留。清理/重用是另一个明确操作，不能以取消名义删除。

**可用性分层**：已有完整 F 时，认证 Abort 只需 Q 个合格旧根及 F 的可验证副本，不人为要求所有旧根可用。尚未形成 F 时，当前完整归档实现要求全部旧成员精确 journal；本轮不能靠多数推断缺失归档。取消代的首 publication 仍遵守现系统全部配置成员及数据/工作区恢复要求，AbortQ 不是「全部旧资源已恢复」。因此可以先报告 AbortCertified，再保持写阻塞等待旧资源；这不是固定 voter 子集引入的额外永久依赖。

## 2. 固定基线的精确源码落点

行号均相对 3305c192，用 `git show 3305c192:<path>` 复核。

| 文件/方法 | 当前事实 | 本设计需要的修改 |
|---|---|---|
| `membership_transition.rs:178` `begin_membership_handoff_with_effect` | newroot 预留先于 fence；intent 含原/目标、物理根、boundary/sequence | cancel 仅使用已承认 I；不重新准备/打开新增根，拒绝 effect |
| 同文件 `:331` `persist_membership_fence`、`:374` `admit_membership_fence` | fence 失败仍阻写；同 intent 重放；恢复拒绝冻结点后的原代决定 | 保留旧 fence，取消恢复检查统一 slot；不能直接清 membership_fence |
| `archive.rs:435` `inspect_membership_metadata_archive` | 全部旧成员完整 fence、精确 publication chain | 保留，不缩为任意 Q |
| `archive/persistent.rs:431,627` build/verify_current_fence | 全旧 journal 独立复制；检查 held source 与 fence 相等 | 可复用 F；认证路径在持有并验证完整 F 后不要求失败新增根 |
| `pending.rs:349,354` member_directory/admit_member_directory | `metadata/membership-pending/<I>` 需 enrollment 绑定 target/archive | 早期 abort 不能依赖该尚未形成的目录；统一 decision 独立于 enrollment |
| `pending/joint/store.rs:177` `JointStore::open` | 当前 joint 子目录 binding 绑 proposal/identity | 投票迁至统一 DecisionStore；joint 只承载 proposal/依赖/certificate，不再保留第二种可授权 vote 路由 |
| `pending/joint.rs:376` `joint_decision` | 检查全部 control，逐个写 Joint singleton | 正常写/恢复先看旧 decision，尊重部分 Abort，不能用现 status 缓存替代读回 |
| `journal/pending.rs:44,109`；`journal.rs:147,169,554` | Joint namespace 只收 JointAccept；singleton 禁第二帧；同步后收据 | 选定方案用原子不可覆盖 DecisionRecord 替换 Joint 投票 singleton；bootstrap/active journal 不变 |
| `pending/joint/store.rs:585` `verify_certificate_with` | pure-add 全目标票 + 独立 oldQ/newQ | 新协议 Joint 证据必须来自共享槽，不能承认另一份旧式 journal 投票 |
| `activation/authority.rs:7,98,252,384` | node 只含 joint/archive，hash 包含整个 node；lineage/verify_candidate 使用 intent.target | 拆 **core 与 evidence**，显式 Add/AbortedAdd outcome；按 effective_config 链接 |
| `activation.rs:22,72,112,177` | Generation、fence+1、membership-add 永久结果、prescribed first candidate | 同 operation_id + 原 I digest 的永久 membership-add-cancelled；使用新 G/旧有效集合 |
| `activation/publication.rs:4,202,315,455` | marker→推广→新代日志→首 publication；失败 blocked | 同 G 的不同有效 witness 不冲突；取消只推广旧根；少数 Joint 不覆盖 |
| `activation/recovery.rs` discover/admit_discovered/restore_anchor/verify_root_membership | Config 非 authority；读取 portable/head/local journal | 选择 core G，验证 detached evidence，按 effective_config，不访问已取消 target，不回退旧代写 |
| `archive.rs:75` `FrozenEpoch::read`、`activation/local.rs` | predecessor.target、portable load、精确历史 inclusion | predecessor.effective_config；增加 evidence resolver，保留原 Candidate 精确纳入证明 |

上述路径均位于 `src/engine/src/engine/membership_transition/`，首两行父文件另计。`disk_registry/**`、`disk_lifecycle/**` 不改；受限 discovery 的并行作者需要后续对接 outcome/core API，不能在本轮跨区修改。

### 当前 pure-add 有无「不同多数证书产生不同 G」漏洞？

本次限定审阅中**没有发现该漏洞存在于当前可激活 pure-add**，依据明确：

- `pending/joint/store.rs:603`：effect.is_none() 时要求 `votes.len()==verified.dependencies.len()`；每票身份/acceptance/真实 singleton 语义确定；完整成员集合与 canonical map 使证书确定，不是任意多数子集。
- 同函数随后仍独立验证原Q和目标Q，不因全集省掉分母约束。
- ExitControl 的 effect!=None 可仅含双多数，但 `activation/authority.rs:99` 的 AuthorityNode::intent 显式拒绝 effect，尚不能创建生命周期 generation。
- 因此不能把「任意多数导致 G 分叉」错误地报告为当前 pure-add 漏洞；它是新 abort/未来 exit 激活**若直接复用当前完整 node 哈希**会引入的安全反例。未来 exit 也须采用稳定 core，不能拿可变 exit certificate 的哈希当 generation ID。

## 3. Schema 和无环依赖

用已有 canonical JSON、BLAKE3 域分离 object_hash、有界严格解析和 typed identity，不使用签名字符串。

### 不变身份和 core

- `I=H("membership-transition",原 TransitionIntent)`；原字节不改。
- `F=H("membership-control-archive",原 ArchiveRoot)`；其完整原日志、Candidate、snapshot/history 闭包不改。F 引 I，不引本次取消。
- `AbortCore {format:"membership-abort-generation",transition_hash:I,archive:ArchiveRoot}`。
- `G=H("membership-generation",AbortCore)`。
  - old config、predecessor、fence/boundary、operation、actor 从 I 严格派生；有效配置严格等于 I.origin.config，不能调用方指定。
  - 不包含 vote 子集、certificate hash、online count、取消时间、随机数或 reason。
  - 同 I 的精确 F 来自固定原 journal 字节、固定 publication closure 和确定 map 编码。若出现不同 F，不能作为同 G 继续投；必须报告 exact-frozen-archive 冲突，不做语义近似。
- 当前 pure-add 的 **core 仍是已有 AuthorityNode{joint,archive} 原始字节**，旧 G 不重算、不增加 wrapper。新增 AbortCore 是当前模型中明确的新 outcome，不是旧格式迁移/兼容别名。

### 成员选择和 witness

- `AbortAcceptance {core_hash:G,identity,original_seal_hash,original_fence:Receipt}`，`X_i=H("membership-abort-acceptance",...)`。
- `DecisionRecord {format:"fenced-membership-decision",transition_hash:I,identity,choice}`，
  `choice=Joint{proposal_hash:P,acceptance_hash:J_i}|Abort{core_hash:G,acceptance_hash:X_i}`。
- `D_i=H("membership-decision",DecisionRecord)`；`DecisionReceipt{identity,record_hash:D_i,record}`。
- `AbortCertificate {format:"fenced-handoff-abort-certificate",core_hash:G,votes:Reference}`；`C=H("membership-abort-certificate",...)`。
  - 对任意至少Q个不同、完整原成员身份的有效同G vote 验证；voters 必须是 O 的真子集/全集，不能重复、重标、混新成员、按 online 降Q。
  - 同G可以有 C1≠C2；这是合法证据差异，不是 authority 分叉。
- `PortableAbortEvidence {core_hash:G,certificate_hash:C,objects:exact referenced closure}`；删除无关对象、缺页不靠另一源掩盖。
- `AuthorityEntry {core:VerifiedAuthorityCore,evidence:AuthorityEvidence}`；evidence 为 AddEmbedded 或 Abort(PortableAbortEvidence)。这是内存/portable envelope，不是计算 G 的对象。
- `PortableAuthority` 持有 head core、完整 predecessor core map、所有 Abort core 的 evidence map。evidence map 必须恰覆盖需要证据的 core 集；已有 Add core 原内嵌证明仍按原规则验证。

依赖为：历史 predecessor/config/Candidate → I → F → AbortCore/G → X_i → D_i → votes map → C → portable evidence/envelope。当前 G 不引用 C；F 只能包含严格祖先的 evidence，不能含本 G，因此无环。

**不能只改 PortableAuthority::hash()**：通用 `AuthenticatedMetadataObject` 必须继续对实际 core 的全部 canonical bytes 做 G 认证，不能让它认证含可变 witness 的 envelope 后伪称等于 G。`node_objects()` 输出确切 core 对象；evidence 用独立类型/解析通道处理。

## 4. 独立 witness 的实际持久化与恢复落点

不把「core/witness 分离」留成抽象口号：

1. `objects/membership-generation/<G>.json` 仍是**内容寻址 core**。Add 保留原 node；Abort 存上述 AbortCore。此物不可覆盖。
2. 现每盘 `metadata/membership-generations/<G>/authority.json` 和 `metadata/pool-membership/generation.json` 存**完整 portable envelope**。envelope 可含不同合法 C，但解析后 head core 都必须等于 G。
3. 新 `install_authority_envelope`：路径不存在时 no-clobber 安装；已存在时解析完整验证其 head G/lineage/closure。
   - 同G且证据有效：保留已在盘上的原字节，重新文件+目录同步；不能因 incoming C 不同返回 ObjectCollision，也不为同步 witness 无谓覆盖。
   - 不同G：只允许 existing head 是 incoming 的精确 predecessor 的现有 marker 前进规则；冲突/损坏不覆盖、不降级。
   - create 结果不确定：重开本路径判断上述语义；不盲重试 replace。
4. `PortableAuthority::load(hash,load_object)` 必须改成显式双能力：`load_core(G)`（内容寻址）和 `resolve_evidence(G)`（有界证据读取与验证）。它不能随意枚举或跟随任意路径。
5. **FrozenArchive 中的 detached 证据**使用与 objects 分开的确定命名文件 `authority-evidence/<G>.json`。该叶按预期 core G 验证 portable witness，不冒充「文件内容散列为G」。
   - 需要哪些叶由精确 ancestor core 链派生；每个 Abort ancestor 必须有完整有效证据；无关叶拒绝，缺失/损坏明确失败。
   - `ArchiveStore::admit` 的目录准入显式允许这个新证据子目录；`ControlSource` 增加 `predecessor_evidence(G)`；LiveSource 从 held current portable 取，ArchivedSource 从 held archive 取；`FrozenEpoch::read` 调用双能力 load。
   - build/copy/verify_generation_archive_observed/install_generation_objects 都遍历同一依赖链，独立复制 evidence 及其内容寻址闭包，并仅读目标重验。不能只复制 core 而漏证据。
   - 证据文件故意不是 F 中的一个可变证书散列：同G的有效 witness 可替换选择，不改 core 身份，也不改已有原 journal/Candidate/F 字节。它是由 core G 和多数证据规则约束的**可独立验证证明**。复制/本地安装保留先前有效字节，不主动修改被冻结源归档。
   - 原来的 FrozenArchive ROOT schema 和原始已哈希对象不用重写；只有含 Abort 祖先的新归档才有此证明依赖。当前 Genesis/Add-only archive 不需要无用 evidence 文件。这是语义分支，不是旧版迁移。
6. `effective_config()` 与源 `intent()` 分离。lineage、candidate、restore、local proof、workspace root aliases、data_dependency 全部按 outcome 选择有效集合；源 intent.target 保留作被否决计划历史，**不作为可访问路径或成员授权**。
7. 同G的两个有效 witness 在 head/predecessor 比较中必须等价；禁止在恢复时按 envelope bytes、C 或 witness成员数排序选「更新代」。后一代只由 core 的 predecessor 链确定。

此方案的必要成本是 portable/load/archive resolver 的明确分离，不能为了少改几处而永久依赖第一批 Q 个投票根。独立证明损坏/丢失可从仍存在的真实同G slots 重组 C2，但只在显式恢复路径写入；只读 discovery 永不修复。

## 5. 同一 decision slot、持久屏障和并发

建议新增 `membership_transition/decision.rs`，由 Joint/Abort 共同调用。统一路径 `metadata/membership-decisions/<I>/decision.json`，路径不得包含 P/G/C/outcome。pending control、certificate 与状态缓存都不是第二投票槽。

1. 持有真实根、既有 writer lock 与同进程排他访问，验证 root/lock inode、pool/disk/slot/binding_generation/physical identity。以持有目录 fd 操作；新盘不得冒充旧成员。
2. 打开/创建中性目录 binding，只绑定 I+identity，不先选择 Joint/Abort。严格扫描完整 slot；损坏/截断/未知格式/换名不是 Empty，不修成空槽。
3. 安装并验证 core G、acceptance X_i 与独立完整 F 的所需本地闭包，完成对象文件及目录同步。
4. 使用 `AtomicFile::stage(...).prepare(bytes)?.create()`：完整临时文件同步 → no-replace rename → 父目录同步。no-replace 是选择线性化点，成功目录同步后才可发 receipt；重新验证持有目录与命名 leaf 身份。
5. 相同 choice 重试必须再次 file+dir sync；不同 choice 明确冲突。`Unknown/Published` 不是未写入，要重开确切命名位置；临时文件不算票、不提升为证书。
6. 不调用 PendingJournal::repair_torn_tail 截断已命名 decision；原子安装后损坏是完整性失败。新 Joint/Abort 只能认共享槽证据，不能接纳原旧式 joint journal 的另一个投票通道；不做旧格式升级/迁移。
7. 单个 Abort(G) 不是可任意撤回草稿。正常 Joint 的准备/写/恢复在任一新票前读并尊重旧成员中已见 Abort，即便未达Q。取消可不断从其它合格空槽补同G，不能改核心 G。已有 Joint 的成员永久拒绝 Abort，但其它成员可组成独立 AbortQ。
8. 现纯新增需要全部旧 control/持根；同一 Engine 的 &mut API 与所有实际旧根 writer locks 防止联合协调器竞态。新增 abort 认证只需Q可用根时，先取得并核验全部拟用Q根的锁再投票，持锁期间其它 JointQ 不可能绕过交集；崩溃后的共同槽保证结果不会二投。

原语证据：`src/platform/src/fs_safety/unix.rs:489,517,570` 是 prepare/file sync、no-replace create、parent sync 和 NotPublished/Unknown/Published；`src/engine/src/disk.rs:295` 已有同 immutable bytes 重试的文件/目录同步。

模型假设与当前多数协议一致：成功 fsync 的数据不自行回滚，成员非 Byzantine，真实 root 身份持续有效。哈希不是签名；调用者任意拼 receipt 不能当实盘投票，在线聚合只接收受控写入/读回与屏障形成的内部证明。人为清空/回滚根不在 crash-stop 证明内；遇损坏必须失败保留。

## 6. 安全证明与未采纳方案

任意 Joint 旧多数 J 与 Abort 多数 A 均为 O 的子集，`|J∩A|≥2Q-N≥1`，交点成员同 I 唯一 slot 不可能同时投 Joint/Abort。不同 AbortCore 的两组多数同理不可能同时成立。同G的 C1/C2不同不改变 G，不会出现两套可写 generation journal。

少数 Joint 可以原样保留：N=5/Q=3，1、2为Joint，3、4、5为Abort(G)合法。推广1、2时验证全证书、自己旧fence/身份/归档，不覆盖本盘旧Joint。旧 generation仍fenced；新 authority使用旧成员集、新G、唯一首publication。

**反例一：单独 abort 文件**。A/B 已Joint，B/C另写Abort：两个多数合法且共同B双投。禁止。

**反例二：证书全部哈希进G**。N=3、三盘Abort同值，证书{1,2}与{2,3}都有效，完整node产生G1/G2。两套日志可分别写入。按「当前最小Q」选择也会随恢复证据改变。必须core/witness分离。

**未采纳备选：提前固定恰好Q个voter**。它可让完整证书字节唯一，但投票成员1、2已固定后2永久不可用，1、3仍是旧多数也不能重组。这样人为制造永久冻结，不符合目标。只能留为模型中的反例/实验，不能作为本轮最终合同或暂时上线接口。

## 7. 故障和恢复矩阵

| 证据/故障点 | 允许动作 | 禁止或写状态 |
|---|---|---|
| 只有 preparation、尚无已承认 I | 保留准备记录，本设计不靠它伪造取消代 | 不删除reservation、不宣称取消完成 |
| 部分旧fence，newroot永久丢失 | 从原I只用旧根补同fence，建立完整F | 不重开新根；旧归档不足保持阻塞 |
| F完整、pending控制未齐/新根失效 | 仅旧根与F认证Abort | 不调用enroll/check_joint_controls，不缩Q |
| 旧Joint少于Q且有Q个可用空槽或同G Abort | 同G补证据，可跨原witness集合 | 不改G、不改已有Joint |
| 旧Joint已Q或其余合格票不足Q | 拒绝并报告精确固定N/Q/已见票 | 「目标尚未发布」不是取消权限 |
| staging/file sync失败 | 无receipt，重开命名slot判断 | temp不算票 |
| rename/dir sync失败、回复丢失 | 同choice重同步；崩溃未保留且未发durable receipt时可重写 | 检查前不换choice |
| 命名decision损坏/截断 | 明确错误、保留全部字节 | 不截断清空成Empty |
| 部分Abort后某投票根丢失 | 从其它旧空槽补同G，只要固定Q仍可成立 | 不绑定原证书那批根、不改G |
| AbortQ成立、certificate未写完 | 从真实slots组成C1或C2，均证明G | 没完成证书文件不代表Abort可撤回 |
| C完成、marker前崩溃 | 原代保持fenced，显式继续同G | 不能直接清blocked |
| marker/seal推广部分完成 | 验证任一有效同G envelope，完成旧根推广 | 不因witness变化报代分叉，不访问取消target |
| 首cancellation Candidate/Accept/Publication失败 | 恢复唯一G/唯一prescribed candidate | 普通写保持阻塞，不伪称取消已Published |
| publication完成、回复/系统config丢失 | roots/core/evidence/archive重建，重放原op取消结果 | 原op不能重新新增 |
| 失效newroot以后出现 | 由权威链判断原I已取消，继续pending/不可写 | 不独立激活旧target、不自动清理 |
| 下一次新增 | 新op/new I，origin predecessor=取消G | 保留旧候选/旧archive/原取消结果 |

取消首结果必须 `operation_id=I.operation_id`、`request_digest=I`、sequence=fence+1、kind=`membership-add-cancelled`，保存永久幂等收据。metadata authority epoch前进；数据路由、正文、原Candidate不改变。topology_generation如何表示取消代应在单一prescribed next_state合同中固定，不能由恢复时在线根数决定。为保持现generation边界失效语义，建议和当前activation一致做一次checked increment，但不把它解释为成员数量改变。

## 8. 最小实施阶段与验收

1. **共享DecisionStore+Joint接线**：类型、唯一命名槽、atomic屏障、严格冲突、真实本盘证明，所有Joint读写/恢复只走同槽。还没有取消能力。
2. **稳定AbortCore+独立certificate**：只pure-add；完整F、固定oldQ，可追加不同旧成员的同G票；部分Abort veto正常Joint；明确AbortCertified≠已恢复写。
3. **core/evidence持久化与portable/归档resolver**：当前pure-add旧core字节不改；同G的C差异可恢复；node_objects与证据分离、所有ancestor证据独立闭包。先覆盖缺证据/损坏/等价不同C。
4. **取消新代+prescribed首publication**：effective_config贯穿lineage/restore/local/frozen验证；取消仅持旧根、保留少数Joint，永久原op结果；全部旧资源就绪后才恢复写。
5. **GitHub测试**：纯模型→真实临时目录fault-injection→engine集成；genesis→abort、add→abort、abort→add、系统目录丢失/根地址重绑、不同Q witness同G、证人掉线后新Q重组、旧minority Joint、新目标永久不可用、恢复不访问取消target。未授权真实盘/部署，不做物理掉电。

模型 `test/models/membership_handoff_abort_model.py` 仅Python标准库；未来GitHub命令：`python3 -m unittest discover -s test/models -p 'membership_handoff_abort_model.py' -v`。本轮未运行模型、Rust构建或测试，不把静态设计当集成验证。

成本：旧全历史archive成本不变；决策扫描O(N)，witness分页O(Q至N)，单票依赖对象既有同步+staged file sync+parent dir sync，重试保持file+dir屏障。新证据resolver/复制遍历与ancestor数及证明闭包线性；不进入业务数据热路径、不依靠缓存省略真实边界验证。

## 9. 第一阶段源码：共享槽，尚无生产取消能力（2026-11-04）

本节记录后续源码批次的实现边界，不把本文模型当作已执行的证明。新增 `membership_transition/decision.rs`，每个实际成员根只有 `metadata/membership-decisions/<I>/decision.json` 一个选择名；`binding.json` 只绑定 I 和完整 `MemberIdentity`，不把 P、G、outcome 或在线成员数放进目录。空中性目录／精确 binding 临时前缀可恢复，已命名但缺 binding、损坏、未知格式或截断的 decision 都失败保留。

- Joint 的完整临时记录经 `AtomicFile::stage().prepare().create()`，先文件同步、NOREPLACE 发布、父目录同步。发布结果不确定或冲突必须重新读实际命名槽；同 choice 重试及已见的同 choice 都重新 file+dir 同步。确认后再次核验根、目录、binding、命名文件 inode 和完整记录，才返回内部 receipt。
- Joint 在任何新票前检查全部实际参与者的共享槽，任一可见 Abort 都否决继续 Joint，包括不足旧 Q 的部分 Abort；原始 O/Q 仍来自认证 origin，现 pure-add 的完整目标票和独立 oldQ/newQ 规则未放宽。
- `JointVote.decision` 内嵌完整 `DecisionReceipt {record_hash, record}`，且接受项仍绑定 P、enrollment、bootstrap 和真实 control receipt。独立 certificate map／portable authority／完整 ancestor archive 原有闭包自然携带该记录，不依赖源根外带一个未复制的 decision 文件。portable 认证不替代实际本盘槽：同步验证、activation、重启恢复和只读受限 discovery 都重新核对自己的真实选择；全员最终检查再核对命名记录，缓存 status 不算票。
- 新实验 proposal/certificate format 为 `fenced-joint-slot-decision`、`fenced-device-slot-decision`、`fenced-exit-control-slot-decision`。旧 `joint/journal.bin` 无论完整、空或损坏都明确拒绝，不转换、不截断、不删除；旧 Joint singleton writer 仅留 `cfg(test)` 反例入口，生产不再调用。旧 proposal/certificate 不能通过新 schema。现有用户数据、原 Candidate、TransitionIntent 和 FrozenArchive 字节未重写，未支持的实验成员证据只拒绝访问，不重置池。
- 对外观察字段 `MembershipJointVote.frame_hash` 改为 `decision_hash`，其语义是 domain-separated `membership-decision` 哈希，不再伪称 journal frame。
- Abort 只有解析识别和测试构造。生产没有 AbortCore 构造、固定旧 Q 聚合、独立 witness、取消 generation、恢复写或取消成功 API；第 8 节第二至四阶段仍未完成，exit 激活也未开放。

上述条目描述第一阶段冻结边界；后续只读部分见第 10 节，不能据此推断已有取消写入流程。

新增／扩充测试源码覆盖 Joint/Abort 与不同值的所有前后选择、真实竞争、完整／部分 staging 不算票、NOREPLACE 已准备对象冲突、结果丢失、Unknown/Published 读回及再次同步失败、同选择重试屏障、损坏保留、身份/intent/binding 错误、旧 singleton 无第二授权路、部分 Abort 全员 veto、实际 Joint 成功/部分完成/系统目录丢失/重启、portable 与实际槽区分、最终命名记录改变、受限恢复不修复坏票。故障注入是组件边界模拟，不是物理掉电：AtomicFile 自身的 prepare fsync 和 rename/parent sync 仍使用既有平台原语；测试对实际 create 成功后模拟丢失的 syscall 回复，并要求真实读回与确认屏障。

开销限于成员控制路径：预扫 O(N)，一个成员一条不可覆盖选择；receipt 重试保留 file+dir 两道屏障，最终检查读取每个实际选择。原全历史 archive 成本和业务热路径未改变。仅固定 Rust 1.99 rustfmt 与 `git diff --check` 已执行；本机未运行 Cargo 编译/check/test/Clippy 或 Python 模型，运行验证须由 GitHub 对合批 commit 执行。

## 10. Abort 历史的实际只读闭包（2026-11-04）

此批把读取已完整证明的 Abort 祖先接入实际 `PortableAuthority::load`、`FrozenEpoch::read`、`verify_root_copying`、`archive_epochs`、本地 membership proof 和 archive scrub。不是生产 Abort 写协调器；`observe_vote`、`certify`、实际 Abort 选择与测试 authority 构造仍只在 `cfg(test)` 中。

- `AuthorityCore` 明确区分原 `AuthorityNode {joint,archive}` 与 `AbortCore`。Add 继续直接序列化原对象，已有 G、原 Add-only portable/chain 表示不变。两种 core 的严格字段集合不相交；Abort 必须匹配唯一 format，额外 `joint`、未知 format、缺证据不能降级成 Add。`node_objects()` 只输出 core，通用内容寻址认证仍覆盖 core 的全部 canonical bytes。
- 含 Abort 的 portable 使用明确的新 `membership-generation-evidence-chain` envelope；每个 Abort core 恰有一份 `AbortAuthorityEvidence`，Add 不得附证据，无关或缺失 key 拒绝。其 witness 保持原独立 certificate/acceptance/decision/map 闭包；另附 `PortableAbortArchive`，只携带原 I、F.members 的精确对象闭包以及每个原成员的原始 seal 字节。解析验证 I/F 绑定、精确成员数、seal 原字节散列和完整身份、对应原 fence receipt。它证明身份投影，不冒充完整 journal/publication 归档；完整 F 仍由持有的 archive 独立验证，不能靠另一个源补缺页。
- `AuthorityResolver` 显式区分 core-object 与 detached-evidence 能力。held archive 的 `authority-evidence/<G>.json` 由 G 对应的 core 和旧固定 Q 规则验证，不冒充文件内容散列为 G。完整归档检查从实际 predecessor 链派生精确文件集合，缺失、损坏、换绑或多余命名证据失败。复制时先复制并认证 core，再独立复制 envelope；目的地只用自己的文件重验。已有合法同 G 证据即使证书不同也保留原字节并重新 file+dir 同步，坏证据不覆盖。原 F 根 schema 和成员 journal 字节不变。
- Abort 的有效集合严格是 I.origin；lineage 链接、当前/祖先 Candidate、本地 proof、历史 workspace root aliases 和 data dependency 不从被否决 target 授权。`FrozenEpoch` 读取原 F 的 base 后，只在内存重建 `membership-add-cancelled` 的唯一首 Candidate，核对原 operation、actor、I digest、fence+1、topology+1、快照、history、transaction ID 和原成员集合；不会调用 Add 发布来制造这份历史。
- 当前 Abort 头在 `discover_unrestricted_legacy`、`admit_discovered`、`restore_anchor`、restricted marker admission、`Generation::result` 及需要 Joint 的资源调用处由 `add_joint()` 明确拒绝。legacy discovery 先解析 marker 后读 fence routing hint，坏 marker 直接失败；只有已完整验证的 lineage 能过滤明确被取消的 intent。取消头不会沿 rejected target 继续探测。这个 guard 不等于完成删除、离线 bootstrap、取消 activation 或永久冻结闭包。

新增测试源码覆盖同 G 不同合法证书的 core 字节等价、独立 resolver 能力、envelope 精确闭包、缺 seal/I/map page、未知/混合 schema、只读 prescribed cancellation Candidate、当前 Abort 头在 Add 恢复写入和 rejected-target 探测前拒绝、坏/缺 marker 不回退，以及**用显式测试 journal writer 构造的完整 Abort 后继冻结归档**。该归档走实际外部 archive 验证、`read_generation_base`、独立复制和 scrub 路径，覆盖所有 live roots 不可用、证书更换、缺/坏/额外/换绑证据、缺原 journal 或 publication object 时不能从仍健全的源 F 回退。原 Add→Abort 测试另保留 Add 原 core/portable 字节的比较。这些是待执行测试定义，不是已通过的运行证据，也不证明生产 Abort→Add 协调器存在。

成本与限制：portable envelope 有 MAX_METADATA 总字节上限；身份投影额外存储 O(原成员数+F.members 页)，witness 为 O(Q 至 N)。归档读取/复制保留原完整历史检查，新增按 ancestor G 命名的有界证据读，scrub 计入读取字节和证据叶预算。未增加业务写热路径或减少任何 fsync。源码机仅执行固定 rustfmt 与 `git diff --check`；Cargo/default/fault-injection/Clippy、测试计数和端到端故障验收必须在独立构建环境对准确提交运行。后续仍需生产旧槽聚合、同 G envelope 的 marker 安装、原根推广、取消首 publication 和恢复协调，才可能恢复普通写。
