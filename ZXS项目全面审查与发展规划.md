# ZXS 项目全面审查与发展规划

> 基线：2026-11-03，GitHub main `72e08db452804f89e91e3866922ee11dc9cb8c77`，源码树 `927bbc7f7354b2cd6f4b84fdd84c9686692174d2`。本规划自身和版本准备是基线之后的变化。文中“现状”指该基线，不因文件发布时间自动变成最新事实。

## 1. 目的与阅读边界

本文根据六个相互独立的审查方向整理项目当前能力、缺口和后续路线：存储引擎；Foundation；Web 和 API；SMB 与部署；性能与可观测性；测试、许可和发布。各方向阅读代码、文档及既有证据，部分做了定点验证。审查不等于逐行证明整个项目无缺陷；Foundation 体量尤其大，未检查的区域和未经复现的疑点均保持待核实状态。

事实、已复现问题、设计缺口、建议和现场验收分别标记。已有实现不重新列为从零开发，不以旧结果代替新提交的测试，不以模拟故障代替物理断电。先处理正确性、可恢复性和已确认的交付问题，再基于测量优化；不得为了提高数字删除数据完整性或用户授权边界。本文是计划，不代表所有项目已获得自动执行授权，也不自动重启先前暂停的工作。

## 2. 用户目标与运行边界

- 目标系统为 Debian 13 amd64 最小安装；ZXS 为一个内嵌 Web 的主程序 ELF；系统通过 apt 安装 Samba，使用与实际 Samba 包匹配的独立 ZXS VFS 模块。Debian 提供必要通用运行库。不能把此描述扩展成所有依赖均由 apt 动态链接：当前 SQLite 使用 bundled 配置。
- ZXS 程序、数据和日志按部署配置归于 `/zxs`；模板负责传入相应路径，裸 CLI 的相对路径默认值不是相同保证。默认监听 `0.0.0.0`，测试隔离使用回环不改变生产默认。
- 不强制 Btrfs，不以物理独立性准入限制替用户作决策；实际故障域、设备拓扑和备份责任仍须在使用说明中如实说明。旧格式及旧代码兼容性不作为必须目标。
- 目标配置：Intel Core Ultra 9 288V、32 GB 内存、1 TB 系统盘、12 块每块 15.6 TB 的企业级 E1.S 数据盘。主板、背板、PCIe switch、网卡、固件和实际拓扑未提供，不得据此承诺聚合性能。
- 管理界面的磁盘 UUID 与备注已实现；不要将它们误列为尚未开发。真实性能、断电及物理盘恢复须在专用测试环境验收。

## 3. 证据与当前发行状态

基线精确提交的 GitHub CI run 41641687665 共有 17 个任务成功，包括真实浏览器、Debian 单程序、Samba 和 SMB 等相关检查。这是该提交的 CI 结果，不是本规划或 1.0.0 版本变更的验收，也不是目标设备、真实断电或任意浏览器的证明。基线的安装候选为 0.1.1，不能改标为 1.0.0。发行工作流对新版本须独立构建、检验七项发行资产并记录其 SHA-256。

截至基线，公开仓库尚无 release 或 tag；活动 Web 依赖 `erased@0.1.2` 的独立许可／版权材料复核仍未关闭，具体事实和可接受的关闭路径见 `compliance/README.md`。不得把机器校验或本规划写成许可结论。目标设备相关验收和未验证的故障场景应作为明确限制保留。

## 4. 规划排序与交付方法

| 阶段 | 目的 | 主要内容 | 退出条件 |
| --- | --- | --- | --- |
| A：固定基线 | 避免错误承诺及重复工作 | 固定提交、版本、构建来源、真实未决项；核实许可资料和发行边界 | 候选、依赖和证据可追溯，缺失项显式登记 |
| B：正确性与可恢复性 | 先关闭有证据的问题 | 验收收据重跑误读；清理/拆分已发布后的错误分类与物理清理状态；文档和代码冲突 | 可复现回归变绿；发布后结果、错误和物理状态可区分 |
| C：协议和边界 | 以明确契约推进复杂项 | 有限读取租约、PURGE_PENDING、撤权和排空；管理资源路径边界；Foundation 公开能力候选按暂停范围管理 | 状态机、崩溃矩阵、并发测试和不变量齐备；不因 TTL 到期删除仍在使用的数据 |
| D：可观测与实测 | 找真实瓶颈 | 目标机拓扑、巡检预算、CPU、内存、锁、磁盘与网络度量；真实 SMB/历史/恢复负载 | 每项数字均有环境、工作量、提交、基线及可复现方法 |
| E：维护与体验 | 降低长期成本 | API 合同覆盖、国际化 key、草稿体验、测试隔离、lint 一致性 | 分阶段回归覆盖和兼容边界明确，不为形式统一降低测试敏感度 |
| F：实际验收 | 形成可交付证据 | 干净 Debian、精确 Samba 包、真实客户端、盘故障、恢复、长期负载和许可材料 | 记录通过、失败、未执行及限制，不能用单一 CI 绿灯代替 |

阶段可在依赖允许时并行：不在等待 CI 时闲置；共享文件和发布引用由单一集成者处理。避免为凑并发重复审查或重复运行同一提交同一验证。涉及真实生产盘、凭据、权限或不可恢复删除时，使用独立测试环境并遵循适用授权要求。

## 5. 当前重点与依赖图

1. **已复现的工程故障：验收收据残留。** 部署验收脚本在当前尝试失败前未移除旧成功 JSON；在同一目录重跑时存在误读。干净 CI job 仍由失败状态阻断，不能据此断言发布门禁已被绕过。先增加“成功后失败重跑”的回归，再修正结果生成及身份绑定。
2. **存储协议与结果分类。** purge 已提交后清理失败仍可能返回看似未提交的原始错误；需将操作发布、物理清理和各盘剩余状态分开。有限租约和持久化 PURGE_PENDING 应与新租约屏障、读排空、撤权及崩溃恢复作为一个协议设计，不做仅靠定时到期删除的捷径。
3. **文档与实现不一致。** 旧依赖免读已在具备真实巡检覆盖和谱系条件时启用，部分说明仍称未启用；SQLite 投影故障的阻塞行为、发行 VFS 资产说明也需对齐当前实现。逐项按来源链更新并加入可执行一致性检查。
4. **设备与巡检约束。** 288V 的原生 PCIe 资源不足以让 12 块 x4 盘全部独占直连，必须核对实际背板和共享上行。按当前默认 64 MiB/s，扫描 187.2 TB 正文的理论下限约 32.3 天，与 48 小时新鲜度不能直接匹配；实际应按已占副本字节和访问条件核算，不把理论满容量直接当实测。
5. **Web 与接口。** 现有 OpenAPI 为部分覆盖，静态盘点 94 个注册路径、47 个已列合同；这不是 47 个故障。外置资源路径的检查与后续打开之间有已记录的竞态边界；语言切换时普通未提交草稿的保留语义待明确。监测增量应先证明收益，现有通用摘要方案的合成实验反而增加流量。
6. **Foundation 范围和风险。** 宏、随机数转发和 ring buffer 的上一批确认问题已修复。SQLite collation 跨 FFI panic 和 regexp 转换 OOM 等候选须限定功能范围并复现；不得当作已证实当前产品漏洞。已暂停区域保留为规划，不得因本文自动重启。

详细工作包、代码位置、依赖和验收标准见以下六个审查章节。时间估算仅供资源规划，未经拆分和现场测量不得作为保证工期。


---

## 6. 专项审查与工作包

### ZXS 存储、持久化与恢复审阅及实施规划

审阅日期：2026-11-03。本文仅为规划与静态审阅，不恢复暂停任务，不修改产品源码、不启动验证矩阵、不部署。

#### 0. 基线和证据等级

- 阅读位置：`审查检出目录`。
- 本地 `HEAD=a50502f5b5073dd90ff88a1f3fcb39e08f9f24cf`，`HEAD^{tree}=927bbc7f7354b2cd6f4b84fdd84c9686692174d2`，审阅开始时工作区干净。
- GitHub connector 核实公开提交 `72e08db452804f89e91e3866922ee11dc9cb8c77` 存在；主协调者随后用 GitHub REST 核实其 tree 同为 `927bbc7f7354b2cd6f4b84fdd84c9686692174d2`。因此以下行号按同树公开 main 引用，不把两个不同 commit SHA 写成同一提交。
- **事实**：本次直接读到的代码、类型、测试断言和文档。**风险/建议**：由这些事实推导的使用风险和未来方案，不是已复现的数据丢失。**未验证**：本次未运行 Rust 测试、故障注入、模型搜索、性能测量、设备掉电或真实部署；仓库中的历史结果不自动成为当前树的执行证据。
- 优先级含义：P0 是继续宣称能力或做相关改动前必须先澄清的基线；P1 是应优先闭环的行为/恢复问题；P2 是独立能力或规模建设，不借本规划自动重新启动。

#### 1. 已有基础，应保留而非重写

1. **持久决定与可见性分离。** `src/engine/src/engine/publication.rs:150-321` 在最终冻结状态、磁盘 seal、metadata epoch 和可选复用许可复核后开始不可逆决定；`274-296` 设置 blocked，并把决定中的失败保留为 `IN_DOUBT`。完成后才安装 State、快照和历史根。`docs/10-transactions-and-recovery.md:238-256` 明确 FINALIZE、正文提交证明、PUBLICATION、本盘发布证明和 State 安装的屏障。
2. **固定配置的恢复门槛。** `src/engine/src/engine/recovery.rs:126-221` 对准入成员实扫 journal，`190-194` 使用配置的 quorum，不按在线数量降门槛；`523-571` 在缺成员时只展示经过完整 publication seal 核验的前缀并保持不可写。对应源码回归在 `test/engine/unit/engine/recovery/tests.rs:688,756,803,854,896`。
3. **准确摘要和原始证据是权威，SQLite 是投影。** `src/engine/src/engine/operations.rs:119-151` 已发布结果走永久收据，未决结果只沿候选绑定 owner/operation 页核对 operation、摘要和序号。`src/engine/src/engine/index.rs:612-639` 普通发布后的索引失败停用投影，不改写永久结果。
4. **读取验证的是实际字节。** `src/engine/src/engine/reads.rs:238-286` 锁内捕获版本、路由和租约；`353-409` 只接纳准确版本的证明与 fd。`src/server/src/verified_chunk.rs:43-88` 先核对完整校验块，再返回同一块已验证缓冲的切片，不在 hash 后重新读取未经验证字节；同版本副本可按块回退。
5. **持有磁盘根而非反复信任路径。** `docs/components/engine.md:31-43` 和 `src/engine/src/disk/owned.rs` 的 owner/FD 生命周期是普通 I/O 的边界。`src/engine/src/maintenance.rs:524-608` 在破坏性清理时再次核对实际目标 seal，从持有根写墓碑、删除正文并同步父目录。已有根替换、seal 变化、硬链接/符号链接拒绝测试，不能以“减少检查”移除。
6. **恢复 checkpoint 只是保守加速。** `src/engine/src/engine/recovery_checkpoint.rs:1-7,96-98` 和 `docs/architecture/verified-recovery-checkpoints.md:27-60` 保留完整 journal 扫描、覆盖依赖验证、全成员一致要求，损坏或超预算回退完整恢复，不转移发布权威。
7. **读租约异常失败关闭。** `src/engine/src/engine.rs:168-216` 的计数丢失/零值使 registry 失败，不能把异常当“无人读取”；测试在 `test/engine/unit/engine/reads/tests.rs:564,593,913`。新增期限机制必须保留这一安全性。

#### 2. S01 / P0：统一完整性和发布结果的真实能力基线

##### 事实

- 旧依赖免读**已经有条件接入**：`publication.rs:947-956` 从成功发布基线与真实 scrub 覆盖构造 `PublishedDependencyReuse`；`1067-1082` 有资格时只安装本次新编码 Snapshot 对象，无资格时走完整路径。最终准入仍在 `260-274`。`src/engine/src/engine/integrity_scrub/schedule.rs:70-168` 核对原 owner、admission generation、epoch、原始期限和基线。
- `docs/adr/0005-incremental-publication-integrity.md:3-25` 和 `docs/architecture/published-dependency-omission.md:9-43,66-81` 已说明这一事实及损坏发现时点后移。
- 但 `docs/10-transactions-and-recovery.md:78-82` 仍写“尚未启用”，`docs/architecture/integrity-scrub.md:3-6` 仍写“没有启用旧依赖免读”。这些不是同一能力的准确现状。
- `docs/19-capacity-and-performance.md:243` 仍写“SQL 投影失败…保持写阻塞”；实际普通发布 `publication.rs:298-321` 调 `update_index` 后解除 blocked，`index.rs:612-639` 只停用派生索引。启动重建失败、工作区完成失败则是另一种边界，见 `engine.rs:458-472`、`docs/10:248-252`。

##### 风险

读者可能误以为每次普通写入仍重新发现所有旧元数据损坏，或误把所有 SQLite 故障都视为业务未提交/全局不可写。错误的现状描述也会导致规划重复建设已存在的路径，或以缺失门禁证据冒充已放行。

##### 实施顺序与验收

1. 建一份“规范目标 / 当前调用路径 / 适用条件 / 精确测试与 CI 证据 / 尚未验收”矩阵，并从编号规范、ADR、组件和运行手册引用它。
2. 明确五类不同状态：发布前拒绝；ACCEPT 后未知；已发布但工作区核对失败；已发布但清理未完成；已发布且仅派生索引失效。不能统一 catch 为成功或未知。
3. 对免读路径列出真实调度 owner、完整成功、期限、失效、重启、pending 来源例外和缺少资格的完整回退路径；把仍写“未启用”的正文改成条件描述。保留历史验证文档原基线，不把历史文字全改写成当前结果。
4. 用当前树的 `omission_tests`、`schedule_tests`、索引故障、服务端周期巡检及最终准入测试建立执行证据，缺日志则标未验证。

验收：所有当前状态文档不互相矛盾；对同一故障，源码、错误接口、UI 文案及测试断言一致；报告仍明确“源码存在 ≠ 门禁通过 ≠ 性能收益 ≠ 物理耐久验收”。不以此工作减少任何校验或 fsync。

#### 3. S02 / P1：补齐 purge/split 发布后的结果与物理清理闭环

##### 事实

- `src/engine/src/maintenance.rs:323-339` 先发布 `ContentState::Purged` 和 `PlanState::Committed`，`357` 再执行 `cleanup_purged()?`。
- 清理错误直接返回原错误。`test/engine/unit/maintenance/cleanup_tests.rs:94-175` 明确断言 `commit_plan` 返回 `UnsafePath`，同时原 operation 已是 `Published`、plan 已 committed；`test/server/integration/http_engine/plan_lookup.rs:287-307` 也断言 HTTP 返回 `UNSAFE_PATH`，而 plan 为 committed。`src/server/src/engine_backend/error_registry.rs:546-558` 将该错误映射为 400。
- 已有 `PublishedWorkspaceReconciliation` 和 `PublishedRecoveryReconciliation`，但没有维护清理的等价上下文：`src/engine/src/error/published.rs:7-142`。当前 `src/server/src/engine_backend/wire.rs:12-23` 对已发布上下文的固定文案还包含“writes remain blocked”，不能不加区分地复用于清理残留。
- 原 ID 重试已经正确继续清理，不重复发布：`maintenance.rs:259-271`。这不是重复业务提交已被发现的缺陷。
- 目标 seal 不可用时 `maintenance.rs:571-582` 跳过该 slot，最终 `612` 仍 `Ok(())`；`cleanup_tests.rs:320-337,387-405` 明确验证可以返回 Published 而目标正文仍在。当前返回没有逐盘 OFFLINE_REMAINDERS、清理失败阶段或实删数量。
- 清理每次遍历全部历史 `content_states` 中的 Purged/Migrated：`maintenance.rs:528-570`；恢复也调用它：`engine/recovery.rs:578`。没有清理完成游标可供恢复跳过已完成目标。

##### 风险

调用方可能把普通 400 理解为确定未发生，或把 Published 理解为所有副本已释放。现有幂等保护不会因此消失，但用户协调、重试和容量判断仍不完整。全历史重清理会随墓碑数量增长，且一个历史清理失败可能干扰另一个已发布维护请求的返回。

##### 可实施步骤

1. 先做窄修复：为已发布维护失败保留 `OperationResult`、plan、阶段、目标 version/slot 和具体 source；发布前错误维持原分类。响应明确“逻辑清除已发布，物理清理未完成，保留原 operation_id”。不要虚构 IN_DOUBT。
2. 单独定义 `CleanupObservation` 或同等类型：目标集合、已删除/原本不存在、待处理、离线、失败及最后核验时间。永久 publication receipt 不随清理进度改写。
3. 设计可重启的清理账本，绑定准确 plan、已发布墓碑证明、version、route/binding generation；记录事实不创造新的删除授权。失败在 unlink 后/目录 fsync 前时按“结果待核对”处理，不预报已释放。
4. 清理采用有界批次和恢复游标。旧盘返回先核验身份、墓碑和放置代际；错误盘不删除、不复活内容；保留原重试幂等性。
5. 管理/API 显示逻辑 Published 与物理 Complete/OfflineRemainders/Failed 两个维度。只依据实际删除报告文件级回收；不能保证磁盘立即释放等量空间，更不能声称安全擦除。

##### 依赖和验收

与 S03 共用计划代际/目标集合，但步骤 1 可独立先做，不必等待完整 Pending 协议。不得把“有离线清理残留”自动等同于全部业务写入应阻塞；真实 metadata/identity 故障仍按既有围栏规则处理。

必测：墓碑写失败、unlink 前/后、目录 fsync 失败、掉盘及返回、目标 seal 在证据读取后变化、重复原 ID、不同 owner/digest 碰撞、部分副本清完、服务重启。断言永久收据和业务事件字节不变、已清除版本所有读取路径拒绝、不删除非目标内容、残留状态准确、不能在清理失败后返回暗示“未提交”的普通错误。

#### 4. S03 / P1：作为一个协议交付有限 ReadLease 与 PURGE_PENDING

##### 事实

