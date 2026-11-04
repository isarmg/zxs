# 当前登记命名空间：消费者与工作区准入

状态：源码实现，尚未编译、运行测试或验收。此记录只覆盖当前读写消费者；正式 D3、固定成员证明、bootstrap 与真实 I/O 停止由生命周期实现提供。不能用本文的内部测试夹具代替这些证明。用户语义见[直接退出登记](../design/direct-disk-deregistration.md)。

## 准入和可见性

`PreparedCurrentNamespaces` 从完整验证的 registry successor 与 occurrence mask 构造，但准备本身不激活。历史回放与活动安装共用 `PreparedCurrentNamespaces::admit`。唯一生产安装点只接受不可由调用方构造的 `AdmittedDiskExitPublication` 的 Live 证明，Archived 证明只能生成回放 view；安装另核对当前 generation、规定 first sequence 和 source snapshot，再核对 registry commitment、mask、源 snapshot/sequence/business generation、实际 D3 snapshot/sequence 以及冻结 branch/directory heads。证明由正式生命周期发布或恢复验证产生，不能用请求提供的摘要、序号、内存布尔值或 workspace manifest 代替。

- Engine 与 MetadataView 的当前 namespace listing/lookup 使用同一 owned entries；当前目录条目另有 `current_directory_entries`。`registered_disks`/`registered_disk` 从当前 registry 枚举，Detached 不出现，Disabled 保留并标明模式。
- 原 `Commit.entries`、`DirectoryTree.entries`、版本、布局、永久历史回执不改写。显式历史查询仍使用原记录；历史记录存在不承诺正文仍在线。
- 任何携带 registry 的 Exit/Add/Abort authority 已存在但当前 view 未获准入时，当前 namespace 读取与普通 mutation 返回恢复错误；可选 file-context 查询返回缺失，登记枚举不回退到旧 Config。MetadataView 将 Initial、AwaitingPublication、Published 区分开。
- `namespace_snapshot` 只持有一个共享 Commit 或投影 entries 的 Arc，不持有全部 State；弱 witness 同时核对 registry 与 entries 的分配身份。仅认证状态改变可保留同一快照，D3 或命名空间变化使旧 witness 失效。数据所有权本身不是读权限，传输层仍须逐次授权。 `directory_namespace_view` 以 O(1) 共享目录表和对应投影表，锁外才按目录 id 取 entries；不持有全 State/history，也不在锁内遍历或复制条目。可由调用方构造的 `NamespaceEntries` 只提供数据，不能转换成声称已准入 canonical source 的 `NamespaceTree`。
- 普通提交在持久化前准备下一 owned view。成功发布 State 时同时切换 view；失败不把未发布 draft 安装为当前视图。普通后继 Candidate 持续携带活动 registry commitment。
- 恢复逐个装入已认证 Candidate，在准确 D3 之后立即重新验证并安装 view，随后才能推进普通提交。所有携带 registry 的 generation 的 checkpoint load/save 暂时全部禁用，不能从晚期 raw heads 猜测丢失的投影。
- 普通写入仍要求当前 view 的 commitment 与已准入 generation 完全相同。无 view 的冷恢复只有生命周期实现核实全部历史 stop 的真实全域 I/O 排除证明，才可按保留 `config.io_disks()` 检查恢复锁；它不开放业务读写，也不改变固定投票成员的 Q。

## 实际写入路径

新普通 workspace 的选择、上传 base/version/context、entry/file identity 和最终 commit draft 都使用当前投影。同名重建不能回退到旧 HEAD、隐藏 path context 或 pending reservation，必须分配新 file identity，并拒绝调用方复用历史 entry identity。改变保留项时，新提交不能携带未改动的隐藏项。

file-manager-root 的普通保存已接入 `prepare_file_write`、文件分支辅助创建、`synchronize_directory_edits` 和目录快照刷新。最初的投影 materialize 时，除本次写入路径外，还刷新隐藏路径的全部选中祖先；避免新根仍引用包含隐藏项的旧 child tree。只移除这些路径的当前 write context，不删除其它 occurrence 仍使用的全局版本、选择或历史。

直接和 workspace 的 Mkdir、Delete、DeleteTree、Rename 使用当前 entries。Rename 只按准确 entry/file/version/size 消费已有可见 occurrence，保留 alias 数量及各自版本；不会把保留 identity 当作跨 scope 复制许可。case-only、嵌套目录及后续独立 file-branch 保存同步对应 path-node/context。恢复重新计算同一规则，不使用内存中临时授权印记。写入及策略更新不得路由到 Disabled 或 Detached slot；不自动提升 backup。保留的共享内容仍能列出，已移除 owner 的 namespace 没有隐式写权。

