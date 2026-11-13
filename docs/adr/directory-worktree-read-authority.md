# Proposed：目录快照登记与 worktree 只读来源权威

## 状态和范围

2026-11-13，基于 `e641a9b3e` 的隔离实施分支。已授权采用明确新格式，
拒绝旧材料且不自动迁移，只允许显式全新根启动。新格式必须阻止旧 writer 继续写入。
目前隔离实现已加入显式 fresh 存储初始化、先于 writer sidecar 的 SYSTEM 格式选择、
新 DISK seal、三根身份记录及完成标记、完整初始化的冷开准入；另有 typed 原始
快照意图、fresh-only 意图存储、Engine 冷发现写围栏、目录 Candidate/材料机制。
首节点发布、同决定节点登记恢复与只读入口已在后继切片接线，但这些代码尚未通过
GitHub 编译或行为验证；已有 host 的 FILE/catalog 位置登记与 SMB 仍未完成。不得将本阶段视为 A 段
完成或目录 worktree 上线。以下字段仍是实施约束，不代表已验证的持久协议。
依据 `TASKS.md` 的目录仓库/worktree 顺序及《工程规范》§2–§7，保留类型证明、
真实准入边界、永久收据和故障行为，不通过兼容默认值、路径扫描或旧目录 Git 接口接管。

本切片仅服务用户明确初始化的新 `.zxs` 快照仓库。文件容器仍是 FILE 权威；
不恢复已停止的非容器文件/目录功能或多文件共同分支树。多级磁盘备份不是本切片。
旧 data 不迁移；未登记的 `.zxs`、同名空目录和遗留准备材料均不自动接管或删除。

## 已有能力与实际调用边界

- `engine/disk_registry.rs` 的 `CurrentDiskRegistry::from_engine` 及
  `Engine::registered_disks/registered_disk` 提供当前磁盘登记视图。登记状态不是物理健康，
  也不是目录快照仓库、节点或 worktree 的登记证明。
- `engine/disk_registry/current.rs` 的 `CurrentNamespaces` 由已验证发布产生，
  当前只覆盖既有分支/目录命名空间，没有新 `SnapshotRepositoryId`/`WorktreeId` 路由。
- `working_tree/admission.rs` 的 decode/structure/select_root/register_directory_chain
  只提供结构闭包、当前登记匹配与准备分配结果。它不证明 catalog 来源真实性或完整性。
- `working_tree/snapshot` 已有完整 manifest、`HeldSnapshotNode::prepare/reopen/open_payload`。
  prepare 返回未注册材料及实体绑定；reopen 要求调用方提供独立决定中的原始绑定。
- `directory_worktree` 已有名字规划、生命周期、空稀疏实例、ledger、冻结来源投影与
  `HeldReadView`。ledger 的 checkpoint 必须由独立登记决定，不从本地最新帧自选。
- 上述新基础目前主要由模块内部及测试消费。尚无生产 initializer、SYSTEM/DISK
  登记恢复或新模型的 server/SMB 消费链。Web 旧 `working_tree_id` 不证明新模型接线。
- 现有 `server/smb_rpc/default_share.rs` 按旧 repository/session 路由，
  `default_share/directory.rs` 提供 root/disk 虚拟目录；不能将 `.zxs` 路径匹配塞入这些
  分支后即认为取得新的快照权限。

## 决定：一个登记权威，不新增平行的 read-mount 权威

SYSTEM 的同一个永久决定绑定一组目录仓库、不可变节点、分支和 worktree 登记，
包括其 catalog 完整版本/承诺、参与 DISK 的准确绑定及操作结果。
DISK 保存该决定覆盖的本地投影和物理实体绑定，不独立选举 HEAD、挂载或 checkpoint。
结构 catalog 是该权威登记的编码/派生载体，不另建可独立更新的 read-mount 数据库。

