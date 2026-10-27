# 完整巡检：已准入发布闭包与必需正文副本

提供独占 `Engine::scrub(&mut self, &IntegrityScrubCancellation)` 和锁外
`prepare_integrity_scrub → plan.run → finish_integrity_scrub`，不替换
`Engine::check` / `offline_check` 的磁盘目录诊断，不削减普通发布、读取、恢复、
离线导出的任何现有校验。没有启用旧依赖免读、后台健康缓存或自动修复。

## 检查范围与可信锚

- 逐个配置元数据成员读取并核验保存的成员配置、真实 journal 帧校验和、规范化编码、
  前驱链及序号；独占扫描最终 frame 必须等于此 Engine 已准入的 writer tail。
  锁外扫描到捕获的准确 tail 后只做一次真实 1 byte EOF/extra 探测，
  不追逐之后可能并发追加的完整帧；这一个字节也纳入实际计费和取消边界。
  哈希收据不是签名，单有一条自洽磁盘链不构成权威。未在当前 Engine 准入的成员
  返回明确 `RecoveryRequired`，不能凭重新出现的同路径自签健康证明。
- 对当前 State 序号以内每个 PUBLICATION，读取确切 candidate、publication、
  独立 finalize 对象和本成员 seal；执行现有全部成员 ACCEPT/FINALIZE/PUBLICATION
  收据验证，并把本成员 receipt 绑定到刚读取的实际 journal frame。
  候选的 `previous_snapshot` 依次连接，最终 snapshot/sequence 必须等于当前已准入根。
- 遍历这些候选绑定的每个 snapshot 页和每个 history root/index/page。每个唯一
  `(用途域, hash)` 在**每个成员、本次扫描**真实读取一次并校验域/hash/对象结构。
  Snapshot 字段分类复用 writer 的 `state_map_fields`；history root 复用现有 schema，
  radix 节点与完整解析器共用 `metadata_map` 的 index/leaf/routing 检查。
  对已由上述准确根锚定的原有业务语义不再为每次历史发布重建完整 State。
- 当前恢复代码对每个已发布候选保留并读取准确 snapshot/history 依赖；checkpoint
  也要求保留历史修复材料。当前没有删除这些已发布 metadata 对象的 GC 入口，
  所以这是真实恢复保留闭包，不是把所有目录文件都当作必需对象。
  将来增加 metadata GC 必须同时修改恢复和此保留根选择，不能只跳过缺失页。
- 当前 State 的每个 AVAILABLE 版本，每条 required route 都独立验证实际
  `version.json`、本地发布证明、checksums 和整个正文（逐块、完整 hash、EOF、结束长度）。
  一份好副本不能掩盖其它必需副本丢失；稀疏缺省 AVAILABLE 保持现有已准入语义。
- PURGED/MIGRATED 不要求正文；存在的本地版本目录验证保留 descriptor 与合法墓碑。
  已不存在的本地目录不制造新正文义务。未发布决定和 orphan/staging 目录不纳入
  已发布 State 的必需正文；报告单列 `unpublished_journal_records`。旧 `check`
  继续提供磁盘-only 目录诊断，不能把两者计数互换。

已确认永久丢失的 `PUBLISHED_DEGRADED` 是单独边界：先完成各成员 metadata
扫描，再从一个完整通过且绑定已准入 writer 锚的 recovery publication 取得精确
`(version, slot)` 不可用证据。仅匹配此证据、descriptor 仍正确且安全父目录下的
`publication-proof.json` 确实缺叶时，归为 payload unavailable；不得把任意 NotFound、
坏 descriptor、坏 proof 或别的版本缺 proof 降级。后续完整发布会撤销对应例外。
此证据最多保留 262,144 个 route，超限明确失败，不从 SQLite 或裸文件存在性推断。

`pending_file_versions` 不产生新的 own version/副本义务。原 source 版本的缺正文
仍会报告为 payload unavailable，但巡检不改变 `Engine.blocked`，不撤销 pending
分支 metadata-only 选择的既有例外。报告中的 metadata 损坏、成员身份失败与
payload availability 分开，消费者不得混为“一切写入都失效”。