- `ReadLease` 只有 version 与共享 registry，registry 只有 `BTreeMap<String, usize>` 和 failed 标志：`engine.rs:168-216`。没有身份、期限、续约、撤销代际和进行中 I/O 排空。
- `reads.rs:238-286` 准入租约后持续到对象释放；`maintenance.rs:360-440` 活跃租约、活动分支 HEAD、所有保留目录 HEAD 和活动工作区会阻止清除。preview 本身也调用 `check_purge`：`65-88`。
- 实际 ContentState 仅 `Available/Purged/Migrated`：`src/contract/src/model.rs:844-883`；PlanState 是 ready/cancelled/committed/completed：`201-203`。源码没有 `PurgePending` 和 `CONTENT_PURGE_PENDING` 错误变体，虽然规范有该错误。
- `docs/components/engine.md:158-160` 已坦白缺口。规范要求持久 Pending、新读取/固定屏障、安全取消：`docs/13-retention-and-deletion.md:65-85`；有限期限和禁止 Pending 续约：`docs/19-capacity-and-performance.md:231`；验收 H06：`docs/22-acceptance.md:226`。
- HTTP 已有总传输期限和 admission slot，`src/server/src/transfer.rs:655-709`；它不等于核心通用租约期限，也不涵盖所有原生/诊断/巡检持有者。不要写成“当前 HTTP 完全无限期”。

##### 不可采用的捷径

不能只在引用计数旁加 TTL，过期后从 map 删除并立即 unlink。超时/取消 future 不等于阻塞 worker、内核 read 或已授予的 fd 使用已结束。当前安全拒绝维护比这种“活性改进”更正确。

##### 协议和实施步骤

1. 冻结状态机与操作身份：Preview → Pending barrier → Draining → Tombstone published → Physical cleanup；Pending 可在安全阶段取消，Tombstone 后不可恢复 Available。Pending 与取消必须经原权威发布协议持久化，不能只存在本地 plan 文件或定时器。
2. 拆分“持久强引用检查”和“已有读取排空”：活动 HEAD、工作区、显式内容 pin 等仍是拒绝/重新预览理由；已经合法授予的临时读可排空，不能被误当作持久强引用。preview 展示原因，不只返回第一个 ContentPinned。
3. 明确不可伪造的租约 owner、版本/历史身份、单调时钟期限、当前身份/内容状态续约检查、资源上限与 Engine 代际。持久 Pending 跨重启保留，进程内租约不应跨重启复活；旧 Engine 的计划不能成为新 Engine 的准入凭证。
4. 引入分块/有界 I/O guard 或等效机制：期限到期只停止新块和续约；已开始 I/O 持有保护至真实返回。排空等待不得持全局 Engine 锁。卡住的内核 I/O 保持明确 Draining/受阻，不承诺能强制中断。
5. 把新读、普通/范围读、checksums、工作区来源、完整导出、操作健康诊断、scrub batch、拆分/复制来源与新增 pin 的准入统一到屏障。同一串行边界处理 barrier 和 grant，避免“刚检查无人读取便新增租约”。`reads.rs:239-248` 当前将所有非 Available 都映成 ContentPurged，增加 Pending 后必须显式分型，不能只新增枚举。
6. 适配 HTTP/SMB/current-auth 与原生调用者的续约/取消/断连/关闭；scrub 的批次入口 `engine/integrity_scrub.rs:206-253` 和 `OperationReadPlan` 也必须纳入，不能只修普通下载。
7. 原计划与输入摘要、business/topology/binding generation 的变化规则要明确；取消是持久 Available 的确认结果，不是先解除内存屏障后补日志。未知结果始终查询原操作，禁止以新 ID 重发来逃逸状态。
8. 同步更新 Rust 合同、严格编解码、快照/历史包、内容墓碑与 checkpoint 验证、错误映射、OpenAPI/vector 和文档；只维护当前格式，不新增旧格式迁移或重置池路径。

##### 验收矩阵

- Pending 与 prepare_content/renew/source-pin/scrub admission 的两个胜出顺序；屏障建立后无新 grant。
- 旧租约在授予期限内可完成；到期不接纳新 I/O；已经执行的 read/hash/发送任务在真实退出前仍固定目标。可完成 I/O 的环境内排空有界；不可中断 I/O 不伪报 Complete。
- 到期、身份撤销、SMB rebind、取消、断连、worker panic、drop、registry poison、并发续约、重开旧 owner。
- Pending 发布、取消发布、墓碑发布的每成员文件/目录同步边界与崩溃；重启后 Pending 不丢、Purged 不复活、取消未确认不开放。
- 内容 pin/工作区/目录 HEAD 的保留规则不回退，H01-H06 全路径均有证据；使用可控时间，不以长 sleep 或改系统时钟验证。

#### 5. S04 / P1：关闭“已发布但严格启动失败时无法核对原 ID”的恢复缺口

##### 事实

- 同一 live Engine 可以查询既有永久结果：`engine/operations.rs:121-127`。
- 普通启动依次恢复、加载/核对工作区、重建索引，任一步错误不返回可服务 Engine：`engine.rs:458-472`。
- 工作区双镜像任一落盘失败后保留 blocked：`engine/workspace_storage.rs:101-130`。已有阶段型错误保留 receipt，但不解除写围栏。
- `docs/10-transactions-and-recovery.md:250-252` 明确承认：管理恢复每次重新打开 Engine，故障未排除时操作查询可能仍不可达；尚无绕过完整启动的离线收据查询或服务核对完成协议。

##### 建议与依赖

先写一个小 ADR，决定是提供受限、只读的恢复诊断 owner，还是显式的核对入口。必须区分“读出经过验证的原 publication receipt”与“完成 workspace terminal mirror/index 并允许写入”。它依赖当前管理认证/授权、可信固定配置、准确 journal/publication closure 和 writer ownership；不能以裸 JSON、SQLite 或单份自洽证明发放成功/未发生结论。

可实施切片：

1. 抽出不做修复/清理的准确 receipt 检查能力，返回 Published、InDoubt、可信证据下 NotFound，或证据不可用，保留故障阶段。
2. 当业务 Engine 不可启动而管理认证可用时，仅开放明确受限诊断；授权失败不触发昂贵 I/O，也不泄露其他主体/内部路径。
3. 独立的 reconcile 动作只处理已绑定的未完成终态，不重复业务发布、不清理未知 spool；成功后重新经过全部正常启动准入才释放围栏。
4. 新 API/工具不能成为“忽略坏工作区/忽略索引就直接可写”的旁路。

验收：分别在 workspace 第一个/第二个镜像、预算/路径求值、索引打开/重建、恢复再次崩溃制造失败；重启后仍可准确核对原 ID、摘要和结果，但写保持阻塞；故障排除后同一计划收敛，操作与事件不增加。结合 S02 统一阶段信息，但不把所有后发布错误绑定为同一种服务状态。

#### 6. S05 / P1 验收门禁：建立真实持久化边界证据，而非扩张“已证明”措辞

##### 事实与边界

- 源码已有 extensive journal、恢复、checkpoint、proof、索引、错误源、路径替换、跨入口测试。`docs/validation/engine-failure-boundaries.md:29-54` 明确区分 libc 注入、非 root EACCES 与物理坏盘/断电。
- `docs/10:220` 要求逐步骤进程退出、整机断电、短写、同步失败和永久丢失组合，kill 服务不能替代断电。
- 独立 publication model 是有限抽象。`docs/validation/publication-model.md:5-11,53-59,74-78` 明确不证明生产实现/真实设备，保留旧 8 档根搜索和 6 档局部搜索 Inconclusive；不能自动扩大暂停的搜索或把这些档位写为通过。
- `docs/validation/release-checklist.md:33-56` 要求一次性测试环境、精确交付字节和设备/文件系统记录。

##### 规划

1. 建“协议阶段 × 成员 × I/O 失败窗口 × 恢复结果 × 对应生产测试”的矩阵，标出现有、缺失、未运行、通过/失败的精确 tree。
2. 软件验证先覆盖写/短写、fsync 返回错误但可能有副作用、rename 后目录未同步、日志撕裂尾、中间损坏、单份 publication、系统盘丢失、掉盘后原成员返回和恢复第二次崩溃。
3. 把新 Pending/清理/核对状态机也加入矩阵，使用独立完整模型作为 oracle，不用被测快路径生成预期。
4. 设备验收另立一次性环境工作包：Debian 13 amd64、精确内核/文件系统/挂载/缓存设置、独立实际设备与同提交发行字节。现实掉电、设备掉线、磁盘满分别记录；不在生产盘执行。

验收不变量：成功回执从不丢失/回滚；未知不误判未发生；同 operation 永久幂等；Q 不缩减；正常新发布仍全配置成员和 required 内容路由完成；错误不以零值/不存在掩盖；所有失败原材料保留；坏/旧盘不被重新初始化。物理验收与软件 CI 各给独立结论。

#### 7. S06 / P2：历史拆分支持范围与保留引用的需求补齐

##### 事实

- `maintenance.rs:24-62` 明确拒绝带不可变 file path/metadata/merge/mixed ancestry 节点或目录组合的历史；`90-105` 拒绝独立文件分支。`docs/components/engine.md:150` 同样把它们列为未支持。这是安全能力边界，不应去掉检查来“修复”。
- 规范 `docs/13:51-55` 要求 preview 展示全部受影响提交、分支、句柄、内容固定、路由与预计释放；当前 plan input 主要是 history + 去重 version_ids：`maintenance.rs:65-88,147-167`。
- `pin_content=true` 目前出现在需求文档，当前 State (`src/contract/src/model.rs:1057-1138`) 和 `check_purge` 没有对应显式 pin 记录/接口。此项应标为需求缺口，不声称现有用户已经设置的 pin 会被忽略。

##### 实施规划与验收

先按真实产品用途选择切片，不把所有高级拆分一次合并：

1. 建统一的只读引用分析/preview 投影，覆盖活动及保留目录 HEAD、继承父版本、工作区、开放读取和未决维护。输出完整影响和逐项不能清理的原因；S03 复用同一逻辑，不维护第二套猜测性引用规则。
2. 若要交付显式 pin，先定义 pin 的主体、作用域、持久记录、解除操作和与 Pending 的串行关系，再接入 preview/执行/导入导出/恢复。
3. 若要扩展拆分，逐类定义 file/path/metadata/merge/mixed-parent/directory-tree/selection/backup scope 的 ID 与边重映射、跨界边处理及包内闭包。保持两域同一原子决定，先证明目标 FULL 独立，再清除源。
4. 无论是否扩展，API/UI 能力字段与运行手册明确当前哪些历史可拆分，unsupported 返回是原子无副作用。

验收：拆分后分别断开另一历史域的全部对象独立恢复和导出；墓碑、永久收据、保留关系与跨界审计不丢；不误把审计来源变活动内容依赖；目录/别名/混合祖先不悬空不成环；并发写/Pending/pin/SMB rebind 的输入变更拒绝并要求新预览。没有扩展的类型继续明确拒绝。

#### 8. S07 / P2：恢复和元数据增长的容量预算，不能冒充有界启动

##### 事实

- `recovery.rs:156-185` 启动扫描全部已准入 journal，并保存前缀/记录；checkpoint 仅在全成员在线且无撕裂尾时尝试：`254-268`。
- checkpoint 上限：封装 16 MiB、覆盖字节 8 MiB、每盘条目 32,768，`recovery_checkpoint.rs:96-98`；超限回退完整恢复，`docs/architecture/verified-recovery-checkpoints.md:48-55,74-105` 明确总启动仍随历史增长。
- 当前没有已发布 immutable metadata 的 runtime GC；恢复和 scrub 把历史闭包作为必须保留对象，`docs/architecture/integrity-scrub.md:25-28,63-65`。不能仅按 age/mtime 删除旧对象。
- 事务共享记录首写仍复制全键/句柄集合，历史驻留等成本没有消失，`docs/components/engine.md:74-82`；本规划不重新启动已拒绝/暂停的持久容器候选或规模实验。

##### 实施规划

1. 先做静态成本台账，再定义经单独授权启动的测量矩阵：journal bytes、唯一 metadata objects、永久 operations/events、墓碑、工作区冷热证据、checkpoint 覆盖与回退原因。
2. 测量 warm 写、首次冷启动、checkpoint 命中/未命中、缺成员只读重放、修复、scrub 各自的墙钟、CPU、峰值/驻留内存、真实读写 bytes、对象安装和 fsync 数，不以快照解码变少等同总启动变快。
3. 先消除已完成清理的无意义全量重做（S02 账本），并提供接近元数据预算的准确告警/拒绝；不裁剪永久幂等证据。
4. 如果目标是真正限制总启动 I/O，另立分段 journal + 封存根 + retention/repair + 全历史离线验证的协议 ADR，明确旧字节损坏检测时点改变；不把它包装为简单 cache 优化。
5. 如果将来加入 metadata GC，必须同时覆盖 recovery/checkpoint/scrub/历史包/进行中读计划的保留根和真实磁盘 lease，先定义 RECLAIMING、代际与宽限期，不能凭 Arc<State> 推断磁盘对象不会被删。

验收：每项复杂度声明都有对应数据维度与原始证据；上限到达保守回退且不丢证据；增长告警不假定无限历史；任何分段/GC 设计先有安全模型与恢复反例，旧格式拒绝保留、无自动迁移和运行池重置。

#### 9. S08 / P2：完整持久合同和跨读取器向量

`docs/contracts/README.md:5-15,54-59,73-77` 仍把完整 Candidate、PackageProof、墓碑、日志帧 payload 与回执闭包列为未全部建模，格式未冻结。部分 schema/vector 通过不能证明完整协议或跨 Rust/Web/rescue 行为一致。

计划：按照实际闭包入口补齐缺少的结构与语义映射；共享严格解析器保留重复字段、未知字段、canonical 数字、摘要域、成员集合、路由/前缀/深度/预算和精确目标状态。使用人工审阅固定字节和独立 negative corpus，覆盖断页、尾部垃圾、重复回执、错误对象域、错误池/代际、对象枚举别名及显式空字段引起的重编码漂移。把新增 Pending/cleanup 元数据纳入全部消费者。验收为相同输入在在线引擎、独立包、rescue 和合同检查器得到一致许可/拒绝及原材料保留，不通过重新生成期望值隐藏协议变更。

#### 10. 建议排程与验证入口

建议依赖链：

1. S01 基线矩阵，和 S02 的发布后错误窄修复设计可并行。
2. S02 清理结果合同与 S04 核对合同先定下来，再确定 S03 多阶段计划身份及持久状态；S03 的 core/adapter/恢复测试必须一并交付。
3. S05 持续贯穿每个行为切片；完成新行为后才签发相应能力。
4. S06 高级拆分/显式 pin、S07 分段/GC/大规模测量、S08 全格式冻结作为独立后续项目，保持当前暂停与授权边界。

已有、可纳入未来精确树验证的命令（本次均未执行）：

```text
cargo test --locked --offline -p storage-engine -p storage-contract -p name-policy --lib
cargo test --locked --offline -p storage-engine --test workspace_commit_replay
cargo test --locked --offline -p storage-engine --test historical_workspace_lifecycle
cargo test --locked --offline -p storage-server --test http_engine
cargo test --locked --offline -p storage-server --test service_startup
cargo test --locked --offline -p storage-server --lib integrity_scrub
cargo check --locked --offline -p storage-engine
python3 .github/scripts/check-engine-fault-io.py
python3 .github/scripts/check-disk-observation-io.py
cargo run --locked --offline -p contract-check
```

原生 Mount/ptrace/设备矩阵依赖独立合适环境，不能在不具备条件的云环境把拒绝当跳过通过。发行正常依赖图不得携带 `fault-injection`，也不能用测试构建产物代替实际发布字节。最终每个切片记录源码 tree、命令、环境、退出码、通过/失败/未运行/环境阻塞和原始日志，并分别说明资源、同步预算及仍然未关闭的路径。


---

## 7. 专项审查与工作包

### ZXS Foundation 只读审查与整改规划输入

#### 结论及可使用范围

1. 当前 Foundation 的目录、来源和测试治理已经建立较完整的骨架，不能以“第三方源码”或“原样导入”把它们排除在《禁止防御性编程.md》之外；也不能把重命名、外置测试接线和必要边界检查当作冗余删除。
2. 当前生成覆盖表是 205 个有测试执行入口、0 个缺执行入口、17 个仅参考，共 222 个包。这证明有限测试接线覆盖，绝不等同于全部 API、错误路径、目标、属性或当前提交实际运行通过。
3. 本次优先定位到 SQLite FFI 空指针/回调 panic 边界、响应式 Action 顺序和 abort 契约、DOM 错误传递与延迟队列、公共计时器长度边界等规划项。明确区分静态事实、需复现的行为、实际产品可达性；未复现的不标成已确认产品故障。
4. 本轮只读，无源码、测试、文档、收据、分支或提交修改。当前暂停族和指定文件一律仅规划，不执行，不恢复旧任务。

#### 基线与审查方法

- 本地：`审查检出目录`，HEAD `a50502f5b5073dd90ff88a1f3fcb39e08f9f24cf`，tree `927bbc7f7354b2cd6f4b84fdd84c9686692174d2`，开始/结束 `git status --short` 均为空。
- 已通过 GitHub REST 确认远端 main `72e08db` 使用相同 tree。本地未持有该短 SHA 对象，因此这里不把本地 `git rev-parse 72e08db` 失败解释为远端不存在。
- 已阅读根 AGENTS、完整《禁止防御性编程.md》、Foundation 分组/消费者/测试覆盖/来源治理说明，逐组核对目录与覆盖清单，静态追踪关键消费者和错误路径。
- 执行 `PYTHONDONTWRITEBYTECODE=1 python3 tools/rust-sources/agents_audit.py > <审查库存输出>`。输出为仓外研究产物，不改变仓库。
- 全量 inventory 覆盖 5,520 个 `src/foundation/**/*.rs` 文件、222 个 Cargo manifest，原始文本 1,882,373 行（包括生成数据、Rustdoc、bench 和休眠材料，不是“活动手写实现行数”）。2,198 文件与原路径原始字节相同、160 与其他原始路径字节相同、3,162 需映射复核；后者包含机械改名/合并，不代表 3,162 个逻辑改动。
- inventory 语法候选：clone 5,675、忽略结果 2,279、默认回退 1,796、边界/内部算术 2,034、错误擦除候选 531、通配空分支 448、panic/retry 边界 191。类别可重叠；不是缺陷计数。脚本局限见 `tools/rust-sources/agents_audit.py:10-18,61-69`；既有审阅也明确非全量达标证明，见 `docs/reviews/foundation-agents-audit.md:13-31,61-70`。
- 未运行 Cargo、浏览器、完整 static、来源 metadata verify 或 CI。本次不对运行通过、安全性或性能作新声明。