只有从已准入 SYSTEM 决定恢复出来的完整登记，匹配同决定的 DISK 材料后，才可派生
私有只读 authority。派生还必须复验当前磁盘身份、会话授权、目录 FD 与 ledger checkpoint。
`WorkingTreeCatalog`、`CurrentRootSelection`、本地 marker、manifest、ledger 或路径本身
都不能构造这种生产 authority。无法证明当前登记时明确拒绝，不降级到扫描或旧分支树。

## 首次写入前必须确认的格式字段

以下是逻辑字段要求，不预定 wire 名称或文件路径；实施时使用现有领域类型和 layout。
必须一起定义 canonical 编码、版本判别、严格缺字段拒绝及外部承诺域。

1. **决定外壳**：新格式判别、操作 ID、主体、类型化请求摘要、前驱决定/期望登记代数、
   本次发布序号、完整结果承诺和参与 DISK 集合。复用既有永久决定设施，不再造第二日志。
2. **根与目录闭包**：RootId、StorageLayout、DiskRegistrationIdentity、source chain、
   根实体及 common-data/deleted-data/zxs-data 的 DirectoryId/实体，host/父目录的身份链。
   这些字段必须由当前已准入盘和持有 FD 产生，而非请求自报。
3. **仓库与节点登记**：SnapshotRepositoryId、ReplicaId、原 host_directory_id、
   attached/retained 位置、仓库实体、操作归属；节点的 SnapshotReference 与
   SnapshotNodeBinding（manifest 承诺、repository/node/files/worktrees 实体）。
   保存完整文件清单、准确暴露版本及 `SnapshotFileSource`，不复制 FILE 数据库/标签/历史。
4. **分支登记**：独立 SnapshotBranchId、显示名、准确 HEAD、期望代数和创建操作。
   HEAD 可变，节点 manifest 不可变；不能用旧 DirectoryBranchId 替代。
5. **worktree 登记**：全新 WorktreeId、固定来源 repo/node/branch/manifest、
   SparseWorktreeBinding、ledger Binding 与已决定 Checkpoint、Revision/生命周期，
   可见父目录身份、完整显示名和持久名称预留。可见路由由此派生，不另存平行权威。
6. **DISK 同决定证据**：本地登记投影承诺、所参与 SYSTEM 决定身份、当前盘绑定、
   准备材料精确实体与安装状态。不存在一个仅凭本地自报 hash 即可激活的替代入口。

任何必填字段缺失都不是“空 catalog”“没有 worktree”或“容器不存在”。
部署时没有新格式的旧数据继续走既有行为；不得给它补造新权威或自动迁移。

## 分阶段最小纵向切片

### A. 真实初始化、首节点登记和冷读

用户明确指定已授权目录。写者取得当前登记/代数与持有目录链，捕获现有容器准确暴露版本，
按完整清单准备独立 snapshot 正文；保持父目录对子仓只保存暴露文件的边界。
初次可限定既有基础的有界目录预算，超限明确拒绝，不截断清单。

持久意图先固定操作、分配身份、源版本和准备目标；fresh-only 创建 `.zxs`/节点材料，
验证完整正文及实体、fsync 后，由同一 SYSTEM 决定覆盖登记和 DISK 安装结果。
只有完整决定安装成功后才能报告初始化成功、发布只读查询。
冷开从该决定取得绑定，逐项核验 DISK 登记、源实体和完整材料；不使用“发现同名目录”恢复。

这个阶段的真实消费者应是 Engine 初始化命令及其只读节点清单/正文查询，随后接受认证的
server 边界。名称和错误类型须独立于旧目录 Git。没有真实 producer 与冷恢复链时不发布 API。

### B. 从准确历史节点创建独立空 worktree

在仓库写者围栏内确认来源节点登记、分支和父命名空间；观察完整名称占用并持久预留
`worktree-分支名-001..999`。`names::allocate` 只是规划，不充当并发预留。
创建新实例和 genesis ledger；将准确 instance/ledger/checkpoint 纳入同一永久登记决定。
物理安装采用 no-replace；同节点每次新实例均为空，未改文件来自固定源节点。

本阶段仅开放创建与只读。不能为了测试稀疏读而另设未授权生产写入口；既有修改层读原语
仍由行为测试验证，后续写入必须另行完成受登记决定约束的 checkpoint 发布。