新 repository/disk-root 可以使用当前 Enabled 一级登记作为 owner。新 scope 必须是本次新建的 repo/history/main/empty root commit，没有历史 entries、pending version 或父 commit；原 repository 的 helper 仍继承真实可写 main。新建操作先核验当前路由，旧成功 receipt 则按原身份/请求重放，不被后来退出改写含义。未使用的 create_disk_root operation 不得靠历史 Config 找到已退出 root 并当作当前成功。

以下操作本批仍在 planner 调用任何变更闭包前拒绝：批量移动/复制、merge、目录/文件选择、历史导入/历史 workspace、从旧 commit 创建分支及其它未获当前 namespace 准入的 mutation。同次上传＋Rename 已接新版本与准确 occurrence 证明；若后续 Delete/DeleteTree 会丢弃某份上传，仍在版本构建前返回 WorkspaceDependencyBusy 并保留暂存。普通 workspace commit、上述 namespace 操作、新空仓/磁盘根、当前 directory.scope.initialize、placement.configure、纯 file/version note 与 branch label、placement/protection 策略更新和认证状态更新已允许。这是精确可见的临时能力边界，不是最终产品限制；跨 scope 的 relocation/import 必须先补真实投影语义，不能只移除 gate。

## 不导入内容的元数据编辑

file note 和 file-branch label 只操作实际当前投影中仍存在的 identity；显式 version note 仍绑定准确 file/version，保留历史语义。directory-branch label 必须匹配当前 owner 投影的 directory identity，同名新目录不能借旧 branch。Disabled 的空 root 仍有登记；Detached 的空 root 不因改 label 重获当前身份。保留内容 scope 的 label 不提升 primary 或授予正文写权。

目录 label 发布不能仅凭相同 head 字符串沿用投影。`advance` 穷尽比较除 name 外的所有 DirectoryBranch 字段，并核对实际 immutable DirectoryTree 值不变，才共享旧 owned scope；修改 entries/path/refs/context 或原 tree 不能走此路径。该规则同时用于恢复，并新增失去变更跟踪后的源码正负例。元数据投影缓存须使用另一个真实 I/O 修复补丁，跳过已排除目标；仅打开 operation gate 不足。

## 持久 workspace 基线

新 workspace 在持久 manifest 前发布独立 `workspace.namespace.admit` 永久操作。严格记录绑定 registry、mask、workspace/branch、base head、principal、fencing token 和投影 entries 摘要。工作区正文、manifest 以及 request 自带的字段均不具有提升旧 workspace 的权威。

恢复只从已认证 State 中的成功永久操作重建索引，检查 operation owner、原 request digest、当前 registry/mask、head/history 与 entries 摘要。不从 manifest 的声称推断准入。原投影 head 使用保存的 source entries，其后的 head 使用对应不可变 commit。索引也覆盖 cold committed workspace，不要求 manifest 常驻内存。旧代 workspace 仍保留暂存文件和可检查证据，但不能静默升为新当前基线。

准备与提交线性化点均复验 registry、head、principal 与 fencing；已提交原 workspace 只接受既有完成协议的 fence + 1。跨代旧准备不得提交。现有幂等结果仍可按原请求身份重放，不裁剪永久收据。

## 成本

首次退出准备仍遍历完整冻结 occurrence 图并建立历史 identity 集合。owned view 只复制当前 scope 的保留 entries；不可变历史不复制/重写。当前查询按规范路径二分查找，不为一次 lookup 扫描全部历史。Rename/恢复新增当前 scope 的 O(N log N) occurrence 比对和 O(N) 临时索引，没有正文复制或额外发布屏障。

普通发布使用 State 的现有变更键跟踪；未改 namespace 的发布共享所有 view 容器。有 head 变化时复制对应当前 scope 表的 Arc 元数据，重新持有变更 scope 的 entries，并增量更新 identity 索引。首次修改共享 identity 集合仍可能复制整个集合；Rename 改为准确 scope occurrence 证明后，已经移除无消费者的全局 retained-path/file 缓存，避免继续累加和复制旧路径。必须保留的 known-entry/file 身份禁止集合不变。本批没有声称大规模吞吐或分配预算已验收。恢复丢失变更跟踪时允许扫描当前 scope，不以猜测空 delta 省略校验。

每个新当前-registry workspace 新增一次完整普通发布及 entries 摘要计算，未合并或删除既有同步屏障。永久 workspace 准入索引为 O(W) 小型绑定记录，不给每个 workspace 复制 entries；分离提交准备只复制该 workspace 的一条绑定。恢复扫描永久 operation，并对准入的 entries 校验摘要。Add/Abort 延续复制当前 scope 表的 Arc 元数据，不复制 entries，保留身份禁止集合；后续 Detach 必须重新计算准确当前 occurrence 并维护历史身份集合。大历史/大量 workspace 的成本仍需基准。