#### 暂停与治理边界

当前暂停范围：cryptography、diagnostics、data-storage、collections、core-types、platform；另含 rustls、fancy-regex 指定文件，以及 futures io/sink overflow probe。本报告不会执行任何对应修改或测试性任务恢复。下列“计划/验收”均是获准恢复后的执行设计。

另有独立来源治理约束，不应与用户暂停混同：`docs/architecture/foundation-groups.md:38-56` 规定重命名/合并/裁剪/固定平台与密码学证明等需要专用后继适配器；普通 refresh 不授权重算历史摘要。`docs/validation/foundation-test-coverage.md:12` 点名平台族（tokio、synchronization、system-entropy、libc、memchr、mio、rustc-demangle）、ring、http-transport 的特殊 owner 边界。必须按当前所有者与支持的适配器核验，不能借普通维护覆盖受保护收据。

#### 十二组覆盖与工作包

下表“已执行/参考”来自当前生成 fixture，仅描述登记种类，不是本轮运行结果。组规模是盘上原始 inventory。

| 组 | 包 / Rust 文件 / 原始行 | 已执行/参考 | 本组计划重点与证据 | 优先级与依赖 |
|---|---:|---:|---|---|
| code-generation | 14 / 207 / 93,990 | 12/2 | proc-macro2、quote、rust-syntax、rstml、builder/derive 的宿主语法边界、错误 span、生成路径与命名保持；先复核本地改名/属性而非恢复上游。现有说明 `docs/dependencies/code-generation.md:11-107`、宿主消费者测试范围 `docs/validation/foundation-test-coverage.md:164-173`。 | P2；按真实宏消费 feature 与编译失败负例，不以 WASM 目标代替宿主宏。 |
| collections（暂停） | 36 / 525 / 180,853 | 31/5 | 分配容量/溢出、共享/借用/擦除类型、析构次数、hash 表受外部键控制的资源界限；已修正 ErasedMut 借用契约见 `erased/src/erased_mut_ref.rs:35-85`，不可重提为现存别名缺陷。`docs/components/hash-table.md:32,61-72` 明示默认 foldhash 的 HashDoS 限制。 | P1 安全与所有权；先追踪实际外部键和预算，不能笼统换哈希器；依赖来源与暂停解除。 |
| concurrency | 25 / 938 / 236,131 | 24/1 | channel/锁/取消/worker 生命周期与异常关闭，区分 send 失败意味着接收端已释放与真正丢业务失败。task-dispatch 的测试 executor 资源失败见下文；paused futures probe 不动。`docs/components/synchronization.md:34-40` 承认现有调度测试不等同 Loom/Miri。 | P1 生命周期，P2 API 一致性；tokio 等受保护 owner 另受适配器约束。 |
| core-types（暂停） | 15 / 301 / 111,906 | 13/2 | date-time 数值单位、解析范围、canonical 格式与截断策略；numeric 精度和外部整数转换；不机械删 checked/try_from。`docs/architecture/date-time.md:34-40`、`numeric.md:24-56` 明确支持矩阵与未执行材料。 | P2；与真实协议/序列化向量联动，保持当前格式，无旧格式迁移。 |
| cryptography（暂停） | 18 / 343 / 63,394 | 18/0 | 熵失败/初始化、秘密比较、清零、算法参数/预算与 CPU 分派；Blake3 的固定长度比较真实入口缺直接功能覆盖见 F-08。`docs/components/cryptographic-core.md:7-13,29-41` 与 coverage `:84-90,225-230`。 | P1；功能向量与常量时间性质分离；不能以包均有入口推断性质通过。 |
| data-storage（暂停） | 14 / 469 / 195,060 | 13/1 | SQLite FFI/close/回滚/池清理；序列化重复键/尾随/借用/数值错误；压缩解码预算与 schema 模式执行错误。F-01/F-02/F-03。`docs/components/data-serialization.md:38-52,71-82` 明确 decoder.end 与路径错误边界。 | P1 先恢复失败与内存安全证明；与 platform 消费者、实际 SQLite SDK 和治理链相关。 |
| diagnostics（暂停） | 11 / 234 / 86,224 | 10/1 | 库错误 source、日志字段脱敏、一次记录、过滤器宽容语义与 logger 写失败；不要在日志器里递归记录错误。`docs/components/logging.md:13-15`、`tracing.md:22-26` 明确宽容解析/未执行测试限制。 | P2；先制定错误决策层和日志预算，生产库不能全局 anyhow 化。 |
| networking | 25 / 668 / 195,859 | 23/2 | HTTP 长度/预算/超时、取消/关闭、响应 body 与静态资源 Range；TLS trust 各主体不得因“重复检查”去掉。rustls 指定文件暂停，http-transport 有维护链。`docs/architecture/foundation-ci.md:87-95` 给实际消费者；coverage `:125-127,181,228` 给 TLS/Range 与残余材料范围。 | P1 边界，P2 错误链；先维护现行消费者回归，再按原生/WASM 功能分片。 |
| platform（暂停） | 9 / 546 / 146,571 | 9/0 | ABI、owned/borrowed fd、路径/文件身份、分配/系统错误与关闭；保留真实外部边界。现有命令 `.github/scripts/check-foundation.sh:12-23` 和 test guards 缺口见 F-07。 | P1；真实 Linux amd64 SDK/ABI 与 WASM 空接口分开，不把 stub 编译算执行。 |
| text-unicode | 23 / 528 / 263,223 | 21/2 | 正则资源界限、输入错误、规范化表身份及跨 Unicode 输入；fancy-regex 指定文件暂停。`docs/architecture/pattern-matching.md:43-55` 与 `unicode-data-families.md:24-50` 明示缺提供者/休眠 UTF-16 与支持组合边界。 | P1 正则运行失败，P2 数据/特性；定向负例与支持图，不恢复缺失上游框架。 |
| tooling | 14 / 318 / 99,134 | 13/1 | cc/bindgen/libclang 的外部进程/SDK错误链、路径/空格/长 stderr、CLI 公共参数与模板诊断。`docs/architecture/native-c-toolchain.md:9-32`；真实编译和链接须区别于 Samba SDK/WASM C 实测。 | P2；固定源身份与运行环境在先；不把 root features 闭包当单一产物。 |
| web-runtime | 18 / 443 / 210,028 | 18/0 | 响应式顺序/取消/owner、DOM错误/队列、计时器边界、WASM glue 及宿主文本工具。F-04/F-05/F-06。`docs/architecture/reactive-browser-runtime.md:56-78` 明确“取消 future 不等于回滚网络副作用”。 | P1 状态/调度，P2 API；真实 WASM/Chrome 验收，不新增浏览器兼容降级。 |

Foundation 目录不等于 crate 可合并性。保持 `src/interface`、`src/naming`、`src/macros`、`tools/web-bindings` 独立职责，见 `docs/architecture/foundation-groups.md:22-33`。

#### 重点发现与可执行规划（本轮均不执行）

##### F-01　P1：SQLite regexp 的 FFI 转换失败未守住切片构造边界（暂停）

- **静态证据**：`src/foundation/data-storage/sqlite-access/src/sqlite/regexp.rs:151-157` 先按原值 type 排除 SQL NULL，再调用 `sqlite3_value_text`，随后 `sqlite3_value_bytes`，直接 `slice::from_raw_parts(ptr, len)`。同树 `src/foundation/data-storage/libsqlite3-sys/sqlite3/sqlite3.h:6009-6029` 明确文本/长度转换 OOM 返回与 SQL NULL 相同，且需要在其他 SQLite 调用前立即检查 errcode。提前判断 SQL 类型不能证明转换后指针非空。
- **现状/边界**：当前 root-native 的 sqlite-access features 只有 sqlite-bundled，regexp 属于保留公共能力。`.github/scripts/check-sqlite-family.py:152-156` 在保留全能力测试配置启用 regexp。本轮没有构造 OOM，也没有证明当前服务器可触发。
- **计划**：先审阅完整 SQLite 文本返回与生命周期契约，构造受控分配失败反例；在实际 FFI 边界形成有效借用/错误结果，区分真正 SQL NULL、空串、OOM、非法 UTF-8，不把失败转成空串或无匹配；保持 length 与 pointer 的相同转换时序及来源错误。
- **验收**：整数/REAL/BLOB 转字符串、空值/空串、非法 UTF-8、分配失败均无 UB/abort，正确 SQL 错误可向上识别，随后连接可用或明确退出；bundled 和系统 SDK 各自测试。不得把随机大分配制造的进程 OOM 当稳定故障注入。
- **依赖/估计**：暂停解除、可接受的 SQLite allocator 注入策略、source owner 维护；调查/反例约 1–3 工日，修复/验收约 2–5 工日，故障注入与 SDK 是主要不确定项。

##### F-02　P1：SQLite 自定义回调 panic 与析构异常没有统一安全协议（暂停）

- **证据**：`.../sqlite/connection/collation.rs:117-151` 的 extern C 回调内包含 assert、长度转换 panic、用户 `Fn` 调用，没有 catch_unwind；`:129-130` “unwinding caught at FFI boundary” 容易让读者把禁止跨 ABI 展开误解成可恢复隔离。`:35-36,89-90` 的 C 清理回调会运行用户捕获值析构。`connection/handle.rs:139-148`、`statement/handle.rs:449-462` 在 Drop 中 panic。普通 progress/commit hook 在 `connection/mod.rs:299-346` 有 catch，但 update/rollback `:322-359` 静默丢弃 panic。
- **现状**：`docs/architecture/sqlite-access.md:93` 已准确声明 collation 所有权修补不解决 callback panic，也未发现当前产品自定义 collation 消费者，不能重述为已修复或已知产品事故。已有 NUL/SQLITE_BUSY/capture-release 测试仍应保留。
- **计划**：为每类 callback 明确失败后当前语句、连接和 worker 状态；先以子进程负例证明当前 abort/中毒边界，再设计统一错误通道、连接退役与显式 close。collation 没有天然错误返回码，绝不能简单 catch 后返回 Equal 来掩盖失败或破坏排序正确性；必要时独立设计。
- **验收**：用户回调 panic、捕获对象 Drop panic、SQL 错误、正常 shutdown 与双重失败分别记录；不跨 C unwind、不 silent-success，不产生 double-panic；release 构建 panic 策略与协议一致。
- **依赖/估计**：暂停、FFI/worker 设计评审、显式可失败 close 语义；调查/设计 2–5 工日，落地 3–8 工日或更长，不能先承诺一行 catch 修复。

##### F-03　P2：SQLite 池的后台关闭/清理仍有错误去向空白（暂停）

- **证据**：`.../query_engine/pool/inner.rs:367-374` after_connect 失败后的 close_hard 结果丢弃；`:550-556` 回收闲置连接忽略 close 结果；`pool/connection.rs:157-174` 关闭超时 `.await.ok()` 同时丢失超时和内部 close Result。
- **现状**：池属于 sqlite-pool-validation 及保留可选平台能力，不是 root-native 默认连接模式。普通显式关闭/回滚已经有错误保持修复，不能因此把这些后台路径也称为闭环。
- **计划/验收**：先定义 best-effort cleanup 与首要失败的关系、连接是否已经退休和资源释放结果；后台路径按池边界保留一次结构化诊断/可检查状态，正常调用返回具体双重错误时不重复记录。覆盖 after_connect+close 双失败、shutdown 中运行时退出、回收超时和后续借用拒绝。无需让每个合法 channel.send().ok() 变报错。
- **依赖/估计**：F-02 的关闭协议、pool runtime 许可和暂停；1–3 工日复核，2–5 工日落地，取决于是否需公开错误契约改变。

##### F-04　P1 先验证：响应式 Action 的 latest 顺序栅栏与 abort-handle 丢弃契约

- **证据**：`src/foundation/web-runtime/reactive-state/src/actions/action.rs:268-290,320-343` 读取 `dispatched` 后以其值比较 latest；全文件中 `dispatched` 仅初始化、clone、读取，没有递增写入。若并发 dispatch，顺序判断并不能区分请求。`:250-251` abort 用 oneshot sender；`:280-283,332-335` 把 receiver 的任何完成都当 abort，也包括 sender 被直接丢弃的取消，而 `:428-433` 文档示例直接丢弃返回句柄仍期望成功。
- **现状**：静态事实明确，实际可观察语义仍需用受控 future/单线程 executor 复现。`src/web` 和 `src/interface` 静态检索未发现 Action 构造/dispatch 的当前消费者；不据此删除公开 API，也不说产品已发生旧结果覆盖。
- **计划**：先定义并发完成、显式 abort、单纯丢弃 handle、clone action、owner 清理各自语义；使用两个可控 completion 通道，无任意 sleep 地强制 B 先完成、A 后完成；对 Send/local 两条路径做同一契约测试，再最小修改状态表示和处理。
- **验收**：旧结果不覆盖明确选定的新结果；仅显式 abort 才按契约取消（若保持 drop-cancel，则须明确 API/文档及所有调用方）；pending/count/input/value/version 一致；disposed owner 不被异步回调重活；网络副作用不被错误宣称回滚。
- **依赖/估计**：当前 API消费者核对、source变更登记、CSR门禁和真浏览器；反例 0.5–2 工日，修复/回归 1–4 工日。若公开语义选择需决策则独立记录，不能靠静态稿宣布完成。

##### F-05　P1/P2：DOM 失败、延迟属性队列与 release 可观察性

- **证据**：`src/foundation/web-runtime/view-renderer/src/lib.rs:71-140` 的 or_debug/ok_or_debug 只在 debug 输出告警，release 丢弃 Err 或转 None；`renderer/dom.rs:118-151,255-278` 用于实际 setAttribute、insert/remove、property；Reflect::set 的 Ok(false) 也未被区分。`renderer/dom.rs:60-78` flush 前取走队列，PENDING 在执行所有任务后才置 false，期间重入入队的工作没有显式安排下一轮。`:255-263` 的 value 写进入这个队列。
- **现状**：错误丢弃是代码事实；哪些 DOM 错误可容忍、哪些意味着不变量已失效要按调用点判断。重入丢调度目前是待复现候选，需通过自定义 property setter 或等效真实 DOM 回调验证，不按存在队列就定为产品 bug。当前 `docs/components/interface.md:28` 声称实际 API 失败保留具体错误，需澄清这是否仅指新启动/请求控制层，否则与公共渲染层叙述范围不一致。
- **计划**：列 DOM 操作错误分类表，保留必要宿主边界、正常移除/已卸载语义；对内部 bug 明确失败，对可恢复宿主失败向正确 UI/任务边界保留原因。单独验证 reentrant queue、任务中 panic、同 tick 多次 value、卸载前后迟到任务，避免用全局 catch/default 掩盖状态损坏。
- **验收**：debug/release 对语义的处理一致；错误不 silent-success，不重复灌日志；第二批重入任务按约定帧/微任务边界执行；卸载后不访问已释放 signal。真实生产 WASM/Chrome 记录与准确提交对应。
- **依赖/估计**：公共 renderer API 与 UI 生命周期，测试 fault injection 需仅置于根 test；2–5 工日审查/反例，3–8 工日修复，若涉及 API 错误类型扩散需重新估计。

##### F-06　P2：公共计时器 Result API 的输入边界会先 throw

- **证据**：`src/foundation/web-runtime/dom-helpers/src/helpers.rs:301-329,451-483` 接受任意 `Duration`、返回 `Result<Handle, JsValue>`，但毫秒转 i32 使用 `try_into().unwrap_throw()`（`:324,478`）；外部 Duration 超范围先 throw，不能落入调用方 Err 分支。`src/interface/src/animated_show.rs:63,86-93` 直接传公共 hide_delay，并提供 Err 处理。未证实当前产品给出超范围值。
- **相关候选**：helpers `:36-46,140-141,214-215,286-287,394-409,435-436,520-531` 还有返回值忽略/最佳努力 API；须分别判断契约，不批量修。
- **计划**：将 JS duration 边界显式验证为有界类型或具体返回错误，调用内部不重复转换；不以饱和、截断或默认时长偷改业务语义。保留 owner 清理并明确无 handle API 的资源责任。
- **验收**：0、i32::MAX 毫秒、超一毫秒与极大 Duration；返回错误无 throw/泄漏、捕获对象只析构一次、取消/卸载后不回调。原生验证纯边界、WASM 验证实际 JS 调用。
- **依赖/估计**：公开 API、AnimatedShow/exports 消费者，1–3 工日调查与修复、1–3 工日浏览器回归。

##### F-07　P1 验证可信度：从“存在入口”升级到性质/失败路径/精确名称账本

- **证据**：`docs/validation/foundation-test-coverage-map.md:7-13` 是 205/0/17，分类定义 `foundation-test-coverage.md:49-58` 明确至少一个针对包的测试入口。旧直接 cargo 调用 `.github/scripts/check-foundation.sh:21-22,32,37,41,57-58` 不都具备逐名保护；coverage `:230` 明确承认部分旧调用接受零测试。`:227-229` 的 missing providers、性质和目标缺口仍然成立。
- **计划**：在现有覆盖文件基础上添加（或以紧邻机读索引表达）实际使用 API/行为、边界/取消/关闭/析构/恢复、目标特性、测试名/commit/result/限制维度；保留 P/U/F/I 的不同意义。逐批为旧入口记录实际 --list 和执行名称，区分 Rustdoc compile-only/ignored；不手写通过计数、不用 stub 新建上游 shim。
- **验收**：删除/改名/过滤关键测试会失败；新增失败路径能从消费者追踪到门禁；所有未执行属性仍明确列出；准确提交 CI 的五分片、产品、WASM、浏览器/SDK 不被一个报告替代。
- **依赖/估计**：先按当前 active graph 排序，受暂停范围的新增执行不恢复。入口补强 2–5 工日，深层性质覆盖无可靠总期，分组件 2–10 工日批次规划。

##### F-08　P1/P2：秘密比较的实际固定长度入口与性质证明缺口（暂停）

- **证据**：`src/foundation/cryptography/blake3/src/lib.rs:355,363` 调用 constant_time_eq_32，`:371` 才调用切片入口。`docs/validation/foundation-test-coverage.md:84-90,229` 明确现有 exhaustive 只覆盖切片和 generic 切片入口，长度含 32 不等于固定 _32 API 已直接执行，功能断言不证明常量时间，也不证明 WASM/非 x86 后端。
- **计划**：优先真实产品的 _32 API 功能契约与 BLAKE3 Hash 比较/篡改拒绝，独立登记常数时间、CPU dispatch、WASM、Miri/指令计数的证据需求。不得为数量任意扩展未支持目标，也不得把 Valgrind 单个 codepoint 当全库性质证明。
- **验收**：每一位差异、相等/不等、API调用身份明确；实际选定后端记录；未具备目标/提供者明确未执行。
- **依赖/估计**：暂停、密码学 proof owner、专门工具/目标；基础功能 1–3 工日，性质验证另做实验设计，不承诺单一完成期限。