### C. SMB 只读查询/路由

新路由来自已决定登记中的父目录和显示名，而不是可写符号链接或 `.zxs` 扫描。
server 在现有认证/账户/SMB binding 围栏内解析类型化目标；Engine 在当前登记围栏内
取得私有只读 authority，再复用 `HeldReadView` 的逻辑枚举、内容路由及正文验证。
没有授权绑定、登记未完成、软删或待恢复 publication 时不得暴露挂载。

SMB 修改操作在本切片明确拒绝；不转成旧普通目录写路径。未来复制/移动只接受 SMB
明确传达的操作事实，不能根据内容、哈希或名字猜来源。哈希只用于完整性证明。

## 决定与恢复的责任

- 决定前失败：保留原目录/容器和所有未知材料；记录本操作已创建的准确实体及阶段。
  没有持久意图和实体证据时，不拥有清扫权限。重试不得重新分配身份或借用别人的残留。
- 决定后失败：由恢复者按原决定补齐登记/安装，不重新选择分支、名称、来源或 checkpoint。
  对无法验证的材料保留现场并阻止暴露，不“修复”为其他内容。
- SYSTEM 与 DISK 不一致：只允许同决定的恢复路径，读者不挑一个看似更新的版本。
- 盘退出、重新绑定、会话撤销与 ledger 前进：新 open/下一页按当前围栏重新判断；
  旧 handle 不因显示路径相同而转向另一实例或来源。
- 缺盘、不可读、权限失败和登记缺失均不证明原容器物理不存在。不得据此跳过同步、重建
  容器或切换任意来源；本只读切片不实现容器同步决定。

## 持有式读取，不构造自引用 SMB handle

`HeldWorktreeLedger<'a>` 借用 `HeldSnapshotNode`，`HeldReadView` 又借用 ledger。
不可把三者强行放入自引用长寿命 handle，也不使用延长生命周期或泄漏 FD 的绕过方案。

Engine 在一次受围栏保护的查询中持有 source/ledger/view，返回有预算的页或受控 ReadFile；
SMB handle 保存独立的登记/会话/view stamp、准确目标身份和必要受控读句柄，不保存借用链。
后续页校验同一 checkpoint、来源承诺及父目录 cursor；视图变化返回明确失效，不拼接两代。
正文读取必须保留现有动态会话复验和磁盘 I/O 围栏。已打开 FD 的精确版本语义不能覆盖
权限撤销语义；服务端 read/close 责任须纳入实际消费者测试，不只测 Engine open。

## 测试、CI 与完成口径

当前仅设计，**未执行本机编译、测试、格式化或 CI selector；没有新增行为通过声明**。
基础分片通过只证明基础实现，不证明这些生产消费者存在。

实施选择计划：
- 保留 `directory-snapshot-material`、`directory-history-worktree` 原有预算与全部选择；
  不以新组替代原覆盖，不恢复旧非容器能力。
- 新增初始化/登记专项：真实 initialize→prepare→决定→冷开→读正文；同操作重试、
  各持久阶段中断、双发初始化、未知 `.zxs`、实体替换、损坏/遗漏正文、过时代数、离线盘。
- 新增 worktree 专项：同节点独立实例、空稀疏层、冻结来源、并发名称预留、999 耗尽、
  名称大小写碰撞、软删实例不污染新建实例、旧 checkpoint/待恢复 publication 拒绝读取。
- 新增实际 server/SMB 专项：账户/绑定撤销、盘重绑定、受控 FD 生命周期、分页跨代、
  非授权路径不能触达管理目录、写请求不落入旧路径；随后真实 Samba 验收。
- 按实际修改同步 SYSTEM/DISK 候选与恢复消费者的 CI 映射；验证新构建库存、非零选择、
  精确 SHA 的执行完成及未执行范围。禁止仅以静态字符串/测试名证明行为。

A、B、C 各阶段只有真实 producer、持久决定、冷恢复与消费者行为闭合后才分别报完成。
目录提交的 MOVE-only 正文安装、FILE 同步、未提交删除及备份仍是后续独立责任。