## 待执行的用例

源码在 `test/engine/unit/engine/current_namespace_tests.rs`，前缀 `membership_registry_current_`。夹具只在私有测试模块安装已验证的语义 view，明确不模拟活动 D3 或 I/O 关闭。覆盖：

- 当前 listing、lookup、file context、MetadataView 与目录 entries 不显示目标独占内容；历史和原物理文件不变。
- 同名上传取得新 entry/file/source identity；第二次保存继续新身份。
- 仅修改保留文件不复活隐藏项，选中 child tree 同步刷新。
- DeleteTree 只操作当前保留条目，保留历史版本。
- 旧 workspace、旧 entry ID、错误 principal/fence 拒绝；永久 receipt 可恢复正常和 cold workspace 基线。
- Rename 隐藏同名目的地、case-only/嵌套、链式 Rename/DeleteTree、无关 upload、不同版本 alias、独立 file-branch 保存及跨 disk-root 隔离；反序列化 State 通过恢复安装入口重验正负例。
- 历史分支/工作区导入在修改前拒绝；上传＋Rename 保留新正文和来源，上传最终被 Delete/DeleteTree 消耗则保留精确拒绝边界。
- 当前 Enabled owner 上新 repository 与 disk-root 可新建并保存；Disabled/Detached owner 拒绝，旧成功创建 receipt 仍可重放；非空/历史 root 不能借新建 scope 取得写权。
- 显式目录初始化按当前 identity 捕获内容与子目录 refs，不用旧路径复活旧选择；directory label 保留全部 snapshot 数据，变更同名/原 tree 的负例拒绝。
- Disabled 保留列表与登记但拒绝目标写路由；Detached 不再登记；可显式更新到保留 primary 并写入。
- 移除 owner 的共享版本仍可表示，但不能创建可写 workspace。
- AwaitingPublication 的 metadata snapshot 不回退到 raw head。
- 第二次 Detach、Disabled→Detach 与 Add 延续不从原始 head 复活旧项；已 materialize 同名新身份的 path context 不会被下一代旧 hidden-path 列表清除。
- 薄 namespace snapshot 在投影变化时失效，即使 raw head 没变；无 namespace 变化的 advance 保留 entries 身份；规划 scope inventory 不吞掉陈旧 head 错误。

必须另执行真实生命周期端到端：首次 D3 与重启、普通提交后重启、每个发布/停止屏障崩溃、暂存中的旧 workspace、HTTP/SMB 各当前入口，以及物理目标从第一条 stat/open 起无探测。原历史读、workspace/目录选择、并发准备、恢复、membership、严格 Clippy 与架构门禁也须由统一验证检查。此处仅完成定向 rustfmt 和 `git diff --check`；没有运行 cargo、测试、模型或基准。

## 后继代数与尚未验收的边界

消费者工厂现在接受准确前代 owned view。连续退出和 Disabled→Detach 先验证完整 registry continuation 与当前 source 匹配，再从前代投影计算新 mask；历史 Placement 通过历史 binding 校验，remaining holders 只数当前仍登记的成员。历史 owner 可以已经退出，不伪装成当前 RegisteredDisk。所有旧 identity guards 和尚未 materialize 的隐藏路径继续保留。

Add/Abort 通过 `prepare_continuation` 保留当前 entries 与 guards，重绑本代 registry/source；每代使用本代准确 mask（Add/Abort 无新 mask），不会错误要求上一动作的 mask。每次准入记录本代 activation sequence，不能拿 Abort 继承的相同 registry 冒充新一代 D3。恢复必须由正式 archive verifier 依次认证并重放历史普通发布和每个规定 activation，到达准确新 source 后才可安装当前 Live 证明；仅有 raw State、旧 registry root 或请求的 mask 均不足。

这些工厂依赖生命周期实现的完整 registry continuation、archive replay 和 Live/Archived 证明区分，尚未编译或运行验证。当前文件分支 merge/选择、目录选择与 selected-head 批量已有精确来源消费者；完整历史导入仍按实际 planner gate 拒绝。投影 selection tree 不等于 branch head 的后续批量仍在闭合，不能套用原 head 消费者。解除 gate 必须先接好对应身份和 scope 的真实语义。Rename 与新建 empty root 的 source 已接通，真实生命周期→Add→新盘建仓/保存仍须统一端到端验证。

同次上传与 Rename 的新版本、路径节点及恢复证明见 [工作区写入与改名](workspace-rename-writes.md)。上传被 Delete/DeleteTree 消耗仍保留明确拒绝，不能猜测未记录的用户操作顺序。
