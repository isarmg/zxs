# 普通文件版本容器

本模块把正文与不可变证明分开，正文使用可直接读取的普通文件，容器重命名使用真实目录操作。

```text
histories/<history-id>/
├── versions/<version-id>/       # 版本证明、校验和及发布证据
└── file-containers/<file-id>/
    └── namespace/<实际归属目录>/<当前文件名>/
        ├── container.json      # 从已发布 State 重建的名字/备注/选择缓存
        └── versions/
            └── v000001_20261018T133000Z_ab12cd34.txt
```

版本名由文件内顺序、UTC 时间、内容散列前八位和原扩展名组成；时间来自 Rust `time` 包。文件版本顺序独立于原有全局审计序列。未激活目录共享布局时，`State.file_locations` 保存每文件锚点内的归属路径，`file_names` 保存该投影名称。激活后的主盘实际位置由稳定 main 工作树及集中定位记录决定，两个 State 字段仍保存业务归属，不充当物理路径定位器。分支条目的路径可以与其不同；按稳定文件、版本身份读取材料。当前版本证明必须携带 `storage_name` 和 RFC3339 `created_at`，缺失或 null 在解码时拒绝。首次安装暂存的直接命名容器仍按稳定身份发现，决定后的当前布局投影把它移动到命名空间位置；这是现行发布与崩溃重试流程，不是旧版迁移。

- `version_identity`：验证文件标识、相对路径和内容散列，创建唯一顺序名称与 RFC3339 时间。
- `content_path`：按稳定文件标识查找唯一私有命名目录；正文缺失时返回预期路径供既有修复流程识别，多个目录或符号链接明确失败。当前准备过程和离线历史包使用 `versions/<version-id>/content/<creation_name>` 保存待安装正文；读取保留这条现行路径。
- `install_version_content`：消耗 `PreparedContent`。新复制路径持有 `VerifiedCopy`，其中的输出文件已在复制时核对长度、完整散列与校验清单承诺，并完成只读模式和文件同步；安装沿用这次验证，不再全量读回。现有磁盘正文和重试目标仍核对实际长度、完整散列并同步。使用已有安全目录封装及文件描述符相对 `renameat2(NOREPLACE)`，分别同步源和目标父目录；不创建硬链接、reflink 或第二份持久正文。
- `capture_names`：只为缺少记录的文件初始化名字和位置；切换分支和读取旧路径不会改动实体归属。实际移动、重命名由对应业务操作明确更新位置和名字；删除保留最后位置。
- `synchronize_names`：仅为名字、位置、备注、选择、继承引用、版本、放置或内容状态变化的文件更新容器缓存及真实改名。跨目录移动真实迁移同一个容器及所有版本，不复制正文。位置投影在决定后执行并幂等重放；中途失败后仍按稳定身份找到唯一实体，不新建分支占位文件。

受影响文件分类复用现有事务写集，精确键仍逐项比较，失去 lineage 时扫描前后权威记录；范围、保留成本与 source-only 验证状态见[容器名称写集](../../../architecture/container-name-write-sets.md)。

命名空间与容器叶子分别使用绑定稳定文件 ID 的标记，避免用户命名为 `namespace`、`versions` 等普通名称时误判。发现重复实体、符号链接、非私有目录或异常文件会明确报错。父子路径互换先迁到同一个稳定身份根下的临时名称，随后落到新位置；只移除预先完整核对为空的结构目录。重放不改变版本 inode，也不留下第二份正文。

分支创建仅在 Branch.pending_file_versions / Commit.pending_file_versions 登记冻结父版本，不创建 FileVersion、FULL 校验、保护回执或空文件。读取继承父内容，首个实际写入建立独立 FULL 并清除该文件的待写入归属；零字节或同摘要首写也算真实版本。目录组合与不可变子树保存在发布元数据，不在容器内复制正文。

container.json 的 inherited_branches 记录来源、可见 version_id、空 own_version_id 和继承状态，path_contexts 记录当前各逻辑路径独立的写入目标与继承意图。文件有多个别名时不能用单个文件级选择推断所有路径；容器缓存只用 path_contexts 表达精确选择，不再输出旧 selected_branch_id 摘要。所有缓存都不替代发布快照、历史清单或版本签名证明。恢复只重建缓存，不重建空 stub。缺失正文和元数据继承不会创建空容器。

文件改名新增不可变 FilePathNode，记录旧、新路径并引用同一个真实版本；文件夹改名为受影响的每个文件分别登记节点。path_nodes 缓存这些节点及备注，版本记录的 parent_node_ids 同时覆盖后续正文写入与改名节点之间的父子关系。节点本身不新增正文、版本证明或 FULL 回执。历史包保存节点及混合父子关系，验证同文件归属、冻结正文对应关系和无环性；正文清除后保留节点及 PURGED 版本记录，避免元数据历史永久钉住旧正文。