## A 段实施检查点（未验证）

- `snapshot_initialization/intent.rs` 固定操作、principal、源发布/节点、目标根和实体、
  完整参与盘身份集合、首节点清单与材料承诺。准入不授予权威，禁止以 SnapshotOnly
  替代初始化所需的真实 FILE 来源。
- `snapshot_initialization/store.rs` 保留首次 canonical 原字节，拒绝未知或不同意图；
  同操作精确重试仍复验 held FD、路径身份、全量字节和 stamp，不覆盖原文件。
- Engine 在 SYSTEM 与可用 metadata DISK 发现原意图后阻止普通写入和后台维护。
  离线盘不被解释为缺少意图；本阶段不会据此恢复、完成或采纳任何目录材料。
- `snapshot_initialization_tests.rs` 仅覆盖意图存储的冷读、精确重试、冲突与破坏拒绝，
  不使用 synthetic 根声明生产授权。定向组保留 8 项/300 秒预算。
- 未运行本机编译、测试、格式化或 selector。后续仍须实现新格式 writer 排斥、
  真实 fresh-root producer、typed 决定材料、恢复与只读 consumer，并增加真实故障回归。

### 首轮编译检查点

- `Engine::initialize_directory_storage` 仅消费调用者明确选择的既有空私有根。
  SYSTEM `initialized.json` 使用新严格格式，旧 writer 本来就拒绝该槽；在任何
  writer sidecar 前先以 exclusive create 建立它，半写入拒绝而不自动修复。
- 新 DISK seal 绑定原 selector commitment；本地 FILE proof 必须核对其原 selector
  副本与 Config/实体绑定，不能将新格式降级成 ManagedDirectory。
- 三根绑定和完整标记一致时可冷读原根；selector-only 可同操作重试。任何无完整
  身份证据的部分初始化保留原字节并拒绝，不宣称所有故障点都可自动完成。
- 原始 Config 序列化不携带运行时 fresh 准入；普通 decode 不授予新 seal 权威。
  新根暂拒绝与 membership generation 混用，未扩展备份或拓扑变更。
- 新目录 Candidate envelope 保留 Genesis/Membership/Registry 各自已有权限域，
  完整哈希绑定 typed directory material；这仍需后继真正决定与 DISK proof 接线，
  不能只凭候选或 OperationResult 中的 JSON 开放读取。
- 初始化专用 source 冻结保留租约并要求完整当前清单与原捕获严格一致；preview
  原先允许保留冻结旧版本的语义不变。所有测试仅已编写，未本机运行。

### 显式 FILE 来源到首节点的后继切片（未验证）

`initialize_directory_snapshot_from_source` 只接受已验证的实际 FILE/Current namespace
来源和一个已独立准入、common-data 完全为空的新根。它不从名字、正文相等或已有
普通文件猜测复制来源，也不把 raw catalog 当授权。永久候选只新增该操作记录与
sequence；完整 node material 绑定原意图和同一个 candidate，在原 FINALIZE 后才
以 NOREPLACE 安装 `.zxs`，DISK 决定证明绑定原 finalize。全部 publication seals
验证后才建立只读登记；冷读再次验证原登记、当前盘/根实体、完整正文与决定证明。

这不是就地初始化一个已有 FILE 的 common-data 目录。普通 FILE 保存目前仍使用
既有容器/Current producer，尚未形成 FILE→working-tree catalog 的生产位置登记；
该映射以及真实 SMB 路由仍须后继实现。本阶段保留单一原初始化 intent 槽，只允许
池的首个显式来源快照；另一操作在写围栏之前拒绝，不能覆盖或删除原意图。

已编写真实非空 FILE 来源、完整副本/原来源不变、冷开/精确重试、未知 `.zxs`
全盘不变、正文/实体替换、离线拒绝与 ACCEPT/FINALIZE/Publication 故障恢复用例。
未在本机运行。首节点执行与冷读尚无 GitHub 行为验证结果，不得作为 A 已完成交付。