## 并发与资源生命周期

独占命令仍整次保持 `&mut Engine`。在线消费者锁内调用
`prepare_integrity_scrub`，O(member) 捕获 `Arc<State>`、已准入 config、snapshot
和各 member 的准确 writer tail，不复制整棵 State、不做磁盘 I/O。
已有 detached commit preparation 由具体 `CommitPreparationBusy` 拒绝。
`estimated_payload_bytes` 在锁外计算 required payload 字节下界；不是 metadata、
proof、checksum 在内总 I/O 的伪精确估计。

`plan.run` 在锁外真实读取 metadata，继续复用唯一 schema/hash/closure 验证器。
当前无删除这些已发布 immutable metadata 的 runtime GC；将来添加时必须同时
补覆盖此次捕获闭包的真实 lease。不能只靠 State Arc 声称磁盘对象不删除。
每个版本通过消费者短锁回调 `admit_integrity_scrub_batch` 对照当前 descriptor、
placement 和 content state；变化则 `Superseded`，不是 metadata corruption。
同一 Engine/configuration 的身份用已有 read-lease registry 与 configuration
admission 的 Arc 身份绑定；重开 Engine 不能接受旧 plan 的结果。

每批只持一个版本 ReadLease，锁内解析当前物理 container 路径并打开 body fd，
因为 namespace rename 本来不受正文 read lease 禁止。打开之后的完整 proof、
checksum 清单与正文读取/hash、节流等待全部锁外。每批最多持 64 个 body fd，
route 超预算明确 `Failed(ResourceLimit)`，不会截断成健康完成或预开全池 fd。
真实 purge/split 已检查版本 ReadLease；长正文扫描会令对应操作 `ContentPinned`。
管理员可取消巡检，实际 read callback 返回并释放 batch 后才能重试，不承诺中断
卡住的 kernel I/O。已经 PURGED/MIGRATED 的墓碑允许现有幂等 cleanup 原子更新；
它不重新要求正文。结束由相同 live Engine `finish_integrity_scrub` 接受结果：锁内核验当前 writer
仍 ready，捕获的 writer 后来 uncertain/消失明确 Failed(RecoveryRequired)。
捕获时就无 anchor 的成员仍保留既有逐 member identity/metadata 失败（不可健康），
不伪造为新的 runtime 转换；若曾观察到 frontier 后
额外字节却 writer tail 未推进，报告 metadata LogCorrupt。当前 ready writer 已
推进时保留旧 baseline interval 语义，不把合法并发 append 误判成旧闭包损坏。

锁内 path discovery 的 namespace/owner marker 真实字节也计账：每批最多 4096
个 lookup/read 检查点；到 64 KiB 后下一 read 拒绝，连同最后一次至多 64 KiB 的
read，总 admission burst 小于 128 KiB。只累计实际字节，不在锁内调用限速等待；
出锁先发布已观察到的 open/length/layout 故障，再交付实际 marker 账单。其余
`on_read` 每次最多 64 KiB；即使读
过程中被取消，已读字节也先记账。普通读写保持原完整验证与原正文/帧 read 粒度，不把后台64 KiB节流
拆分强加到普通1 MiB正文块；普通 metadata/proof读取仍使用原非节流读取路径。procfs/sysfs 身份探测和目录枚举不计入 byte counter。
`on_progress` 在每帧、每副本和首个观察到的故障后发布真实部分计数；其
`Running` 永不健康，消费者不能在本轮已见故障后仍展示上次成功为 Fresh。
限速/周期属于管理消费者，core 不建假 scheduler、不缓存好坏标志替代实际校验。

每盘开始和整次结束重新核验实时 seal 与 device/inode/physical-device/UUID/binding。
Config Arc、State Arc 和 OS writer lock 都不是物理磁盘快照；managed owner 必须
持真实 Engine/OS locks 到全部 I/O 结束，不为此 clone 全部锁 fd。普通无关发布
允许与扫描交错，报告仍精确命名旧 baseline sequence/hash，不能冒充最新 State
完整健康。成功只说明 `started_at..finished_at` 中逐对象/逐副本观察；检查后的
外部改写、介质静默损坏没有永久保证。