##### F-09　P2：任务调度初始化与资源失败的公共契约

- **证据**：`task-dispatch/src/lib.rs:182-200` 的 init_futures_executor 仅设置静态函数，真实 ThreadPool::new 在首次 spawn 惰性执行并 expect；`:205-214` spawn 失败亦 expect。初始化成功不证明 OS 资源已就绪。`docs/architecture/task-dispatch.md:5-12,35-43` 指定该后端为原生测试，浏览器使用 wasm-bindgen；必须按实际后端，不把保守统一图中的 feature 当当前 WASM运行调用。
- **计划**：记录 public init/spawn 的资源错误策略和是否需要显式 fallible 初始化；若维护支持路径，外部线程创建失败保留具体错误，不永久缓存失败；已初始化要求与递归 poll 的合法无操作不机械视为防御编程。
- **验收**：一次性初始化/失败再试/多线程竞争，未初始化调用的显式契约，以及资源失败不伪称已初始化。真正使用的 wasm backend 原有调度/清理测试不退化。
- **依赖/估计**：公共契约与测试后端用途确认，0.5–2 工日调查；若需 API 修改约 1–4 工日。

##### F-10　P2/P3：文档当前状态、历史记录与法律材料保持分层

- **已确认叙述漂移**：`docs/validation/foundation-test-coverage.md:189` 仍写“当前清单 204/0/18”，而生成表 `foundation-test-coverage-map.md:7-10` 为 205/0/17。该行若保留为历史应明确当时快照，当前值只链接生成表，避免又建一套人工数量。
- **审阅历史不是失效证据**：`docs/reviews/foundation-agents-audit.md:3,16-18` 的旧 baseline 与 5,488 文件是带日期/基线的历史，不能机械更新成当前数字而抹掉当时证据。新增本次说明应独立标记范围。
- **中文/当前API**：新维护说明使用中文；旧上游原文、许可、historical proof 不翻译改写。`docs/dependencies/README.md:9-13,43-56` 已明确三层文档和当前包名规则。
- **既有法律开放项**：`compliance/NOTICE.md:5,67,90,97` 三项独立许可资料复核仍开放（convert_case_extras、drain_filter_polyfill、erased）；文档/功能测试不能把它们自动结案，也不应直接改写固定 NOTICE 来消除告警。需要取得精确来源材料与受支持后继证据，保持原始字节。
- **验收/估计**：当前/历史措辞修订与自动索引核对约 0.5–2 工日；法律材料受外部证据与治理支持影响，不给无依据时限。

#### 不应执行的“简化”与已保留的正确保证

- `reactive-state/src/computed/arc_memo.rs:112-138` 已用 PartialEq/自定义 changed；`computed/inner.rs:165-180` 仅 changed 通知，不应再按上游印象建议“所有 Memo 都缺等值短路”。后续性能工作应追实际投影大小与锁/分配测量。
- `erased/src/erased_mut_ref.rs:35-85` 返回引用绑定 wrapper 借用，既有 compile-fail 证明保留。不能重复修旧缺陷或去掉必要 unsafe 前提。
- SQLite collation 的 NUL 和失败注册捕获释放已存在 `collation.rs:47-68,81-113`；不能仅因 CString 检查每次存在就删除外部边界。
- `task-dispatch::poll_local` 未初始化或事件驱动后端无操作、递归借用不重入是文档约定；`ActionAbortHandle::abort` 向已经完成接收端发送失败通常是合法完成状态。真正待核实的是错误如何改变业务状态，而不是 `.ok()` 数量。
- 保留资源限制、外部长度/overflow、授权/撤权、磁盘再读取校验、FFI指针和SDK契约；保留合法 `#[cfg(test)] #[path]` / `include!` 接线。`禁止防御性编程.md:29-44,58-67,103-111,131-147` 是依据。
- 不恢复原名/原框架、不把测试迁入 src、不增加旧格式兼容/迁移、不改变真实部署/凭据/权限、不借全仓整改删除用户数据。

#### 建议执行顺序、依赖与交付

1. **B0 固定证据**：基线 tree/干净状态、消费 profile/feature、source owner/adapter、暂停边界和已执行精确结果。把本报告候选拆成 confirmed / required boundary / valid contract / unresolved，不把 grep当裁决。
2. **B1 定义边界契约**：先 F-01/F-02/F-04/F-05 的输入、失败决策层、生命周期与错误类型。安全/持久化/公共 API改变需要独立设计记录，不以“禁止防御性”替代证明。
3. **B2 最小反例**：根 test 中可控失败/时序测试，先保留原失败证据与名称；非暂停区域可获准后逐批推进，暂停区域仍等待恢复指令。
4. **B3 最小实现**：只改调用链已证明需要的实现，字段私有、类型保证持续有效、显式错误来源；不额外添加通用框架、全局 fallback 或重复校验。
5. **B4 分层验收**：适用 debug/release、default/no-default/显式支持 feature、native/WASM/真实浏览器/实际 SQLite SDK；隔离 full feature 测试仅在已验证支持组合内，不盲目全仓 all-features。
6. **B5 整合证据**：按受支持操作更新当前来源摘要并审阅差异，运行 architecture verify、foundation_coverage --check、documentation verify、manage verify --metadata、static、受影响分片和最终准确提交CI。fixed owner需先有正确adapter；失败不能重算历史摘要蒙混。

可复用验收入口（本轮未执行）：

- `python3 tools/rust-sources/architecture.py verify`
- `python3 tools/rust-sources/foundation_coverage.py --check`
- `python3 tools/rust-sources/documentation.py verify`
- `python3 tools/rust-sources/manage.py verify --metadata`
- `bash .github/scripts/check.sh static`
- `bash .github/scripts/check.sh foundation-{platform,crypto,data,language,async}`（按真实命令分别执行，花括号不是 shell 中一次合法参数）
- CSR / 生产 WASM / browser-smoke、sqlite-system 等按真实受影响链增加，不能宣称上述 foundation 五片穷尽产品验收。

每批交付至少包含：当前状态、具体输入边界/类型保证、实际调用者/feature、被保留的必要检查、变更前后复杂度/分配/锁/同步预算、准确命令/提交/测试名称，以及通过/失败/未执行/环境限制四类结果。Foundation 数百万原始行不能由一次 static audit 宣称“全部达标”。工作量应按上述小批反例和依赖滚动修订；本报告的工日区间是规划量级，不是承诺，不能简单相加成全仓交付日期。


---

## 8. 专项审查与工作包

### ZXS Web、API、国际化、监测与硬盘标识/备注审查

#### 基线与结论边界

- 只读审查目录：`审查检出目录`。本地 HEAD 为 `a50502f5b5073dd90ff88a1f3fcb39e08f9f24cf`，tree 为 `927bbc7f7354b2cd6f4b84fdd84c9686692174d2`；已通过 GitHub REST 核实远端 main `72e08db` 与该 tree 相同。因此以下路径/行号可作为同树规划依据。
- 本轮没有修改仓库，没有新运行 Rust 单测/WASM/浏览器。PATH 中未发现 rustc；本轮未重复运行长耗时测试。本文的“已有测试”表示测试源码存在，历史 CI 证据只适用于其标注的精确提交，不能写成当前 main 全部重新通过。
- 本轮没有发现足以直接宣布 P0 的新故障。最明确的工程剩余是：外置静态资源目录检查/打开间隙；实时监测的剩余 JSON 边界；明确声明的 OpenAPI 覆盖缺口；稳定 i18n key 尚未全站完成；真实 HTTPS/移动设备/物理硬件验收仍有环境依赖。
- **UUID 和硬盘备注已经实现，不是空白功能。** 不应在规划书写“新增 UUID/备注”。应写“保留现有功能，补用户操作说明、故障与真机验收、草稿体验”。

#### 1. 已实现且应保留的产品与安全边界

##### Web 与文件管理

- Rust/WASM CSR，复用 web-interface，不依赖业务 JS/TS；仅声明现代浏览器能力契约，而非所有浏览器品牌兼容承诺。`docs/components/web.md:3–7,139`。
- 默认文件页，磁盘目录区分逻辑角色与业务可写状态；有界目录每页最多 200 条，服务端排序/搜索、250ms 输入合并、读取取消和会话/请求代次隔离。`docs/components/web.md:15–17`。
- 文件历史树保留真实父边、继承来源、路径节点、pending 文件分支、目录分支组合；批量 namespace CAS、合并、标签、备份配置已有相应入口。`docs/components/web.md:18–24`。
- 文件/版本/路径节点备注是 128 UTF-8 字节单行语义，不能套到磁盘多行备注。文件正文不通过 Web 上传、下载、预览或写入，这是明确产品范围，不是缺陷。`docs/components/web.md:25,31`；`docs/16-client-interfaces.md:39–41`。
- 只有一个管理员，账户 CRUD、角色、仓库 ACL 不属于待补齐功能。`docs/components/web.md:29`；`docs/components/daily-management-coverage.md:35`。
- 写入 Unknown 保留原 operation ID，使用 `/api/operations/{operation}/outcome`；诊断 GET 才带健康观察。不会把 HTTP 2xx/超时/取消推断成已发布或回滚。`docs/components/web.md:24,41–45`。

##### 硬盘标识与备注

- `src/contract/src/disk_details.rs:36–74` 定义严格 `DiskDetailsRecord`、`DiskNoteRequest`、`DiskNoteResponse`，拒绝未知字段，nullable 观测字段与确定性身份分开。详情包含登记目录、实际挂载目录、型号、序列号、容量、设备路径、文件系统 UUID、ZXS 登记磁盘 ID、mapping、note。
- Web 完整展示长标识和明确身份标签，采用安全文本输出；P01/B01 不是机箱盘位，也不代表独立故障域。`src/web/src/admin/disks.rs:405–418,459–475,502–510`。
- 硬件观察从已验证并持有的 root FD 获取 mount ID，不重新按 replaceable pathname 认盘。filesystem UUID 经 `/dev/disk/by-uuid` symlink target 的 block major/minor 匹配，不冒用 NVMe namespace UUID；虚拟/堆叠多设备不猜测单一物理盘。`src/server/src/disk_hardware.rs:95–135,138–183,210–231,369–419`。
- 当前型号/序列号实际仅从确认的 NVMe controller 读取。SATA/SAS 不做兜底猜测；这是显式保守支持范围，不应报成普遍的硬件识别故障。`src/server/src/disk_hardware.rs:224–231,352–360`；`test/server/unit/disk_hardware.rs:165`。
- 备注按登记 disk_id 写到系统盘私有目录，不更改数据盘 seal；`expected_disk_id` 防换盘错写，`expected_note` CAS 防静默覆盖，同值重试仍经过持久化屏障。`src/server/src/management.rs:903–930`；`src/server/src/management/disk_notes.rs:18–21,84–123`。
- 每卡独立加载/出错，刷新不覆盖当前编辑文本与 CAS 基线，晚到读取不能回滚成功保存；保存返回必须匹配 slot、disk ID 和文本。`src/web/src/admin/disks.rs:83–100,207–265,269–294,326–397,492–496`。
- 已有测试覆盖多行文本、安全渲染、空值清除、冲突、原 CAS 重试、读写竞态、会话代次、登记变更、持久化重开、符号链接拒绝、权限/CSRF。代表位置：`test/web/unit/admin/disks_tests.rs:55,81,129,200,249,302,348,382,406`；`test/server/unit/management/disk_cards.rs:62,114,245,343,391`；`test/server/unit/management/disk_notes.rs:20,39,56,78,100,122`。
- 浏览器场景 `test/tools/browser-smoke/runner/metadata.rs:178–384` 使用真实临时服务保存/读回/取消/清空，只有硬件 unknown 观测是合成；实际运行时会输出 `PASS real temporary management-disk notes...`。不能把这项证明写成物理 UUID 或真实设备映射验收。

##### 监测与运行状态

- 实时 CPU、内存、盘容量和可获得的真实功耗；缺失不补 0，不按 TDP/利用率推算功耗。后台历史独立于浏览器，最近 1h 近点、31d 分钟聚合，支持 5 个范围、最多 361 点。`docs/components/server/src/telemetry.md:9–23,27–43,56–58`。
- 前端已有范围切换取消、epoch/generation 围栏，历史 ingress 完整 DTO、UTC 解析、共享时间和 disk ID 索引、等值 Snapshot 与 SVG 保持。不能将这些已完成项重写成未开发，也不能据此声称减少网络下载。`src/web/src/monitor.rs:203–284`；`src/web/src/monitor/history.rs:18–56,74–155`；`docs/components/web.md:175–180,204–208`。
- 状态页在可见时每 5 秒拉 status/history/capacity/scrub，隐藏/BFCache 暂停新后台读取；不是后台继续访问所有页面。`src/web/src/polling.rs:29–57,86–111`；`src/web/src/monitor.rs:66–89`。

#### 2. 建议工作包与优先级

##### WEB-01：外置静态资源描述符安全打开（P1，已有明确边界；部署条件相关）

**证据/现状**：`web_assets::static_etags` 启动时遍历、lstat 拒绝符号链接并读取散列，但后续 `fallback` 使用 ServeDir 按路径重新打开。`src/server/src/web_assets.rs:20–25,55–100,114–120`。`docs/management/control-plane.md:79` 已明确这是未解决的检查/使用时差。默认嵌入式资源与 `--web-dir` 外置目录应分开评估，不能说所有安装都受同样风险。

**影响**：外置资源树准入后被替换时，可能读取非准入对象，且启动时 ETag 可能与实际响应字节不一致。当前运行契约要求操作者控制且保持树不变。没有本轮实际攻击复现，不能写成已发生泄露。

**改进**：保持已有资源上限、API namespace、CSP、gzip、ETag/HEAD/条件请求语义；建立从准入根直到响应完成持有 FD 的描述符相对安全打开路径，禁止链接穿越。若选择启动时固定内容快照，先给出内存/启动成本预算，而不是无界全量缓存。

**验收**：准入后替换文件、父目录、根路径、`.gz`；普通文件→symlink/FIFO；并发替换与流式响应；GET/HEAD/304、gzip/identity 各自 bytes/validator 一致；未知 `/api` 永不返回静态文件；资源失效不影响独立管理 API。继续跑 service_startup + 静态服务单测 + 真实 WASM browser-smoke。

##### WEB-02：实时监测完整 DTO 与错误语义（P1/P2，边界一致性提升，不是数据损坏结论）

**证据/现状**：`src/web/src/monitor/observations.rs:1,10–20,26–36` 明确只验证 `disks[].id/observation_failure`。`monitor.rs:25` 仍存 `Value`；`286–309,356–429,454–479` 按字段查值并将许多错误类型/缺失转为“不可用”。例如仅含 `{"disks":[]}` 的形状通过当前局部 decoder 后可成为 loaded 快照；非法 CPU/online 值没有完整 wire-level 错误。这与 `monitor/history.rs` 的严格完整 DTO 不同。

**影响**：真正的传感器 null 与协议缺字段/错类型可能被混成“不可用”，影响故障诊断；不能据此声称服务端正在发错数据、写入安全围栏失效或已经误报健康。

**改进**：定义只针对当前服务端 status 形状的私有 typed DTO，在 ingress 一次区分必填 nullable、数值边界、布尔、时间、disk identity；状态模型保留原 error source，展示层只处理已验证数据。不添加旧格式兼容，不复用实时 DTO 强行改变历史桶均值语义。

**验收**：真实 server status → Web decoder 联合测试；逐字段缺失/null/错类型/未知字段；duplicate/empty disk ID；0 与 null、离线、sampling、sensor unreadable、disk_sampling_unavailable；错误后旧数据/错误提示策略明确；当前会话 401 优先、晚到旧会话拒绝。对合法响应做差分，CPU/容量/功耗展示保持不变，历史离线桶平均值不丢失。

##### WEB-03：全站稳定国际化 key（P2，已知未完成治理工作）

**证据/现状**：管理技术表单已采用有限 `Message` enum 与中英映射，见 `src/web/src/admin/messages.rs:39–106,108–179`；但新硬盘卡仍直接 `t`。`docs/components/web.md:29` 明确全站未完成。对 `.rs` 作正则 `\bt\s*\(` 盘点：web 536 处/33 文件，interface 44 处/8 文件；包含集中映射入口和函数定义，不是唯一文案数量。

**建议批次**：1) inventory 与术语表，区分用户可见文本、ARIA、错误、本地协议标识与远端诊断；2) 登录/账户/初始化/管理健康/硬盘备注；3) 文件/历史/批量/备份；4) 状态/巡检/共享组件。按功能模块分次提交，不把请求模块引入浏览器依赖。

**约束**：保留协议 action/type/JSON 字段、原始诊断与 UTC 展示；typed placeholders，不对用户文本二次模板替换；本次 key 迁移不要求改成无重载实时切换。

**验收**：每 key 中英完整、占位符数量/含义一致、无漏 ARIA/title、相同短语按语境共用而非盲合并；`lang` 参数→已存偏好→浏览器语言→English 的现有优先级不变（`src/interface/src/i18n.rs:65–107`）；显式语言切换仍保留会话与 unknown guard；中英文、窄屏、长错误、IME/NFC/字节预算回归。不能仅用 grep 为全站完成背书。

##### WEB-04：普通草稿与语言切换的产品边界（P2，源码确认的 UX 风险；须先定行为）

**证据/现状**：`src/interface/src/i18n.rs:175–185` 用 location.assign 重新加载。`src/web/src/ui.rs:85–93` 只聚合会话/改密/重试/管理/文件 pending；`src/web/src/admin/mod.rs:95–96` 仅检查 busy、uncertain operation、disk saving；`src/web/src/files/actions/mod.rs:55–62` 仅 mutation_pending/Unknown。磁盘已编辑但未保存、保存失败后保留文本等普通草稿不一定阻止语言重载。

**影响**：语言切换可丢失内存中未提交文字；这不等于存储发布 Unknown 围栏坏了，也不证明服务端写入丢失。文件普通草稿也应纳入统一盘点，不能只补一个卡片。