不再读取无命名版本字段的旧正文，不迁移 legacy 正文，也不清理 pending-branches。检测到 `legacy` 或 `pending-branches` 旧容器结构时返回 `FORMAT_UNSUPPORTED`，原字节、空占位文件和目录均保留。新分支创建、恢复、改名和历史包导入都不生成 stub。目录组合/子树或文件路径节点存在时旧拆分接口明确拒绝，避免漏掉映射。

实现直接复用本项目的 `disk::{private_dir, existing_private_dir, open_regular, write_json, sync_dir}` 与 `platform-io` 安全文件系统原语；没有复制兄弟仓库的存储协议或脚本。依赖 `rustix` 和 `time` 的真实包名保留。

`held_directory` 将 `existing_private_dir` 已准入的目录直接交给
[`OpenAt2Root::from_private_directory`](../../platform.md#admitted-directory-handoff)，
保留 `SameMount`，不再沿相同绝对路径做第二次逐组件打开。静态减少一次从 `/`
开始的目录路径遍历及相应打开／关闭，实际 syscall 数和耗时尚未计量；原有准入、
openat2 探测、文件单链接检查、源／目标同步和写锁生命周期均不变。

<a id="name-capture-lineage"></a>

## 命名捕获的发布谱系与增量边界

捕获不变量是：每个 repository 当前 main HEAD 的文件都有 `file_names` 和 `file_locations` 两个 key。它只证明覆盖，不把两个值重新设为当前 alias，也不替代历史清单的路径／名称一致性验证。完整捕获按 repository key 顺序、随后按原 `Commit.entries` 顺序分别补齐两个字段，已有值各自保留。

生产 `MutationPlan::Prepared` 只有 `planning::plan_new_mutation` 一个构造点，所有普通及 detached 发布入口都消费这份已经完成捕获的候选。`PublishedStateBase` 只在决定、状态安装和索引更新成功后构造；其 `Weak<State>` 与当前不可变 State allocation 相同，因而也证明该 State 的命名覆盖。此处不需要新增 marker、缓存、config hash 或磁盘健康检查；图复用和决定点的配置、epoch、成员及 I/O 验证保持原状。

冷启动、恢复或 allocation 不匹配时，planner 仍在业务 closure **之前**对自己的 next State 做一次完整捕获。`begin_tracking` 先执行，因此补齐值进入本次写集；不会修改现有快照 hash 绑定的 State。仅在 closure 之后捕获会让分支切换、删除或重命名丢失原物理归属，所以不能删除冷前置步骤。

当前真实身份写入方只有完整／增量捕获、namespace 显式 insert、workspace／batch 的 entry insert、restore 的 extend 和 split 的 insert；它们保留两个 map 的已有 key。基于这个私有调用契约，未变化的 main HEAD 在 closure 后仍然覆盖。`capture_changed_names` 从 repositories、branches、commits 的精确写集确定前后相关 repository，再比较真实 main HEAD；同 ID commit 被替换也必须处理。只遍历受影响的当前 HEAD，按原全量顺序补齐新缺项；非 main 分支和纯备注／认证写入不遍历 namespace entries。没有为不存在的身份删除入口增加反向缓存；将来若引入这种真实写入，必须同时改变此调用契约及其测试。

`MapChanges::All`、foreign replacement 和写集上限溢出保留完整后置捕获。恢复不会从磁盘反序列化发布收据；`Arc::make_mut` 的本地 plan 写入同样使旧 Weak allocation 失效。独立全量 oracle 在根 `test/`，覆盖首 alias、独立缺字段、main 重绑、同 ID commit、更改／移除／重加 namespace、多代及 All／foreign／overflow；真实发布与 RawState 准入测试另行覆盖 receipt allocation 和冷前置捕获。

精确 warm 路径成本为写集 key 查找加受影响 main HEAD 的实际 entries 大小 S，不扫描无关 repository 的 entries N。没有拆分大 Commit，也不声称消除 State clone、名字容器首写、其他 projection 或 I/O 的全量成本。此切片的源码审阅不代表测试已运行；验证以绑定提交的 GitHub 门禁结果为准。

## 备份路径与保护证明

未激活目录共享布局的主盘按 `file_locations[file_id]` 投影；激活后的主盘使用下述稳定 main 布局及集中定位记录。用户配置备份规则后，备份槽按 `file_backup_paths[file_id][slot_id]` 定位：用户给定的独立前缀，加配置根下的相对路径。单文件规则的相对路径为配置时的文件名。该路径随稳定文件身份保存；主盘文件或父目录移动、改名、切换历史视图时均不重算，显式修改备份配置才更新。新增文件沿用所在目录有效规则，已有子目录或文件的独立规则不会被父规则覆盖。

配置会为该规则涉及文件的所有 AVAILABLE 版本补足固定路由中的副本；不生成新的 FULL 版本或重写 `version.json`，已验证副本不重复占用正文容量预算。只修改前缀时按原子重命名迁移容器，正文 inode 不变。目录规则与每个已纳入文件的策略绑定一并持久化，文件移出目录后保留原副本数和备份路径。

`ProtectionPolicy.inheritable` 必填：现行 scope／路径规则为 `true`，仅供文件绑定使用的保留设置为 `false`，缺失直接拒绝，不补默认值。scope 重配保留现行策略 ID 并推进其代数；仅将仍绑定旧规则、但未被本次覆盖的文件移到一份旧设置快照，保留这些文件的路径和份数。保留设置不参加精确 scope 查找或祖先继承，不会被父目录配置覆盖；显式配置该文件才可重新绑定。无文件引用的保留设置清理，空 scope 的现行规则保留。

同一个版本后来补保护时，首次 `commit-proof.json` 和 `publication-proof.json` 保持原字节。新事务的本次证明保存在 `versions/<version_id>/protection-proofs/<candidate_hash>/`；新增备份槽上首次出现的版本仍使用常规证明路径。候选重放和受限恢复按候选哈希选择对应证明，读取与离线验证仍验证该副本首次完整发布的证明。历史包保存逻辑配置和节点引用，恢复时按路由位置映射备份槽；目标新增槽缺少用户前缀或映射产生冲突时明确拒绝。

导入先完整验证历史包并沿用其稳定身份，保留规则角色及文件绑定；无效 scope 直接拒绝，不自动修补。受支持的拆分创建新身份映射时，不能映射的稳定 scope 引用清除，规则转为保留设置而非路径继承规则。保留设置可以携带有效的本地 scope 引用，但仍只能由绑定文件使用；悬空 scope、跨文件归属、无绑定保留设置和重复现行 scope 均由核心拒绝。导入／拆分的目标规则按既有流程从代数 `1` 开始，不重写原版本证明或新增格式版本号，版本命名仍直接使用 UTC。

发布准备先用实际 JSON 编码器检查每个 container.json 的读取大小限制，并用单个匿名暂存文件保存编码结果，避免同时保留所有文件与副本的缓存字节。决定后才进行物理名称变更和原子缓存替换；目录重命名、文件写入与同步错误仍会传播，不因 cache_only 标记而被忽略。缓存格式不变，超过单对象限制的新事务在 ACCEPT 前拒绝。

已经验证的 Publication 允许名称/缓存修补跳过不存在或权限安全且完全为空的副本根目录。这既包括历史发布重放，也包括通过恢复计划及正文保护验证后新构造的退化恢复发布；仍可从健康正文副本读取历史。现存但身份错误或不安全的根目录不会被当作离线跳过。普通新候选和普通未完成候选不使用此修补模式，必需副本数量及落盘证明条件不变；退化恢复仍须通过其独立的恢复验证。缺失副本也会使旧检查点覆盖失效并回退到权威日志重放。

## 派生成员选择

名称准备消费中立 `state_projection` 的同一 State-bound transition，与 history 编码共享版本成员及 owner 生命周期，不自行维护第二份跨发布缓存。文件版本按 file→history→version ID 选择；位置是旧／新全部版本 route 的并集，PURGED/MIGRATED 与旧位置不被遗漏。pending branches、路径／元数据节点、merge events、文件级 contexts 与 alias-source keys 按真实成员选择；所有描述符值仍读 State。

实际容器存在后才创建 call-local 描述符与标签 memo，按 history 复用副本无关内容，保留原排序、精确 JSON 字节和逐对象大小准入。冷／恢复／All 输入一次重建关系，外部完整验证与 post-decision 物理应用边界不变。该源码切片不改变格式、哈希、正文、目录安全或 fsync；也不宣称消除 SharedRecords 的 BTreeMap 首写复制成本。独立根 test 已补 membership/byte/I/O oracle，执行结果以 GitHub 门禁为准；本地未运行编译、测试或性能测量。


## 目录仓库物理执行与恢复

`Engine::reconcile_directory_layout(repository_id)` 对已初始化的目录仓库显式执行首次物理整理。逻辑初始化与首次整理分开，允许先用既有“分离”操作处理同一文件在多个 main 的冲突。整理完成后，普通发布自动维护已激活历史的布局。入口尚未接 HTTP/UI 按钮；回执只统计结构实体，并列出未找到或不可用的目标容器，不证明正文完整性。

目录仓库位于数据盘 `histories/<history_id>/directory-repositories/<root_directory_id>/`。稳定 `main_branch_id` 的当前已保存工作树决定名称，包括尚未用户提交的保存；checkout 其它分支不会改变稳定 main 的身份。每个独立子仓库保留自己的 main 与 `.zxs-retained-files`，父布局不展开孩子内部。main 中没有、但历史仍保留的普通文件容器按完整 history/file 身份平铺到所属仓库的 retained 目录，不复制正文或历史节点。子仓库本身在父 main 缺失时仍返回具名 `UnplacedRepository`，不擅自选择放置位置。

纯规划在权威 State 中区分当前唯一 main、准确当前 outer main Commit 的 Unmanaged 位置与历史归属证据。前两类当前位置优先；缺少当前位置时才接受唯一历史归属。多个 main、main 与 Unmanaged 重复出现、未知归属或历史歧义均拒绝。路径、保留目录名称、父目录与大小预算在物理写入前检查。

集中定位记录在 history 根的 `.zxs-layout/directories/<directory_id>.json` 和 `.zxs-layout/files/<file_id>.json`，不在用户目录里新增数据库或目录身份标记。记录绑定实际 device/inode、历史/文件身份和有限的 from/stage/to 地址；它们只定位，不授予操作权限。生产调用必须同时持有已发布或已决定的 State、配置准入、完整目标集合与 Engine 写锁。文件继续使用现行 String owner marker。目录实体使用集中记录的 inode；新空目录仅在随机私有 preparation 名下排他创建，从不接管未知空目标。

批处理先验证完整目录闭包、现有实体、最终父关系、未知占用及目标唯一性，再持久写全部定位意图，随后暂存需独立移动的实体、按父序落目录、最后落文件。首次目录定位记录将 from/to 都绑定已核对的当前来源；完整最终目标仍由已准入 State/批次持有，不能把尚被外移文件正文占用的未来后代地址提前写成可遍历候选。每次实际目录迁移仍先写精确来源与该次 staging/目标地址，再移动实体；冷重试从 State 重建目标并重新加载精确定位记录。父目录整体迁移前先写后代的过渡前缀，避免在父目录 staging 期间丢失孩子地址。所有可见移动使用 held-root `renameat2(NOREPLACE)`，同步源和目标父目录；不删除未知对象，也不以失败回滚覆盖后来的实体。冷重试重新读取定位记录与实际 inode 并重建批次，旧批次不能越过 exact-previous 校验。

发布前只分配私有空目录和每文件受控 metadata 父目录，不移动共享可见实体、不写文件首个 owner marker或新格式定位记录。Finalize 后才应用布局、替换 `container.json`，随后既有 held-payload 证明重新按稳定身份找到正文并检查 inode。显式整理只应用当前已发布 State；应用中断时阻塞后续写入，需恢复。

本次排他创建的 preparation 父、叶目录绑定创建时 inode。尚未开始持久定位协议时，拒绝或取消按叶到父顺序清理这些空目录；每次删除在同一准入 I/O lease 内持有父描述符、检查 inode、执行非递归 rmdir 并同步。未知替换或非空目录保留，清理失败记录告警。调用批执行前永久解除叶目录清理权，因为首次定位记录可能已经部分落盘；成功后只清原来的空 request 父目录。创建已可见但同步或准入尾检失败、因而未返回身份的目录仍保留，不把不确定创建当作可删除资源，也不宣称所有失败都无残留。

历史发布回放跳过已集中定位的共享容器，防止中间旧 State 把后续实体移回旧名。CompletePending 在最终 State 验证完成且没有未决/fenced 状态后统一整理一次；PreservePending 不据旧前缀整理。缺失定位记录但共享树仍有相同 owner 时拒绝，不能因此新建第二容器。缺少正文不生成空容器。原 `legacy` / `pending-branches` 检测仍拒绝并保留字节。

数据盘 `metadata/directory-layout.json` 在首次新格式记录或移动之前持久写入固定模式 String。旧可选 replay checkpoint 的覆盖模型只认识每文件锚点，因此标志存在或无法证明缺失时不加载该 cache；含目录仓库的 State 也不保存此旧 cache。恢复仍用权威日志。此处未实现数据库格式升级、外来盘 inode 重绑定或三处数据库安装恢复。

当前执行每次重建所选森林并盘点其完整实体，成本与这些森林的 State 快照/版本数、路径字节和实际目录项数相关，不宣称 O(1) 保存。纯规划限制 1,000,000 步及 16 MiB 路径，物理记录盘点限制 1,000,000 实体、单记录 64 KiB、物理地址 4096 字节/64 层。同步次数随移动实体、受影响前缀后代及记录更新增长；未做性能测量。备份显式路径不被覆盖，primary/backup 同槽冲突明确拒绝。业务响应中的 `physical_path` 旧字段和 UI 激活入口仍需上层整合，不应作为新物理路径授权。

根 `test/` 中新增纯规划、定位、批执行和 public Engine 回归源码；实际执行结果另在绑定本修订的交付证据中记录，源码存在不代表通过。