- 串行扫描，一个 journal frame/metadata 对象/版本 batch 的 I/O 工作集；
  body fd 上限如上，不预开全池正文。
- frame 上限沿用 16 MiB、metadata 对象沿用 64 MiB、checksum 清单和正文块沿用
  现有格式限制。解析出的对象仍有常数倍分配开销，不能把编码上限冒充精确 RSS 上限。
- 每成员最多读取 262,144 个 metadata 对象，保留不超过该数量的用途/hash 与有界
  shape 摘要；不缓存完整 JSON、State 或每个历史根的全量展开。journal 流式解析，
  每成员最多 4,194,304 帧。超过预算明确 `Failed(ResourceLimit)`，不是健康完成，
  也不静默驱逐后反复重读导致无界二次扫描。
- 最多保留 128 个具体问题（各自保留 `Error` source），额外问题继续计数并报告
  `issues_omitted`。一个成员闭包失败后停止该成员的元数据遍历，继续其它成员与
  可验证的正文；不能把已失败成员未能展开的子树计为已验证。
- cancellation 是一向取消信号，实际在帧、对象、版本和正文块边界读取；不承诺
  中断已经进入内核的 I/O。取消/资源失败保留部分计数，终态分别为 Cancelled/Failed；binding 过期为 Superseded；
  `Complete` 仍可能带健康问题，只有 `is_healthy()` 才同时要求已完成且所有已定义检查成功。

计算量是 journal 字节、每成员唯一保留 metadata 对象字节、格式本身的 root 引用表
及 required payload 字节。旧 Snapshot/Candidate 根本身仍是全量引用表，这部分成本
没有消失；未宣称 O(delta) 巡检或发布。

## 修复与验证

此 API 没有写盘、State 发布、SQLite 重建、repair、GC 或 journal 尾修剪调用。
将来修复只能从已实际验证的好副本按原域/hash复制，并另行完成持久化协议；
不得从 State/SQLite 猜造新根或把巡检成功当作持久化收据。

根 `test/engine/unit/engine/integrity_scrub_tests.rs` 覆盖健康去重/必需副本、
State-only 与历史-only metadata 缺失/损坏、history root 丢失、全副本页丢失不修复、
等长正文损坏、proof/正文分类、pending missing-body 原例外、墓碑、稀疏 AVAILABLE、
取消、preparation fencing、真实身份替换边界、journal 锚与截断、悬空链接父目录和 issue 上限。
`degraded_recovery/tests.rs` 另覆盖真实降级恢复后合法缺 proof 的精确分类及其它版本的反例。
`integrity_scrub/metadata_tests.rs` 覆盖对象预算和单次扫描共享页去重。
这些是源码测试项目，执行结果以中央验证记录为准；本文不把静态存在当作已通过。

锁外测试还覆盖真实线程上的并发 publication 与固定 frontier、读回调不持 Engine
锁、purge-before-batch 的 Superseded、实际 purge/cleanup 在 ReadLease 活跃时拒绝且
取消后恢复、已打开 body 对真实 container rename 的耐受、最后副本读完后丢盘、
首故障及时进度、重开 owner 拒绝旧 outcome，以及实际 byte 计数/64 KiB burst 上界。

公开 acquire callback 返回的 batch 还绑定准确 State/config admission、Engine read
registry 和 version id；同 bytes 的其它版本/其它 owner 不能替换目标副本。已知 body
open 失败先发布 payload availability，后续 proof 也坏时额外报告 metadata，但不把
同一副本再次计为 unverified；合法 degraded 缺 proof 同样不重复计 payload failure。
取消从真实对象 read 传出时保留 OperationAborted，不能套 ObjectCorrupt 上下文制造
损坏；已有 Failed 原因不被后续不同 owner 的 finish 覆盖。

额外覆盖静态未解释尾字节、1 byte 探测的实际计费/取消，以及 probe 后 writer
发生真实 PartialWrite 时 finish 拒绝健康；不靠 stat 长度替代实际读取。