**决策与改进**：选择“存在编辑器时禁用应用重载并说明先保存/取消”或“明确提示放弃草稿后切换”；不默认把敏感草稿写入 localStorage，不扩大为全业务门禁。磁盘 CAS 失败仍保留原文、原 expected_note，不能刷新后静默替换依据。`docs/validation/ui-g4-browser-acceptance-20261031.md:32` 中“未完成草稿”宜限定到实际被 pending/unknown 持有的草稿，避免泛化承诺。

**验收**：普通磁盘备注、空值清空草稿、409、网络未知、列表/卡刷新、语言切换、离开后回来、会话失效、保存晚到；文件 inline/batch/backup 草稿同类矩阵。验证实际 DOM/按钮与导航结果，不只状态 reducer。

**附加低优先 UX**：磁盘备注只受总 HTTP 1MiB 限额，更新体含 expected_note 与新 note；大文本可能保存前才遇到 413。可加当前请求 UTF-8 编码体预算提示，不能误加文件备注的 128 字节限制，也不把“无限多行”当承诺。`src/server/src/management/disk_notes.rs:95–104`。

##### WEB-05：OpenAPI 覆盖与可执行文档（P2；是已声明覆盖缺口）

**静态盘点**：对 `src/server/src/**/*.rs` 的 literal `.route("/api/...")` 提取并统一参数名，共 94 个不同 API 路径；OpenAPI 47 个路径/51 个 operation；OpenAPI 的 47 个路径均找到注册，另 47 个注册路径未入 OpenAPI。此数字是静态 path 盘点，不是运行时方法覆盖率，不代表有 47 个坏接口。

**缺口类别**：management/status、storage/retry、setup/recovery、scrub、repositories/branches/policies、plans、workspace 创建/写入/封存/中止、events/compare 等。注册证据 `src/server/src/lib.rs:1850–1984`、`src/server/src/http_integrity_scrub.rs:17–18`、`src/server/src/setup_recovery/http.rs:13–28`。

**既有说明**：`docs/contracts/README.md:5,12,56` 已声明附件局部覆盖和待补齐，因此不用“文档撒谎/接口无文档”措辞。

**改进顺序**：先 management/status/setup/scrub 这批 Web 真实消费端点，再日常管理请求/响应/错误，再完整正文 API。维护 route-method 与 OpenAPI 差异清单，暂未建模项须有显式豁免与原因，不把设计接口当已开放。真实 Router 响应过 schema + Web decoder，避免只验证手工例子。

**低成本同步**：`docs/components/daily-management-coverage.md:8–30` 缺少硬盘详情/备注、巡检/outcome 区分条目；`docs/15-api-and-errors.md:147` 的 `op-example-0007` 不是当前 OperationId（`src/contract/src/operation_id.rs:5,20–28` 要求 op_+32 小写 hex）。示例本身还含哈希占位，不能当可直接发送的完整请求；可替换为合法合成 ID 并标注其余占位，或用向量生成示例/测试。

**验收**：schema/core 共用定义一致，所有新增 method、auth/CSRF、status/error/413/timeout/Unknown 语义正确；request/response roundtrip；文档本地链接、示例结构与真实 validator；不对线上未支持磁盘注册、换盘、目录合并作成功承诺。

##### WEB-06：跨标签/重启的待登出意图（P1 设计研究，非本轮已证漏洞）

**证据/现状**：当前 LogoutUnconfirmed 页禁止新业务/登录/初始化并保留撤销凭据，精确成功回执才解围栏；但它只是内存状态，手动刷新、关页、浏览器重启、另一个标签可能继续恢复有效 cookie。`docs/management/control-plane.md:39–47` 已清楚披露；`src/web/src/ui.rs:25–39,67–103,303–330`。

**用户风险**：用户误以为“关页/新页面=登出完成”。现有页面文字已提醒，不应删除或模糊提醒。持久跨标签意图不是已实现功能。

**规划**：先确认是否需要产品级跨标签保证，再定义最小无秘密的撤销意图、同源协调/重启恢复、服务端确认条件与锁死恢复路径。不得将 token/密码持久化到前端存储，不把仅收广播视为服务端撤销证明，不自动重发非幂等改密。安全设计优先于加一段 localStorage 代码。

**验收**：两标签先后/并发注销、传输丢失、服务端已成功但响应丢失、服务端仍有效、手动刷新、BFCache、浏览器重启、存储不可用/禁用、旧标签/旧 401、新会话 epoch；撤销不成功时所有已覆盖恢复入口不能重新获得业务准入，明确保留人工排障出口。

##### WEB-07：技术管理表单的可读审阅层（P2/P3，体验增强，不是故障）

**证据/现状**：管理页要求手填 repository/branch/history ID、预期 generation，查询结果/完整计划主要通过 `<pre>` 显示。`src/web/src/admin/mod.rs:314–329`。安全上已有固定预览、摘要与显式确认，输入改变会使计划失效（`:99–110`）。

**改进**：保留完整 JSON 诊断，额外提供可读的目标/影响/数量/不可逆后果/摘要/状态；逐步用已认证读取结果提供 ID 选择器，原手工 ID 可以保留为技术入口。不能自动选择目标、改代数后重试或用“漂亮摘要”替代完整预览与确认。

**验收**：实际请求捕获身份与审阅对象严格一致；用户编辑任一依赖后重新审阅；旧请求晚到不重启旧计划；未就绪/拒绝/取消/Unknown 表示准确；移动窄屏长 ID 和多行错误可复制且不截断。

##### WEB-08：目标设备、HTTPS 与硬盘真机验收（P1 发布门槛，依赖既有授权环境）

**现有证据**：`docs/validation/ui-g4-browser-acceptance-20261031.md:7–19,36–45` 有精确历史提交的真实 Chrome/WASM/字体/G4/故障 witness；6 个 CSS 尺寸固定 DPR1、mobile:false、桌面 UA，不是 iOS/Android。`同文:49–50` 明确移动设备与真实 HTTPS 入口待验收。

**范围**：按实际发布目标记录设备、OS、浏览器版本、DPR、触控、输入法、HTTPS 证书链/代理路径/Cookie/CSRF；磁盘记录 NVMe/分区/整盘/bind mount/同底层设备/多设备/缺失 UUID/权限不足，逐项核对 complete serial、filesystem UUID、registered disk ID，不能依靠目录模拟盘证明物理独立。

**验收**：同一发行构建的正常与中断路径：登录恢复/登出、中文 IME/NFC、备注编辑与冲突/未知、窄屏横滚、复制完整 ID、状态数据/缺失、范围切换、刷新/Back/Forward/关闭、HTTPS 真实失败边界。允许自动化的部分继续自动化。无目标设备或入口就登记“未执行/环境待提供”，不擅自部署、改证书/权限/凭据。

##### WEB-09：监测表达的细节增强（P3，先测量，不能夸大性能收益）

- `monitor.rs:503–508` 直接用 wire range 值作为 label；可本地化为“最近1分钟/30分钟/1小时/24小时/30天”，value 仍为原 wire token。避免“1month”被误解为自然月。
- transport error 保留旧 snapshot（`monitor.rs:83–88`），页面有错误和采样 UTC（`:515,529`）；可进一步明确“最后成功采样/当前已过期”，而非把旧数值当最新。阈值应与实际采样/超时协议一致，不额外高频轮询。
- 降低 history 网络量、G4 局部几何或新 IOPS/SMART 指标属于独立测量/设计任务，当前只确认需求与可量化收益；已有共享投影/等值 SVG 不可再计一次优化，也不可强推已被负例否定的增量方案。

#### 3. 执行顺序与退出条件

1. **先对齐事实与小文档项**：保留硬盘 UUID/备注“已实现”；修示例 ID、管理功能矩阵；建 API method inventory 和 i18n inventory。退出条件是内容能对应真实消费链与精确代码/测试路径，不是仅报告生成。
2. **再处理边界与高风险设计**：外置静态资源安全打开；实时监测完整 DTO；跨标签撤销先设计再实现。单个边界独立 PR/提交，保留原协议和 unknown 语义。
3. **随后分模块 i18n/UX**：不与协议/持久化改变混在一批；普通草稿/语言切换先定规则，再补真实浏览器测试。
4. **发布验收并行准备**：锁定目标环境与精确发行构建，整理现有真实 Chrome 证据；不能用本轮静态审查抵消未执行的 HTTPS/移动/硬件项。
5. **性能增强后置**：只有基准能证明瓶颈和收益时才动 polling/聚合/G4；UI 测试计数不等于吞吐、RSS 或网络字节。

#### 4. 最小验证组合

- 原生：`cargo test --locked --offline -p web-app -p web-interface -p view-macros`；对应 storage-server lib 单测及 `--test http_engine`、`--test telemetry_status`、`--test service_startup`。
- 契约：`cargo run --locked --offline -p contract-check`，但它只证明声明范围，不能替代 endpoint 行为。
- Web：按 `docs/components/web.md:147–157` 离线构建并 check；WASM clippy；`docs/tools/browser-smoke.md:11–17` 的真实生产 WASM Chrome 命令。工具编译/工具单测不等于 browser PASS。
- 每次交付分列：新增/修改测试已通过；既有精确提交可复用证据；本次未运行；环境阻塞；仍待产品决策。数据盘模拟、合成响应、真实临时服务、物理设备四类证据分开记。


---

## 9. 专项审查与工作包

### ZXS SMB、Debian 13 与部署审阅

审阅日期：2026-11-03。范围：SMB/Samba VFS、Debian 13 amd64 发行、单个服务端 ELF、系统运行依赖、systemd、`/zxs` 路径与外部验收。此轮未修改仓库、未安装或启动服务、未更改账户/凭据/系统权限。

#### 1. 结论与证据身份

目标架构已基本落实为：**一个内嵌正式 Web 的 `zxs` ELF + Debian apt Samba + 与该 Samba 精确匹配的独立 `zxs.so`**。这不是一个完全静态、零系统依赖的 ELF，也不是把 smbd 内嵌进 ZXS。`/zxs` 安装布局和 `0.0.0.0:7400` 默认监听已体现在代码/模板中，不需要重新设计协议入口。

本轮读取 `审查检出目录`，实际本地提交 `a50502f5b5073dd90ff88a1f3fcb39e08f9f24cf`，树为 `927bbc7f7354b2cd6f4b84fdd84c9686692174d2`。已通过 GitHub REST 核实远端 main `72e08db` 为同一树。本报告的源码结论适用于该树；**同树不能把另一个提交的构建收据改写成本提交收据**。

本轮实跑 31 个相关脚本/契约回归，全部通过。没有执行当前树的实际 Samba SDK 编译、smbd 加载、目标机 systemd 启动或真实客户端/物理盘验收。历史收据只能证明它们所绑定的旧候选。

Debian 官方包页在本轮核查时仍列出 trixie Samba `2:4.22.11+dfsg-0+deb13u1`，与仓库机器合同一致：[Debian trixie Samba](https://packages.debian.org/trixie/samba)。精确版本固定用于 ABI 验证身份，不能成为停止安全升级的理由。

#### 2. 源码中确实已有的能力

##### 2.1 发行与依赖

- `src/server/build.rs:1–49` 只在正式构建设置 `ZXS_EMBED_WEB=1` 时生成 Web 内嵌产物；普通开发构建可为空 bundle。因此“任意 cargo build 的 zxs 都自带 Web”不成立。
- `.github/scripts/release-native.py`、`release-package.sh`、`release-assemble.sh` 与 `verify-release-assets.py` 实现服务 ELF、VFS、资料包、收据、安装说明和 SHA256SUMS 的七文件发行集合。`zxs-rescue` 位于资料包，是可选离线工具，不是服务端运行的第二个必需进程。
- `.github/scripts/native-runtime-receipt.py:93–129` 对实际 ELF 做 `readelf NEEDED`、`ldd` 解析，含传递库和 loader，记录文件 SHA、Debian 包名和精确版本。构建并不把这些共享库复制进 ZXS。安装说明以此收据选择运行库，而不是要求目标机安装构建容器的开发包。
- `.github/scripts/check-debian13-single-binary.py:18–92` 在 Debian 13 amd64、无 Cargo/cc 的运行环境，将 ELF 拷到独立临时目录、不传 `--web-dir`，逐资源核对内嵌字节 SHA。`.github/scripts/release-acceptance.sh` 另让宿主有正常沙箱的 Chrome 访问运行在 Debian 容器中的该发行 ELF。两者分别证明静态资源交付和浏览器流程，不是物理部署验收。
- `.github/workflows/release.yml` 的 `linux-web` 等待 `smb-sdk`，最终组装等待完整 `verify`、`linux-web`、`smb-sdk`。仅 publish 有 contents write；workflow_dispatch 只演练构建、不创建公开发行。

**重要限定：**根 `Cargo.toml:68` 使用 `sqlite-bundled`，`src/platform/Cargo.toml:20` 使用 `libsqlite3-sys` 的 bundled 特性。当前不能说“所有原生依赖均动态来自 apt”。“通用共享运行库由 Debian 提供”与“SQLite 也必须由 Debian 动态提供”是两个不同目标。若用户要求后者，应作为明确依赖变更任务，而不是把现状包装成已完成。

##### 2.2 Debian Samba / VFS ABI

- `.github/samba-debian-target.json` 固定 Debian 13/trixie/amd64、包版本 `2:4.22.11+dfsg-0+deb13u1`、VFS interface 50，以及上游原档/发行补丁的 SHA256。
- `.github/scripts/smb-sdk.sh:10–48` 核对实际 apt 包，下载同版本 Debian 源包，核原档/补丁哈希，执行 Debian 的 `debian/rules configure`，仅缩小 SDK 编译目标。
- `src/smb-vfs/build.rs:1–59` 校验 Linux amd64、Debian changelog、Samba VERSION、interface 50 和生成配置头；后续 bindgen 读取真实内部头，不维护猜测布局。
- `src/smb-vfs/src/module.rs:1372–1446` 在注册回调之前比较实际 `samba_version_string()` 为 `4.22.11-Debian-4.22.11+dfsg-0+deb13u1`；不同发行构建不能仅凭 interface 相同就加载。
- `src/smb-vfs/src/lib.rs` 与 `.github/scripts/smb-vfs-check.sh` 要求 unwind；正式 SDK 门禁包含 Clippy、实际 VFS 测试编译/执行、panic=abort 负例和正常 release 正例。`--build-only` 明确不是运行验收。
- `.github/workflows/smb-sdk.yml:27–49` 在第二个无编译器、`--network none` 的 Debian 容器里运行真实 apt smbd，按宿主 `smbd -b` 给出的 `MODULESDIR/vfs/zxs.so` 只读挂载模块。配置使用 `vfs objects = zxs`，不是绝对模块路径。

##### 2.3 SMB 身份、数据与恢复边界

- `src/smb-vfs/src/module/authentication.rs:13–58` 从真实 Samba session token 读取 account_name，拒绝 force-user、anonymous、guest、无 Authenticated Users 成员资格及缺失身份。
- `src/server/src/smb_rpc/transport.rs:76–134` 把显式单一 Samba 用户绑定到唯一当前 ZXS 身份和仓库主分支，并捕获 binding generation；不是通用多用户 NAS ACL 或 AD 集成。
- 只读正文通过协商的 verified-range-reads RPC 获取；不会把不可变正文 FD 直接交给 VFS。每次 Read，包括缓存、EOF 和零长度，仍执行当前授权。写句柄使用服务端私有完整工作文件，SCM_RIGHTS 交 FD，不交内部磁盘路径。
- `PrepareWrite` 在每次修改前做授权/工作区/额度/恢复空间准入；Dirty 按保存代通告，FLUSH、write-through 和最终 CLOSE 保留原持久保存语义。断连不是成功保存，也不是自动放弃。
- 活动 Flush 可核对原 operation_id 后重试；失败 Close/Detach 终结句柄、释放路径锁但保留正文/恢复记录/预算。未知结果不得换新 operation_id 盲目重做。已有 socket 不自动覆盖，正常退出只移除自身 device/inode 对应的 socket。
- `deploy/samba/smb.conf.example:6–38` 配置 SMB3 最低版本、强制签名和加密，禁 guest、oplocks、durable handles、sendfile、EA、symlink。
- `src/smb-vfs/src/module.rs:1201–1360` 明确拒绝 chmod/chown、客户端时间设置、分配、链接、xattr、ACL 修改、压缩、FSCTL 与复制卸载等不支持的变更路径。它们是当前产品子集，不应为“兼容”而落到宿主文件系统旁路。

##### 2.4 `/zxs` 与 systemd

`deploy/systemd/zxs.service:7–42` 已有：

- 专用 `zxs` UID/GID、UMask 0077；程序 `/zxs/zxs`、工作目录 `/zxs`。
- `--root /zxs/data/state`、`--management-root /zxs/data/management`、示例盘根 `/zxs/data/disks/P01`。
- HOME、TMPDIR/TMP/TEMP、全部 XDG 路径指向 `/zxs` 下；应用 stdout/stderr 追加到 `/zxs/log/zxs.log`。
- 无新 capabilities、NoNewPrivileges、ProtectSystem=strict、ProtectHome、DevicePolicy=closed、精确 ReadWritePaths。
- SIGINT 请求排空；`TimeoutStopSec=infinity` 不会把五秒网络宽限误当持久化结束时间。卡死内核 I/O 的显式强停属于崩溃恢复，不是成功优雅退出。

`docs/operations/deploy/systemd.md:18–50` 和安装模板明确 root 所有且服务不可写的程序树、私有目录 0700/文件 0600；`/zxs/smb-export` 为 root 所有 0555 的独立空目录，`/zxs` 0755 允许 Samba 认证用户遍历。socket 位于 `/zxs/run`，SMB spool 固定在持久 `/zxs/data/management/smb-spool`，不能放到 run/tmp 或自动清理。

系统 unit、apt Samba、系统共享库与 VFS 模块仍在发行版系统目录，这是所选架构的必要例外。systemd 自身状态日志也属于系统记录。不要为“所有东西都在 /zxs”而复制一套 Samba/系统库。

`src/server/src/main.rs:65–66` 默认监听确为 `0.0.0.0:7400`，与模板一致。CI 的显式 `127.0.0.1` 是隔离夹具，不应反向改掉产品默认。直接 HTTP 的暴露范围、明文传输风险需由部署者控制；可选 HTTPS 代理的 external-origin/trusted-proxy 要按真实拓扑设置。

**路径限定：**裸命令默认 `--root ./zxs-data`（`main.rs:50–55`），`/zxs` 是正式安装配置保证，不是任意 cwd 下无参数执行的硬编码保证。不要将两者混写。

#### 3. 已确认缺口与不可越过的验收边界

##### A. 验收输出可能保留上次成功文件，建议优先修复

`.github/scripts/check-system-samba.py:69–71,295` 与 `check-debian13-single-binary.py:23–28,92` 只有成功末尾写 JSON，开始时不撤销旧结果。本轮以临时目录预置 `{"previous":"success"}`，分别触发隔离前提失败和编译器存在失败，确认脚本失败而旧输出仍在。未启动真实服务。

这会使**复用目录的人工/脚本阅读者误认旧成功**。正式干净 CI 的 job 失败仍阻断发布，因此不能声称已绕过公开发布门禁。修复可沿用 `native-runtime-receipt.py:112` 的先失效旧收据模式，成功后再发布新收据；增加“上次成功→本次前置失败/中途失败”的回归，并避免误删非本次管理的外部路径。

##### B. 运行证据和交付身份仍主要靠 CI job/artifact 关联

`VFS-BUILD.json` 在 `smb-sdk.sh:74–78` 记录 `runtime_acceptance: not-run-by-build-script`，这是诚实区分；最终七文件不含 system-samba.json。`verify-release-assets.py:98–106` 绑定模块/源码/目标，但不验证独立运行通过摘要。正式 workflow 依赖确实等待真实运行 job 成功。

建议在不混淆 build-only 的前提下，将实际运行结果摘要（源码、模块/ELF SHA、夹具 SHA、包版本、必需检查集合、run/job URL、状态）与发行收据明确关联。不能只改字符串为 passed，也不能用旧候选、同名 artifact 或编译成功顶替。

##### C. systemd 和真实 `/zxs` 部署尚需独立验收

容器里直接 Popen 引擎/ELF不证明 systemd 的 UID、挂载命名空间、只写指定目录、设备拒绝、启动/停机顺序、日志打开或重启恢复。`systemd-analyze verify` 也只是语法/依赖检查。

本轮运行 verify 在 `/run/systemd/` 只读处失败，未进行权限绕过。应在获授权的 Debian 13 VM/目标机上验证完整单元；保留真实主机部署独立授权，不能把仓库修改许可当作部署许可。Debian 的 systemd 手册也说明 WorkingDirectory 会产生访问该路径所需的隐式挂载依赖：[Debian systemd.exec](https://manpages.debian.org/trixie/systemd/systemd.exec.5.en.html)。因此 `/zxs` 必须位于可用系统盘，不能把管理可用性建立在故障数据盘上。

##### D. 真实客户端兼容矩阵仍未闭合

当前自动门禁使用 smbclient，覆盖真实登录、错误口令/匿名/force-user 拒绝、既有正文读取、put/get 字节、rename、既有 tree 撤权及删除，并由真实 EngineBackend 核对。它没有等价覆盖 Windows Explorer/Office、macOS Finder、Linux CIFS 挂载和实际编辑器的临时文件/替换/多句柄保存模式。

`docs/components/smb.md:239,252–258` 已明确 write-through、FLUSH、断网重连、编辑器保存和真实断电需另验；没有自动重新挂接旧句柄。时间只投影已发布版本保存时间，客户端设置时间被拒绝，目录枚举后 Samba 按名打开的新 pathref 也不保证冻结到先前目录页版本。规划应先划支持/不支持行为，再用客户端证据决定改动，不能承诺完整 POSIX/Windows 文件系统语义。

##### E. 资源和吞吐需要真实链路预算，不能仅据局部优化下结论

- 服务桥接：64 个连接、8 个认证会话、每会话 1024 句柄（`src/server/src/smb_rpc.rs:25–27`）。
- 读 RPC：每次最多 64 KiB，仍为 JSON 数字字节数组；同连接请求串行。客户端填满一次宿主读可能执行多次 RPC（`src/smb/src/protocol.rs:52–53`；`client.rs:607–627`）。去掉 Value 中间数组不等于已测得端到端吞吐收益。
- VFS 异步池：每 smbd 进程内的静态池，2 worker、8 排队槽、单次 I/O 上限 8 MiB（`asynchronous.rs:11,37–59`）。必须额外算已完成未回收结果、其他宿主状态和多个 smbd 进程，不能直接把局部上限宣称全机 RSS 上限。Samba 为客户端会话派生服务进程，见 [Debian smbd](https://manpages.debian.org/trixie/samba/smbd.8.en.html)。
- 每会话一个最大 4 MiB 已验段缓存，8 会话合计最多 32 MiB 正文缓存；清单共享证明预算约 1 GiB，不包括其他独立任务。
- 每活跃 writer 可多持一个恢复记录 FD，极端 8192 个 writer 额外 8192 FD/128 MiB 编码记录；unit 未显式设 LimitNOFILE，必须按目标机实际软硬限制核算和压力验证。
- 默认 `--max-upload-bytes` 为 1 GiB，也限制 SMB 可写文件；SMB spool 账户/总额默认为 100/500 GiB，恢复保留 1 GiB（`main.rs:75–111`）。对大视频/镜像等使用场景须明确合适配置，而非默认无限扩大。
- 日志为持续追加，未自动轮转。应有容量告警和安全归档验收，不能用清空恢复材料解决满盘。

##### F. 安全更新、恢复与物理故障不能算完成

Samba apt 升级会触发精确版本拒绝，必须建立“新包→真实 SDK→模块→同样客户端门禁→安装/回退计划”的流程。不能长期 hold 易受攻击旧包，也不能放宽版本检查冒充兼容。已有池不迁移、不自动重置、缺盘不自动降低仲裁。

实际设备独立故障域、冷启动挂载时序、盘掉线/替换、满盘/只读、掉电和 fsync 行为仍需外部测试。普通目录、单盘夹具、LD_PRELOAD EIO 与进程级重启各自有价值，但不能升级为物理冗余或断电证据。

#### 4. 建议进入根规划书的工作包

| 工作包 | 前置依赖 | 具体工作 | 验收与停止条件 |
|---|---|---|---|
| DEP-01 固定候选证据 | 确认提交/树 | 收集当前 SHA 的 SDK、system-samba、单 ELF、浏览器和完整 CI 终态；逐项列 passed/failed/not-run | 产品 hash、源码、包版本和 run/job 完整一致；缺任何项就保留未验收状态 |
| DEP-02 修正验收输出生命周期 | 不依赖部署 | 两运行脚本先失效自身旧成功输出，成功发布新结果；加失败重跑回归 | 预置旧成功后，各前置/中途失败均不能遗留可被误认的通过收据；正常成功仍完整保存日志 |
| DEP-03 完整发行证据关联 | DEP-01/02 | 把 build-only 与真实验收区分固化到收据/验证器；记录夹具与产品 hash、目标版本和检查集合；保持七文件规则或有意升级其合同 | 修改任何产品/收据/必需检查都会拒绝；正式 CI 与本地演练行为一致，旧证据不可换标签复用 |
| DEP-04 明确运行依赖合同 | 用户目标中的依赖定义 | 明确 bundled SQLite 是否允许；按实际 ELF 生成最小 Debian 运行包清单；如要 unbundled，贯通 platform/engine特性和真实 SQL 回归 | 最小运行环境无编译器、无 SDK、无额外 Web 目录可运行；NEEDED/loader 全有 Debian 所有者，缺库给明确失败 |
| DEP-05 制作只读安装预检 | DEP-01/04 | 对 `/zxs` 权限/真实目录、binary/VFS哈希、Samba版本、MODULESDIR、socket/spool分离、配置一致性、磁盘/FD/空间预算生成报告 | 报告不创建账户、不改权限、不删 socket、不启动服务；能识别版本错配、私有树被导出、旧socket、路径不可写与低空间 |
| DEP-06 systemd 隔离 VM 验收 | DEP-05；独立部署授权 | 使用完整 `/zxs` 布局/专用账户；首次引导、正常停止/重启、数据盘不可用管理面、晚到挂载、日志和只写范围；Samba正常/倒序启停 | 管理可达和能力状态正确区分；不会写到预期树外；不授予额外设备权限；停止不提前宣称成功；遗留证据完整 |
| SMB-01 客户端支持矩阵 | DEP-06、协议合同固定 | Windows/macOS/Linux真实客户端：下载/覆盖/重命名/删除/Unicode/大文件；编辑器临时替换/多句柄；FLUSH/write-through；断网与撤权 | 每个组合保存版本/字节、错误码、客户端结果和恢复证据；不支持操作明确拒绝且正文/历史不损坏 |
| SMB-02 真实链路性能预算 | SMB-01；同硬件/同数据集 | 冷/热读、顺序/随机写、大目录、1/多会话；记录吞吐/p50-p95、CPU、RPC/编码、RSS、FD、fsync、spool与保存代计数 | 优化前后同条件可比较；无新越权/保存语义变化；只根据实际瓶颈决定是否改 JSON/批量/池或缓存 |
| DEP-07 更新和恢复演练 | DEP-01/06、恢复流程 | 新 Samba版本精确重建/测试；旧模块拒绝；候选替换失败回退；SIGKILL/断网保留与原操作核对；单独真实断电测试 | 不复用旧 handle/新 operation_id 掩盖未知结果，不删除未确认数据；回退不自动迁移格式；物理验收单独签结论 |

顺序建议：先 DEP-01/02，DEP-04 的需求边界同时澄清；代码/脚本层可并行，之后 DEP-03/05；实际环境授权后 DEP-06，再开展 SMB-01、SMB-02、DEP-07。不能为了先跑性能绕过身份、完整性、原操作核对或 fsync。

#### 5. 本轮验证清单

通过，均在同树工作区，`PYTHONDONTWRITEBYTECODE=1`，结束 `git status --short` 为空：

1. `python3 -m unittest discover -s test/ci -p 'test_system_samba_gate.py' -v`：8/8。
2. `python3 -m unittest discover -s test/ci -p 'test_maintained_docs.py' -v`：5/5。
3. `python3 -m unittest discover -s test/ci -p 'test_smb_build_only.py' -v`：7/7。
4. `python3 -m unittest discover -s test/ci -p 'test_native_runtime_receipt.py' -v`：11/11。
5. 两个运行验收脚本“旧成功输出 + 当前失败”临时夹具复现：均确认旧输出保留。这是证据输出缺口复现，不是实际产品运行。

环境阻塞：`systemd-analyze verify deploy/systemd/zxs.service` 返回 `Failed to create directory '/run/systemd/': Read-only file system`。未更改系统权限，未把失败写成通过。

未运行：当前树 Rust/SDK 编译、真实 apt Samba加载、单 ELF运行、浏览器、目标机systemd、真实客户端、物理盘。当前环境缺 docker、cargo/rustc 与 smbd；有 cc，亦不符合“无编译器运行镜像”前提。没有安装新工具来改变验证环境。

#### 6. 可用但不能替代当前候选的历史材料

- `历史 Debian SDK 与 Samba 运行收据`：产品 `f33758c891b103b6d14413d4e02e2685d7f6d1b0`，run `41199025476` / job `124531601230`；VFS/module hash 与运行收据匹配，真实 put/get、rename、既有tree撤权、delete 和核心状态通过。该旧收据尚不包含后来增加的独立非root身份检查，不可替代现行完整集合。[历史工作流](https://github.com/isarmg/zxs/actions/runs/41199025476)
- `历史 Debian 内嵌 Web 运行收据`：产品 `4332e1595c3f6a70c86bb58b4163d11a1915438b`，observer为另一个提交；单 ELF 的 762项资源 SHA 通过。原产品和新观察器身份被分别记录，不应重写成当前产品证据。
- 本轮在现有 `历史验证资料` 的 JSON/MD/TXT 中未找到 `72e08db`/`927bbc7...` 当前树运行收据；这是本地检索结论，不表示远端 CI 不存在。当前运行终态由GitHub 核验补充。


---

## 10. 专项审查与工作包

### ZXS 性能、可观测性与 288V/32GB/12 盘适配审查

#### 审查范围与证据等级

- 本轮为只读源码、仓内测量记录、Intel 官方规格审查，没有修改仓库、构建、运行 benchmark 或接触真实设备。
- 实际读取 `审查检出目录`，HEAD `a50502f5b5073dd90ff88a1f3fcb39e08f9f24cf`，tree `927bbc7f7354b2cd6f4b84fdd84c9686692174d2`，工作树干净。经 GitHub REST 核实 main `72e08db` 同树；该短 revision 不在此检出的本地 object 集合中，本审查独立核到的是准确 tree，不声称独立核验了远程 main。
- “源码事实”指本树实际实现；“历史已测”指仓内记录的指定实验，不是本轮重跑；“计算”只是按给定参数求界限；“建议目标”须在实验前登记，不能描述为已达性能。

#### 一、先处理硬件可行性，而非先承诺吞吐

Intel 官方规格说明 288V 为移动平台、4P+4 LP-E、8 线程、32GB LPDDR5X 上限，30W 基础/37W 最大睿频功耗，PCIe 配置 Gen5 x4 + Gen4 x4、最多 8 lanes。官方 quick reference 另明确 memory-on-package。来源：
- https://www.intel.com/content/www/us/en/products/sku/240961/intel-core-ultra-9-processor-288v-12m-cache-up-to-5-10-ghz/specifications.html
- https://cdrdv2-public.intel.com/828731/Intel%20Core%20Ultra%20Processors%20Series%202%20-%20LNL%20Quick%20Reference%20Guide%20v2.0.pdf

因此 12 块 SSD 若都要求 x4 独占直连，需 48 lanes，不能由此 CPU 原生提供。可否接上、是否能并发满速是两件事；扩展交换芯片能增加下行设备数，不能创造上行带宽。实际还要计系统盘、NIC 和板载设备使用的端口。需要先取得主板型号/原理图或官方拓扑、背板和 PCIe switch 型号、实际 negotiated speed/width、SSD 与 NIC 完整型号/固件、IOMMU/驱动、散热及电源资料。

- 不能从“E1.S 企业盘”推出每盘代际、功耗、PLP、耐久度或热插拔能力；须按准确型号核对。用户给的是 15.6TB，本报告照此计算，不擅自换成常见的 15.36TB。
- Gen4 x4、Gen5 x4 的编码后单向理论数据量分别约 7.88、15.75 GB/s，尚未减事务/协议开销；这是总线算术，不是本机可用吞吐，更不能把所有共享路径相加。
- 网络规格未知。10/25/100 Gb/s 对应的 1.25/3.125/12.5 GB/s 只是线上位率换算，SMB/TLS/TCP/CPU 后实际更低。应先实测 NIC 与客户机链路上限，再解释存储端结果。
- 8 核不是 8 个相同性能核，短时睿频不是全天持续性能。需记录持续频率、温度、功耗、降频与各核占用；先保留 OS 调度，只有测到明确迁移/争用才比较绑核方案。
- 同交换芯片、背板、电源或机箱内的 12 块盘仍共享故障域；ZXS 多副本不等于异地备份。不能以软件开发目录验证物理独立性，也不能由 SSD 产品宣传推出端到端掉电持久性。

##### 容量和系统盘界限

按用户标称十进制容量：12 × 15.6TB = 187.2TB，约 170.26TiB。假设全部可用于数据且三份副本均匀布置，内容理论上界 62.4TB，约 56.75TiB；再按 10% 余量计算为 56.16TB。它们均未扣格式化、元数据、工作区、历史完整版本、复制/迁移/修复暂存及路由不均衡，不能当可售可写容量。各目录固定目标可能先满，不能按全池空闲相加。

源码和契约始终为 FULL 保存：修改 4KiB 的 10GiB 文件仍会生成 10GiB 完整新版本；三份正文约 30GiB，另有暂存和元数据。容量模型必须至少包含文件数、尺寸分布、每日保存次数、保留版本数、保护份数及变更率，不能仅用当前逻辑文件总量估算。

1TB 系统盘仅约 931.32GiB。HTTP 与 SMB 暂存是独立账本，各默认 100GiB/主体、500GiB/池、1GiB 恢复余量。500+500GiB 总额度大于 1TB 标称盘，但唯一管理员正常已归属数据还受两路各100GiB 限制，不能据此错误声称默认必然写满 1TB。真正问题是两路暂存、核心工作区、管理数据、日志、SQLite/WAL、恢复检查点与系统的总峰值未统一核算，且实际共享挂载点未知。独立 free-space 检查仍存在并可拒绝写入，不得描述为无任何容量保护。

证据：`src/server/src/main.rs:74–104`；`spool_budget.rs:24–48,365–435`；`smb_rpc/budget.rs:370–436`；`docs/19-capacity-and-performance.md:203–232`。初始化核心单文件/工作区各1TiB，服务默认 `max_upload_bytes` 1GiB，100GiB主体暂存限制也不是“支持1TiB文件”的证明。部署档应明确最大文件与所需系统盘峰值，不能只抬高一个参数。

#### 二、当前实现值得保留的边界，以及仍需测量的热点

| 组件与证据 | 已有事实 | 不能外推/下一步 |
|---|---|---|
| `src/server/src/engine_backend.rs:63–78`；`engine_backend/content.rs:14–79,87–103` | EngineBackend 持单个 Mutex；读取计划和当前授权在锁内，正文打开 I/O 已移到锁外 | 不能说所有文件 I/O 都持全局锁。应分别测 lock wait/hold、准备、复核、发布决定与恢复，找出实际长锁路径 |
| `src/engine/src/engine/publication.rs:755–842,1123–1260` | 最多4个 worker 按独立文件分组；同文件版本和路由副本链保序；所有 worker join 后才进入决定 | 不能直接改成12线程或并行所有副本而破坏来源链。需要按共享上行、SSD队列和前台P99比较1/2/4/8候选；调整先保持语义/同步点 |
| `src/smb/src/protocol.rs:53`；`client.rs:22,484–505,607–628` | Read RPC≤64KiB；Client clones 共用连接Mutex，逐块 request/response，解码仍在该锁内 | 新借用解码减少局部中间对象，不是零拷贝或端到端高速证明。较大frame、binary传输或多请求并发属于独立协议研究，必须保留严格JSON/FD/顺序/撤销/超时边界 |
| `src/server/src/smb_rpc/handles.rs:354–414` | 每session仅一个已验证完整校验段cache，miss先驱逐；每次先授权 | 顺序读能复用当前段，跨文件交错会驱逐。先测命中/错失/验证字节和真实client吞吐，再决定是否值得有界多项cache；不可把未验证段标健康 |
| `docs/12-reading-and-cache.md:65–77`；`src/server/src/content_proof_budget.rs` | 新内容默认256KiB校验段，旧格式描述支持64KiB/256KiB/1MiB/4MiB；内容打开manifest有跨打开共享约1GiB编码/解码重叠预算 | 4KiB单段随机miss的256KiB/4MiB分别64×/1024×只是读取字节计算。1TiB文件256KiB段哈希数组约128MiB，不包括头/其他对象。32GB应同时计所有内存owner而非各自预算单独通过 |
| `src/contract/src/shared_records.rs:1–18,157…`；`model.rs:1057–1137`；`src/engine/src/engine.rs:97–134` | 权威State仍驻留；SharedRecords clone共享map，首次共享写复制全部keys/handles O(records)，不复制所有record payload；另有业务投影 | 不能称按页加载了所有历史或整体O(delta)。请求有界≠永久State与准备观察有界。必须实测旧代驻留、COW峰值、事务序列化、恢复与元数据文件数 |
| `docs/19-capacity-and-performance.md:60–120` | 目录响应/候选有界、已发布路径可二分查询；列表仍扫描相关命名空间，部分工作处在Engine锁；历史健康索引目标scope全认证，失败保留安全回退 | 优化响应条数不等于10万实盘文件导入/写入合格。应拆“文件数量”和“单文件历史长度”两个维度，不以合成元数据代替真实持久历史 |
| `docs/19-capacity-and-performance.md:130–144` | 根描述/写集/编码复用已存在；旧依赖省略须真实周期owner的有效巡检覆盖和准确谱系 | 不应重新计划成“从零增加增量元数据”。需分别测有资格hit、无资格、descriptor miss、冷恢复/损坏fallback，不通过伪造巡检owner制造收益 |

保留 full-only、独立恢复、顺序来源链、所有规定 fsync、错误来源、当前授权与校验保证。性能改进应减少重复工作，而不是删保证。

#### 三、可观测性现状与差距

已有能力不是空白：

- `telemetry/mod.rs:20,22–101,168–227,305–333`：5秒采样 CPU全机利用率/逻辑核数/可用功耗、主机内存、各盘容量/读写只读状态/在线/可用功耗及不可用原因；独立线程；HTTP监测有独立2槽，数据后端不可用时仍尽量提供主机诊断。
- `telemetry/history.rs:12–18`：31天历史、最近1小时细采样、分钟聚合；16MiB单cache文件、64KiB单样本、128MiB编码计费上限，不应等同allocator精确RSS硬上限。持久缓存可重建，失败不否决管理服务。
- `telemetry/statistics.rs`：每指标保留有效样本数，缺失传感器不是0，聚合不使用错误分母。
- `lib.rs:1852–1853`：`/api/system/status`、`/api/system/history`；`disk_hardware.rs:23–34,210–231` 展示型号、序列号、设备路径/namespace容量与映射；明确不是物理独立性或存储准入证据。
- `integrity_scrub.rs`、`management/integrity_scrub.rs` 已有阶段、字节进度、基线、新鲜度、可行性诊断、取消与调度生命周期；不要重复列为待从零实现。

已审生产路由及Telemetry DTO未提供：操作/请求阶段P50/P95/P99分布、Engine锁等待/持有分布、并发槽使用及拒绝原因计数、每盘读写bytes/IOPS/队列延时、fsync时延、进程RSS/峰值、协议验证/缓存字节、NVMe磨损/温度/错误/unsafe-shutdown趋势。没有在这些路径发现Prometheus/OpenTelemetry或通用metrics端点；仓内tracing基础设施与warn/error日志不能代替这些运行指标。

建议 P1 指标契约：
1. 按 operation class、protocol、phase、result code、slot-id 这些受控维度统计；不把文件path、version-id、operation-id或token作为聚合label，防高基数与泄露。追踪关联ID可以放受控诊断事件而非无限label。
2. 单调时间测阶段：入口准入、Engine等锁/持锁、准备、COPY/HASH、元数据安装、ACCEPT、PUBLICATION、索引应用、响应完成。用户确认延迟以保护完成并公开为界，不只测接收/入队。
3. 有界直方图/计数器、明确溢出或丢弃计数；采集不可执行额外业务fsync、全池扫描或持长Engine锁。监测自身CPU/RSS/写入预算单列；开/关配对验证其影响。
4. 加每盘吞吐/IOPS/排队/同步、进程RSS/可达旧代数量、spool分类、恢复所需空间、NVMe温度/磨损/介质错误及采样失效状态。需要权限的硬件采集不得静默扩大daemon权限，先以允许的只读接口/外部采集验证。
5. 状态保持“未测/不可用/已过期”，故障和缓存miss都不能静默变0。诊断缓存丢失不应改变业务决定/恢复结论。
6. 对外导出采用何种协议需单独决定。首先把已有Web和测试runner消费的类型化内部指标做完整，不为有一个图表先引入无限数据依赖。

#### 四、P0 巡检规模与新鲜度的矛盾

`src/server/src/main.rs:40–49`：每次尝试完成后间隔86400秒，最大报告年龄172800秒，限速67108864 B/s（64MiB/s）。年龄从扫描开始而非结束算。`src/server/src/integrity_scrub.rs` 已提供 `PayloadCannotFitMaxAge`，用于用正文下界证伪可行性，符合时也只返回 UnknownTotal 而非承诺可行。

若需扫描的实际已占正文副本共187.2TB，则64MiB/s纯正文时间下限32.29天；90%占用下限29.06天。48小时最多11.596TB，尚未计元数据、切换、散热和前台竞争。若要求187.2TB两天覆盖，仅正文平均速度至少约1.083GB/s（1.009GiB/s），不是软件已实现/硬件已达到值。设备额定容量也不是当前扫描量，必须以实际可达副本数与字节为准。

更严格的新鲜度要求要考虑“扫描完成后再等24小时”和从开始计年龄，不能只检查一次扫描<48h就保证连续不超龄。

建议：先按实际占用显示预计最低耗时、全覆盖完成时间和缺口；明确健康声明与逐对象覆盖定义；测前台P99与scrub带宽/CPU的曲线；据此选速率、周期与可接受最大年龄。若要分片巡检或增量覆盖，须先设计准确信任根、变更失效、恢复与省略资格，不把多次不同根的局部成功拼成全池当前健康；不能单纯延长max_age直到绿灯。失败/取消/过期保持原安全回退。

#### 五、历史已测证据的可用范围

1. `docs/validation/smb-read-decoding.md`（2026-11-03）：优化后64KiB纯codec请求分配字节 11,601,723→133,127，账面峰值4,195,779→65,536，5轮无计数allocator的单次均值中位数4.645ms→2.032ms。空payload反而约673→931ns。此为同工具链opt-level3的独立codec，非socket/Samba/磁盘/目标机/RSS测试；不要推出SMB吞吐提升56.2%。本轮未重跑原probe。
2. `docs/validation/history-growth-benchmark.md:121–158`：固定commit `d3eefd…` 的5000收据/事件增长实验两轮build574.68/587.98秒，reopen1094.16/1197.01秒，recovery1094.04/1193.60秒；完整oracle六阶段通过，最高观察RSS约69.24MB。8192/10000/100000因预登记预算预测超限未运行。该5000不是5000个真实新FULL版本，也不是当前tree或288V表现。
3. 同文159行以后独立`full-versions` workload才每次发布不同4KiB真实FULL，并核旧版本/原operation结果/事件。新计划必须用这个口径做历史增长，不以绑定次数替代版本数。
4. `docs/validation/persistent-state-r48-formal-results.md`：100正式进程及40独立准入进程的精确R47/R48配对中，AVL候选未建立稳定端到端写收益，多读取路径回退，既有结论是不采用。不得重新宣称“改持久AVL必然解决32GB”，也不能外推未测100k规模。
5. `docs/tools/perf-check.md`：1KiB同文件三目录模拟副本、fsync、SQLite typed-history触发器行数、重开/范围验证与独立正确性检查有价值，但非三块物理盘、非HTTP范围吞吐、不是总物理写放大，时延不作为机器无关门禁。

以上是仓内历史记录，不是本次独立重验raw或当前最终commit全仓CI证据。

#### 六、分层基准计划与采用门

##### 0. 冻结输入与安全边界

记录准确commit/tree、dirty diff、binary SHA、toolchain/lock/profile/features、OS/kernel/文件系统/mount选项、firmware/BIOS、CPU电源设置、内存、完整PCIe拓扑、NIC/客户机、每盘型号与实际容量、剩余空间、SSD寿命/温度、保护份数、checksum段、pool配置与数据集seed。只使用预先指定的非生产测试pool/空测试卷；裸盘写入、热拔插和掉电测试需要独立批准和恢复预案，本规划不执行。

##### 1. 物理平台基线

- 单盘、同switch组、跨上行组、12盘并发，顺序read/write与4KiB random，QD1/8/32和1/2/4/8个并行负载；报告CPU与NIC是否先饱和。不得用空盘短缓存burst代替持续稳态。
- 正确记录数据是否可压缩、direct/buffered I/O、写入持久边界、预处理程度、温度和可用空间。写入量和SSD耐久消耗先计预算。
- 单独测1TB系统盘受到spool/WAL/log压力时的同步尾延迟；不要让全部benchmark证据也写在同一待测设备造成未记录干扰。
- 网络基线与客户端瓶颈分离；如果链路先满，不能据此判断12盘或Engine性能已满。

##### 2. 产品操作矩阵

- 同步保存：4KiB/64KiB/1MiB小文件批量；256MiB/1GiB大文件；更大文件需先配额/存储预检。改动比例4KiB、1%、100%；保护份数1/2/3按实际支持配置；逐次flush、close、批量提交分别测。测从请求/屏障开始至最终保护公开及客户机返回，不混入仅暂存完成。
- 读：首次open、重复open、顺序全文件、4KiB/64KiB随机、校验段跨界、同session多文件交错、不同session并发，客户端冷/暖和服务端冷/暖分别定义。包括主副本坏块/不可用后的验证回退，不用错误数据换速度。
- namespace/history：文件数1k/10k/100k阶梯、单目录宽/多层深、单文件真实FULL历史16/32/64/256/1024阶梯。先通过资源预测与oracle预算再扩大，不能预先承诺100k完成。目录翻页、检索、历史窗口、首/尾页、字段片段、branch/rename/merge/split、操作重试要分别测。
- 恢复：checkpoint hit、miss、corrupt fallback、SQLite不可用/重建、正常重启、明确故障恢复。oracle完整检查已确认operation/版本/事件和每份正文；一次已经生成checkpoint的恢复不能叫独立冷恢复。
- 干扰：前台load单独与scrub/修复/分支首次写/拆分并存对照；HTTP和SMB混合、慢客户端、断连留存、异常帧和配额边界。检查正常数据负载满时监测/停止入口/必要恢复仍可用。

##### 3. 测量规则

- 绝对值与差值都报：吞吐、P50/P95/P99、样本数/分布/离散、CPU分核、RSS/PSS/VmHWM、活跃State代、metadata对象与实际分配bytes、read/write/syscall/fsync、网络bytes、device queues、锁等待/持有与各phase。
- tracing/syscall计数、allocation probe与无追踪计时分开；采样RSS不是精确allocation峰值；/proc rchar不是物理盘I/O；表观文件长不是实际分配；OS缓存热与ZXS cache热不能混称。
- 候选采用以相同硬件/数据集/合同的配对试验，建议至少5对独立进程并交替AB/BA，公开全部样本。P99必须有足够请求量并报告统计不确定性，进程内循环不当独立重复。预设预算不足即incomplete/not_executed，不能运行后放宽预算。
- 既有工具的授权/最大规模约束继续适用；不要为规划主动发昂贵超大workflow。先做低风险calibration和量级外推，再决定下一档。

##### 4. 验收和优先级

P0-A 硬件/容量部署档：确认12盘与系统盘/NIC实际拓扑、共享故障域、热稳态与每盘身份映射；确认可支持的最大文件与同时暂存峰值；不满足不承诺聚合容量/速度。

P0-B 巡检可行性：用实际副本bytes算下界，按完整周期和占用增长检验新鲜度；可行性未知/无法达标有明确状态。仍须完整正确性与失败回退，不能以关闭校验或伪覆盖通过。

P0-C 证据门：给每项结果标准确commit、环境、真实/模拟/公式/未执行；把“5000操作”与“5000真实FULL版本”分开，保留已否决AVL结论和所有失败样本。

P1-A 可观测性先行：完成上述有界phase/lock/IO/资源指标。验收是故意制造慢盘/饱和/认证过期/资源耗尽时能区分原因、无敏感高基数、故障不影响权威操作，采集开关配对影响在预先登记预算内。

P1-B 32GB与系统盘统一容量模型：按真实owner建立进程内存/共同文件系统空间预算图；优先配置可执行的保守profile，再决定是否需要跨HTTP/SMB/core统一准入。无swap依赖、无OOM、必要恢复余量保留、断连/重启预算重建、并发超限只拒新增长且保留旧材料，是硬门。可先登记“守护进程峰值≤8GiB、整机峰值≤24GiB并保留≥8GiB给OS/cache”为讨论起点，但它们是待校准建议，不是已测能力或现有代码限额。

P1-C 端到端SMB读与发布阶段：先定位序列化、同步往返、段cache、复制hash与全局决定中的实际占比，选择一个可归因热点做窄改动。以正确性/错误优先级/资源边界不变为先；优化前登记收益/回退线，不能仅纯codec赢就采用端到端宣称。

P2-A 历史与目录扩展：根认证查询材料、冷/热分离、按scope加载、元数据长期增长与恢复峰值单独立项，保留安全回退。测试旧根/真实split/当前授权/完整oracle，既不能静默截页，也不能恢复已否决容器只为理论渐近复杂度。

P2-B 背景自适应：在已有单worker限速/取消/进度基础上增加受控按盘/共享上行公平预算或优先级；以真实前台P99、队列和温度调节，不能自动改变保护份数或使未决决定超时回滚。

P3 硬件特化：仅在以上证据证明有收益后研究更大帧/二进制payload、受控pipeline、异步IO/预读、checksum段策略及线程/绑核。每项独立协议/恢复/安全门，不能把“12盘NVMe”当必须使用复杂路径的充分理由。

所有数值SLO应在测前由项目为目标使用场景登记，至少覆盖保存P95/P99、首字节、目录/历史页、恢复时间和后台影响。规划可推荐基线对照回退门，不能在尚无真实硬件基线时虚构“达到10GB/s”“百万IOPS”“毫秒级所有保存”。


---

## 11. 专项审查与工作包

### ZXS 1.0.0 发行质量、许可及操作审阅

审阅时间：2026-11-03 14:46 UTC。只读审阅，未修改仓库、触发工作流、创建标签、公开 Release、安装服务或部署。

#### 结论

**现有流水线已经能生成、校验并自动公开一套 Debian 13 amd64 产品，但当前基线不能直接作为已验收的 1.0.0 正式版发布。**

当前可以确认的是：精确 main 提交的 17 个 CI jobs 全部成功，而且已实际下载其安装候选，ZIP 摘要、七文件集合和源码身份通过本次独立复核。仍需完成产品版本变更、同一最终提交的完整 Release 演练、发行许可资料决定，以及所承诺环境的实际验收。自动化成功不能代替这些条件。

三个必须保持分开的结论：

1. **可构建、可组装安装候选：已具备当前精确提交的证据。**
2. **公开分发许可资料：仍有明确待人工决定项，尤其 erased@0.1.2。**
3. **1.0.0 稳定/生产支持承诺：不能从 CI 绿色推导，必须明确范围并取得对应证据。**

#### 1. 固定基线与远端状态

- 本地审阅目录：`审查检出目录`，HEAD `a50502f5b5073dd90ff88a1f3fcb39e08f9f24cf`，tree `927bbc7f7354b2cd6f4b84fdd84c9686692174d2`。
- GitHub main：[`72e08db452804f89e91e3866922ee11dc9cb8c77`](https://github.com/isarmg/zxs/commit/72e08db452804f89e91e3866922ee11dc9cb8c77)，tree 与本地完全相同。main 是审阅时的远端最新提交。
- [当前 CI 41641687665](https://github.com/isarmg/zxs/actions/runs/41641687665)：push main，2026-11-03 13:08:28 UTC 创建，13:34:38 UTC 完成，结论 success。
- 通过 GitHub API 查询，`releases?per_page=100` 返回空数组，`git/matching-refs/tags/` 返回空数组：审阅时无现存 Release、无远端 tag。
- 当前 main 的所有 runs 仅有上述普通 CI，**没有该提交的 Release 工作流演练**。
- 已完整读取 163 次 `workflow_dispatch` 运行，发现一次旧 Release 成功：[39941503452](https://github.com/isarmg/zxs/actions/runs/39941503452)，2026-10-23、提交 `22950c2af4be586e9d68ba2ba398a22138b9f7b7`。旧成功不能转算当前源码/当前发行资产的验收。
- main API 返回 `protected: false`、无 required status checks；repository rulesets API 返回空数组。这里只记录当前事实，未修改设置。现有发布阻断主要由工作流依赖关系承担，并非分支保护或环境审批。

远端事实查询来源：

- https://api.github.com/repos/isarmg/zxs/branches/main
- https://api.github.com/repos/isarmg/zxs/actions/runs?head_sha=72e08db452804f89e91e3866922ee11dc9cb8c77&per_page=100
- https://api.github.com/repos/isarmg/zxs/releases?per_page=100
- https://api.github.com/repos/isarmg/zxs/git/matching-refs/tags/

#### 2. 当前测试、CI 与证据边界

当前 CI 的 17 个 jobs 均成功：静态检查、13 个验证分片、Production WASM、Debian 13 Samba SDK/实际 SMB、单 ELF Debian 服务端与宿主 Chrome 验收。

13 个分片为：`rust-sources`、`quality`、`security`、`workspace`、`foundation-platform`、`foundation-crypto`、`foundation-data`、`foundation-language`、`foundation-async`、`web`、`contracts`、`build-tools`、`smb-protocol`。

关键可用证据：

- [精确 Samba SDK 与真实 SMB job](https://github.com/isarmg/zxs/actions/runs/41641687665/job/125938333544)：构建匹配 SDK/VFS、最小运行镜像、apt smbd 加载模块及真实客户端验收步骤均成功。
- [交付单 ELF 与浏览器 job](https://github.com/isarmg/zxs/actions/runs/41641687665/job/125941957658)：构建和解包验证、Debian 13 单程序运行证据、安装候选上传、模板浏览器验证及截图步骤均成功。
- `.github/workflows/ci.yml:35-50` 的普通 CI 会选择有证据的分片；本次实际 jobs 列表显示全 13 个分片均执行，不能仅凭工作流顶层 success 假定每次都全跑。
- `.github/workflows/release.yml:53-105` 使用固定 13 分片，`assemble` 在 `:201-221` 等待全部 verify、linux-web 和 smb-sdk。没有将普通 CI 的旧结果直接转为发布证据。

质量门禁的实际含义：

- 格式、严格 Clippy、Cargo JSON warning/error 检查覆盖第一方和被实际编译的本地依赖，不等于所有模块的语义、安全或 unsafe 审计。见 `docs/quality.md:5-19,27-45`。
- Foundation 是登记的限定消费者/后端/回归矩阵，不是每个上游项目全部测试。见 `docs/ci.md:14`、`docs/quality.md:13-19`。
- 安全扫描覆盖固定 scanner 锁、产品锁、活动和已合并实现的上游身份；当前公告与 yank 状态实时查询，扫描不完整失败。但没有发现已知漏洞不等于不存在未知漏洞，也不关闭许可缺口。见 `docs/quality.md:21-25`。
- 当前 Samba gate 包含真正的系统 smbd 与 SMB3 客户端，不应误说成“只编译了 SDK”。但它仍是隔离 Linux 客户端场景，不是 Windows/macOS 等完整客户端矩阵。见 `docs/validation/debian13-system-samba.md:13-31`。
- Chrome 是宿主沙箱浏览器，服务端 ELF 在 Debian 13 容器；不能声称容器内 Chromium 也通过，或所有浏览器已通过。见 `docs/release.md:15`。
- `docs/22-acceptance.md` 是验收目标；其 `:236,286` 明确禁止整体记为已通过。

#### 3. 已独立核验的真实安装候选

[安装候选 artifact 13234552842](https://github.com/isarmg/zxs/actions/runs/41641687665/artifacts/13234552842)：

- 名称 `installation-candidate-linux-web`。
- ZIP 93,670,910 bytes，SHA-256 `9a529b215794850fd4eb87678429170e0e60bbfa87b0cb81155ca7ac186a4efe`。
- API 记录 2026-11-10 13:32:43 UTC 过期，不宜把临时 artifact 当长期发行地址。
- 本次已下载 ZIP、计算相同摘要、检查恰好七个顶层文件，再用当前源码的 `verify-release-assets.py` 校验实际字节。

| 文件 | 字节数 | SHA-256 |
| --- | ---: | --- |
| `zxs-0.1.1-debian13-amd64` | 58,833,088 | `fe54294e564808375f473a6a8c509fd33fab5c877d915ea5d554d3918155bff6` |
| `zxs-vfs-0.1.1-debian13-amd64.so` | 1,395,312 | `6f4ac090b5f031e8496a9ce7ab94e2183b0718ab5645e0b9f21c2a9cf03c8ca0` |
| `zxs-materials-0.1.1.tar.gz` | 67,316,089 | `2addd40694380558ee171ee3dc6ae680e75f34dab7abaef781d907dfe98be5db` |
| `BUILD.txt` | 4,609 | `b58c7a4579eb2eeffa80b832d3168383f2b0dfc75d870cf7ebe66116a5660724` |
| `INSTALL.md` | 7,912 | `f32f4c69c0a92a3cb172973d4c1cd04e21aaefd6e05b661d00a5eb184ef454fc` |
| `VFS-BUILD.json` | 977 | `fc5bfb5ef0be0e5a658c47bdeea4ef1970e41019092ed19f49fe677713dd9fc9` |
| `SHA256SUMS` | 516 | 清单覆盖其余六项；ZIP 摘要另记 |

执行命令和结果：

```sh
python3 .github/scripts/verify-release-assets.py \
  --directory <验收资产目录> \
  --version 0.1.1 \
  --commit 72e08db452804f89e91e3866922ee11dc9cb8c77
# Verified 7 release assets and their source identity.
```

收据实际值：

- 产品版本 0.1.1；native/Web commit 均为 `72e08db...`，Web/VFS tree 均为 `927bbc7...`。
- `native-web-embedded=true`，target `x86_64-unknown-linux-gnu`，release profile，Rust 1.99.0。
- 运行库为 Debian 13 的 glibc、libgcc 等，不是静态链接或任意 Linux 通用 ELF。
- 默认 Rust CPU 查询为 `x86-64`，但收据明确写 `native-isa-attestation=not-attested`，不证明所有 C/汇编对象最低 ISA。
- VFS 针对 `2:4.22.11+dfsg-0+deb13u1`，VFS interface 50；模块 SHA 与收据一致。
- `VFS-BUILD.json` 中 `runtime_acceptance=not-run-by-build-script` 仅说明构建脚本本身不执行运行验收，**不表示本次 CI 没有运行验收**；后续实际 Samba job 已成功。

本次只校验文件和收据，没有执行产品二进制或部署。候选不能改文件名/改收据冒充 1.0.0，必须从真正 1.0.0 的最终提交重建。

#### 4. 许可与 erased 的最新权威复核

##### 4.1 仓内许可边界

- 第一方默认 Apache-2.0：`Cargo.toml:46-50`、`compliance/README.md:5-13`。
- 第三方许可、版权、SPDX 和作者头继续适用；集中归档、重命名或合并不改变原始权利。
- 发布资料包含 `compliance/`、`docs/`、README、deploy 和可选 rescue。见 `.github/scripts/release-package.sh:59-69`。
- `convert_case_extras@0.2.0`、`drain_filter_polyfill@0.1.3` 已退役，不能报为当前二进制活动依赖；但历史资料缺口仍保留于所分发材料。见 `compliance/README.md:30-42`。

##### 4.2 erased 仍是活动依赖

依赖链：`web-app → web-interface → view-renderer → erased`。`src/foundation/web-runtime/view-renderer/Cargo.toml:45-48` 是真实依赖边，`src/foundation/web-runtime/view-renderer/src/erased.rs:1-24` 实际使用 `ErasedBox`，不是仅保留的历史来源。

本次直接查询上游，取得以下证据：

1. [上游固定提交完整递归文件树](https://api.github.com/repos/JonathanBrouwer/erased/git/trees/60eebad28ab2578a6322199099b8a07035438008?recursive=1) 返回 `truncated=false`，只有 workflow、README、Cargo.toml、gitignore 和四个 Rust 源文件，没有独立 LICENSE/COPYING/NOTICE。
2. [固定提交 Cargo.toml](https://github.com/JonathanBrouwer/erased/blob/60eebad28ab2578a6322199099b8a07035438008/Cargo.toml) 声明版本 0.1.2、MIT 和两名作者；这提供许可声明和作者来源，不提供独立版权通知/年份文本。
3. [上游提交历史](https://api.github.com/repos/JonathanBrouwer/erased/commits?per_page=20) 只有 15 个提交，最新仍是 `60eebad...`，2024-08-16；不是已有新 LICENSE 而本仓尚未同步。
4. [Docs.rs 0.1.2 原始包文件列表](https://docs.rs/crate/erased/0.1.2/source/) 同样未列独立许可文件；当前上游 issues API 返回空数组。
5. 仓内官方 crate 来源收据记录 SHA-256 `a1731451909bde27714eacba19c2566362a7f35224f52b153d3f42cf60f72472`，VCS 身份与上述固定提交一致。本次未重新下载 crates.io 原始包；官方站点页面/API经 web 工具不可访问，因此没有把该网络失败写成成功验证。

**未找到足以自动补齐仓内所述许可资料缺口的新权威文本。** 这不是断言“没有任何许可”或作出违法分发的法律结论；MIT 元数据确实存在。结论仅是仓内要求的独立复核项目前不能由技术门禁自动关闭。

可选关闭路径：

- 取得上游明确适用于 0.1.2 的许可与版权通知，保存原文、权威地址、版本适用范围和哈希，再追加中央材料；不能从作者字段自行编造版权年份。
- 由发行负责人或其许可审查人员就现有 MIT 声明与分发材料作独立决定并记录依据。助手不能冒充已经完成法律审查。
- 若选择替换，则另立设计/实现/unsafe 与内存安全回归/浏览器回归/显式退役任务，保留原来源；不能为了清单“全绿”删除记录。

联系上游属于对外沟通，本次没有发送 issue 或消息。

#### 5. 升级 1.0.0 所需的准确变更范围

最低版本相关改动：

1. `Cargo.toml:47`：workspace version `0.1.1 → 1.0.0`。
2. `Cargo.lock`：12 个第一方产品包版本同步更新：authentication、storage-contract、name-policy、platform-io、storage-engine、storage-server、storage-client、storage-rescue、web-app、web-interface、smb-protocol、smb-vfs。检查最终 diff，不能全仓替换所有 0.1.1。
3. `test/fixtures/command-line-cli.json:27,102`：实际 `zxs --version`、`zxs-rescue --version` 的两个固定快照更新到 1.0.0；先跑实际 CLI 再确认快照。入口见 `.github/scripts/check-command-line-family.py:331-364`。
4. `compliance/rust-sources.json` 固定根 manifest 和 lock 哈希；需通过现有 `tools/rust-sources/manage.py refresh` 显式刷新相关登记、审查生成 diff，再 `verify --metadata`。生成关联文档按工具实际结果审查，不能手改历史来源收据去掩盖变化。
5. 发行说明/验收记录写明首发范围、已测/未测项、许可决定和最终 SHA。若修正下述文档歧义，必须纳入最终候选再验收。

无需机械改变：

- 12 个产品 manifest 目前全为 `version.workspace=true`，无需逐个硬编码 1.0.0。
- `tools/release-tools`、`web-build`、`browser-smoke`、`contract-check`、`perf-check`、`unicode-build` 保留自己的 0.1.0 版本；`web-bindings` 保留上游 schema 对应 0.2.129。Foundation/宏的真实来源版本也不改。
- `test/ci/test_native_consumer_features.py` 和 `test/ci/test_tracing_family.py` 中 0.1.1 是合成 graph 样例，不是发布版本源。是否改成更清晰样例可以审阅，但不是必须全替换的版本耦合。

`tools/release-tools/src/main.rs:19-65` 会要求合法规范 SemVer 与 root 及上述 12 个产品一致。**当前直接推 v1.0.0 标签会在版本 job 失败。**

#### 6. 现有发布机制与最小可行操作顺序

##### 6.1 工作流机制

- `.github/workflows/release.yml:3-6`：push `v*` tag 才走正式发布；手动 `workflow_dispatch` 是只构建演练。
- `:201-221`：assemble 等待 version、全部 verify、linux-web 和 smb-sdk，然后校验并上传 `release-complete`。
- `:223-244`：只有 tag push 的 publish job 持有 `contents:write`，不需要另跑手工上传脚本。
- `.github/scripts/release-publish.sh:9-66`：先校验并做私有上传快照；确认远端 tag 已存在且指向所构建 SHA；创建带源码 marker 的草稿；上传；下载并验证严格集合、摘要和身份；再次检查 tag；最后公开。
- 已公开版本拒绝覆盖，只续传同一源码的本流程草稿。网络/权限/服务器错误不能伪装 404 新建。假 gh 的离线发布故障回归不向 GitHub 写入，入口 `test/ci/release-publish.sh`。
- 当前脚本没有在上传前读取人工许可决定或物理验收签字的技术门禁。因此**不能先推 tag，再等待人决定是否允许公开**；tag push 可能自动完成公开。

##### 6.2 最小可行步骤

下列是操作计划，本次没有执行。

1. 明确 1.0.0 支持范围并关闭/记录发行许可决定，修正安装文档歧义；取得所承诺实际环境的验收证据。不能把未测项目改为“通过”。
2. 在干净候选分支上完成第 5 节版本、锁、CLI 和来源登记变更；不得顺带全量升级第三方依赖或更换存储格式。
3. 有 Rust 1.99.0 与所需目标/SDK 的环境执行至少：release-tools tests、`release-tools -- v1.0.0`、实际 CLI 快照、来源 verify、static；随后以最终提交运行全部适用门禁。发布工作流本身仍会全跑。
4. 推送候选，记录最终 SHA。触发现有 Release 的 `workflow_dispatch`，例如经授权使用 `gh workflow run release.yml --repo isarmg/zxs --ref <候选分支>`。触发后必须读取新 run 的 `head_sha`，不能仅按最近运行或分支名猜测。
5. 等待**该次**演练全部结束、assemble 成功，下载 `release-complete`，独立复核版本、提交/树、七资产摘要、BUILD/VFS 收据和实际资产验收。保留完整 run 链接及摘要。
6. 发行负责人确认范围、未测项和许可决定后，才在经过验收的精确提交上创建并推送 `v1.0.0`（例如 annotated tag；签名 tag 另需已有可用身份）。不能使用会自动指向“当前最新 main”的含混方式，也不能 force 移动标签。
7. 观察 tag 对应 Release run 到终态。成功后再读取公开 Release、tag commit 与全部资产，下载复核，不把 push 成功或 upload 成功当成发布成功。
8. 失败时保留诊断。若已有本流程草稿可按原 SHA 重试；如果源码必须变，先重新决定版本/标签策略，不能覆盖已公开资产或把旧产物标成新源码。

真实部署、配置系统账户/Samba 凭据、修改权限/防火墙、操作数据盘是另一类动作，不是 GitHub Release 发布的自动附带步骤。本次授权范围只读，以上动作均未执行。

#### 7. 仍需明确的实际验收与产品限制

`docs/validation/release-checklist.md:33-56` 要求对一次性测试环境记录：

- 干净 Debian 13 amd64 安装七文件，真实服务账户、模板路径、停止/启动、Samba MODULESDIR 和 socket 配置。
- 首次初始化、登录/退出/改密、磁盘登记、拒绝匿名/guest/旧会话。
- 真实系统 SMB 客户端创建、读写、改名、列举、并发打开、断线重连以及授权/版本一致性。
- 数据盘不可用和恢复、损坏正文/日志/索引、离线诊断与不覆盖导出。
- 满盘、元数据限额、强制终止恢复，独立记录物理断电/设备缓存/文件系统条件。
- 实际大目录、大文件、持续运行资源和容量范围，不填写未经测量的性能承诺。

明确不支持/不应含混的边界：

- 发行基线是 Debian 13 amd64 GNU ABI，不是所有 Linux、其他 ABI/架构或原生桌面/移动 App。
- Web 用于管理与文件元数据，不提供正文上传、下载或预览；正文主要通过 SMB，受控 HTTP API 仍存在。
- 系统 Samba 由用户管理，预编译 VFS 只适配精确包版本；安全升级 Samba 后须重建/重验匹配 VFS，不应长期冻结有漏洞版本。
- 无自动旧存储格式迁移、兼容回退或任意降级承诺。
- 目录分支合并尚未实现；部分含目录组合的历史树拆分拒绝。配置草稿不是在线拓扑编辑/换盘能力。
- 单系统盘+单数据盘可部署，但单数据盘永久损坏不承诺正文可恢复，多目录也不等于独立故障域。
- 长期容量、物理断电、全浏览器/SMB 客户端组合仍不能视为已验收。

来源：`README.md:5,9,18-20`、`docs/23-delivery-and-decisions.md:23-30,54-61`、`docs/release.md:29-31`。

#### 8. 发现的文档歧义与建议优先级

1. **发行前必须澄清：** `docs/23-delivery-and-decisions.md:30` 仍说“发行归档不含预编译 VFS”，前半句也暗示要自行编译。实际 Release 提供独立预编译 `.so`，只有资料 tar.gz 不包含它。建议改为“发行提供与指定 Debian Samba 精确匹配的独立预编译 VFS；不放在资料归档内，非匹配宿主须重建并重验”，与 `docs/release.md:24`、`docs/ci.md:23` 和安装模板一致。
2. **发行前保留显著限制：** 许可待决与目标环境待验收不能被自动生成的简短 release notes 淹没；当前 notes 主要概述自动检查。完整验收记录应可从 Release 直接找到。
3. **后续加固建议：** 可考虑将“发行决定/支持范围/许可项已审阅”的可审计材料纳入 tag 发布门禁；新增保护或权限配置需另行按权限规则授权。不要用单个无依据的布尔字段冒充法律与生产验收。
4. **长期质量工作：** 按明确负载补持续容量/真实客户端/物理故障证据，并建立受支持矩阵。此工作不可用现有测试数量或总覆盖率替代。

#### 9. 当前审阅环境限制与未执行项

- 当前 shell 中 `cargo`、`rustc`、`rustup` 均不可用，常见安装路径也不存在；仓库要求 Rust 1.99.0。没有声称在此重新运行 Rust tests、Clippy 或 native build。
- 没有可见 Docker/Podman 可执行文件及 Docker socket；有 Chromium 但没有 ChromeDriver，不能在此复现现有 Debian 容器+沙箱 Chrome 全链路。没有绕过沙箱或关闭浏览器安全策略。
- 审阅时 `/tmp` 是约 4.9 GiB tmpfs；下载前仅约 935 MiB 余量，下载/解压后约 724 MiB。即便补工具链，也不满足这里某些 HTTP spool 测试的默认 1 GiB 恢复余量，更不用说完整冷构建。不能降低生产额度去“修好”环境失败。见 `docs/ci.md:58-64`。
- 没有独立测试物理盘、Windows/macOS SMB 客户端、长期运行环境，也没有执行破坏性硬件实验。
- 本次可用 GitHub connector 完成远端只读查询和 artifact 下载。Release 工作流按文件名查询的特定 endpoint 被工具判定 URL 不支持，已改用它允许的 actions/runs 读取并完整分页，未绕过账户/权限限制。未尝试任何 mutation 或权限扩大。

本次实际完成：静态代码/文档/工作流审阅；当前远端与全部 CI job/资产元数据查询；远端 tag/Release 状态；手动运行历史分页；erased 上游当前和固定提交权威资料核对；真实安装候选 ZIP 和七资产的字节级校验。

本次未执行：代码改动、版本升级、Rust 本地测试/构建、Release 演练触发、tag/Release 发布、第三方联系、许可人工结论、部署与物理验收。

本次下载的临时核验材料位于 `<验收临时目录>/`，只供本任务内复核，不是用户下载入口，也不是正式发行目录。
