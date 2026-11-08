# 辅助工具

`storage-helper` 生成 `zxs-helper`。它是在本机显式运行的离线辅助工具，不是远程管理客户端，不启动 ZXS 守护进程。普通文件访问和日常账户管理仍由主服务 Web 与独立 SMB 提供。辅助工具须配备自身独立 Web 控制台，以 root 手工运行、默认监听 `0.0.0.0`、无需认证直接打开，不依赖 systemd 或自启，退出即停止；并负责首次账户/系统及必要磁盘配置初始化、ZXS 管理员与 SMB 密码重置、一条龙 ZXS 安装及 systemd/SMB 配置。发行物还须内置匹配 ZXS 载荷，用于离线首装或仅程序文件修复。本片加入独立 Web 控制台源码，提供逐盘正文检查、核验后下载、真实退出及内置 ZXS 程序的固定路径首装；完整控制台、初始化/重置、配置和覆盖修复仍未实现，当前组合未构建或运行。具体接口和限制见[Web 首片](../design/helper-console.md)及[程序首装](../design/helper-program-installation.md)。

源码放在项目根 [`helper/`](../../helper/)，测试集中在 `test/helper/`；与服务共享根工作区、锁文件及 ZXS 的通用格式、验证和持久化能力。辅助工具专用流程、参数、报告和退出策略在本目录实现。构建命令为 `cargo build --locked --offline -p storage-helper`；当前只生成 `zxs-helper`，不安装旧名称别名。

已确定将**磁盘添加、注销/移除、外来磁盘接入和磁盘初始化**移入此工具，并取消用户发起的“计划下线”动作。失联、不可用和既有 Disabled 状态仍须如实展示。这是职责决策，不表示四项操作已经迁完；分项现状、原操作恢复及切换门槛见[辅助工具磁盘管理](../design/helper-disk-management.md)和[架构决策](../adr/0017-helper-disk-management.md)。当前可执行命令以下述各节及 `--help` 为准，不注册只会假报成功的磁盘管理占位命令。

所有需要打开池的救援操作应先停止该池的服务。`--root` 指向系统状态目录，`--principal` 指定审计与操作所属主体，默认 `admin`，不是角色或 ACL 提权入口；独立 Web 已有管理员密码重置，需手动输入和独立确认；实际 SMB 密码重置尚未实现。帮助、版本与参数错误不启动服务或工作线程。失败返回非零退出码。

需要打开或恢复池的命令支持[四项保留证据运行期上限](../19-capacity-and-performance.md#runtime-retained-workspace-limits)，作为全局参数同时提供；不带时使用原配置基线。`offline-check` / `offline-export` 直接核验单盘，不借这些参数改变证明或载荷规则。恢复前应配置足够额度，不能通过删除已提交清单释放名额。

## 恢复系统盘

```sh
zxs-helper --root /srv/zxs/new-system restore-system \
  --disks /srv/zxs/old-metadata-1 /srv/zxs/old-metadata-2
```

上述为多成员示例。实际至少需要固定配置的 Q = floor(N/2)+1 个可信原元数据成员，N ≥ 1；单成员池可只传唯一旧盘。不能拿空盘、替换盘或任意目录替代，不能按在线数量重算成员集合。所有新初始化盘保存池配置锚；缺锚旧盘不兼容，不自动补锚或迁移。核心校验发布证据、成员与池身份并保留冲突拒绝规则。工具不格式化磁盘、不自动重置、不在多成员池中根据不足 Q 个旧成员推断整个池的最终状态。单成员盘永久丢失不具备原池恢复保证。

输出中的 `credential_recovery_snapshot_available` 只表示恢复后的池中存在凭据灾备快照，不表示已创建或替换管理面权威凭据。工具不写旧 `auth.json` 缓存，也不覆盖独立系统目录中的 `management-auth.json`。随后启动守护进程或重试存储发现都不会自动导入该快照，无论是否已有权威标记。有效的本机管理凭据继续作为唯一权威；凭据缺失或损坏时保持认证关闭。需要恢复管理凭据时，保留此次恢复目录作为核验依据，停止相关服务，并在另行准备的干净系统状态目录中走受本机 bootstrap 授权保护、明确接受旧凭据风险的恢复流程。不要删除或覆盖现有管理凭据、权威标记或部分恢复目录来绕过保护；工具不提供凭据重置命令。

`restore-system` 的 `restored:true` 只表示原权威 State 恢复。新增 `restored_scope:authoritative-system-state`、`all_database_scopes_restored:false` 和逐盘当前/历史派生状态，避免把某一可选库失败或尚未实现的三范围恢复报告为全部完成。权威恢复后才维护派生盘库；错误不授权删除恢复材料或重复原决定。

## 无 SYSTEM 的盘根数据库清单

`zxs-helper disk-databases SELECTION [--scope both|current|history] [--records]` 根据明确物理登记读取同一盘的当前/历史 SQLite，严格折叠历史前后行，并在覆盖范围内比较当前清单。支持明确仅读仍完好的一份，另一份保留且不访问；`--records` 将已经完整读取的清单按 JSON 行输出 stdout。它不打开 SYSTEM，不写源盘，不是独立发布证明或恢复权威，不检查正文可用性；原始材料缺失/损坏不被解释为空盘。该源码尚未编译或运行。身份选择、CLI 与边界见[离线数据库材料](../design/offline-disk-database-material.md)。

## 受限降级恢复

```sh
zxs-helper --root /srv/zxs/system recovery-preview OPERATION_ID --reason '经审查的原因'
zxs-helper --root /srv/zxs/system recovery-commit RECOVERY_ID \
  --digest REVIEWED_DIGEST --accept-permanent-loss
```

仅支持已经获得 ACCEPT 但尚未发布、所有原始数据盘都可访问且正文确已不可修复的操作。执行时必须持有池的独占锁及适当本机权限。先审查持久预览和损失范围，再提交完全匹配的摘要并明确接受永久数据损失。缺盘替换、自动重试发布、自动 reset 和任意撤销已发布历史均不属于此能力。

`recovery-commit` 若在原操作已发布之后因索引重建或工作区核对失败，仍以非零退出码报告本次恢复未就绪；诊断明确包含原 operation ID、已发布事实、恢复阶段及具体来源。这个退出码不表示原操作未发生，不能换新操作号或另一恢复计划重做。保留原计划、摘要及恢复材料，排除故障后按既有严格恢复准入核对；此工具尚未提供绕过 Engine 重开失败的独立收据查询。

<a id="integrity-scrub"></a>

## 显式完整巡检（第一阶段）

```sh
zxs-helper --root /srv/zxs/system scrub
```

先停止该池服务。此命令是完整巡检执行器的真实离线调用方，不是定期后台调度，也不替换 HTTP `/api/admin/check` 或 `offline-check` 的单盘目录诊断范围。当前 `Engine::scrub` 在整次扫描中持有 Engine 独占借用；不得把本命令改为默认周期在线任务，或据此宣布增量发布免读门禁已经满足。

命令先安装 SIGINT / SIGTERM 接收器，再在一个专属本机线程中打开池；系统目录和可用磁盘的既有非阻塞写者锁拒绝同池服务或第二个巡检进程，不排队积压。打开使用 `PreservePending` 恢复入口，不自动提交未发布决定。**打开阶段仍会执行既有恢复、日志尾修复、已清除正文清理和可重建索引维护，不是整个命令只读。** 只有随后的 `scrub` 扫描是无隐式修复的观测；恢复可能先修复可恢复的元数据副本，因此报告反映打开之后的实际字节，不能当作打开之前损坏清单。无法安全打开时不运行巡检、不输出健康报告，错误写到 stderr，退出码为 1。

SIGINT / SIGTERM 只请求协作取消，不终止或分离 I/O 线程。核心在对象、日志帧和正文块之间检查取消；已在执行的系统调用及打开阶段必须实际结束。命令等待线程 join 后才返回；调用 future 被取消或销毁也先取消并真实 drain。OS 强制终止、设备永久阻塞及物理断电不属于协作取消的完成保证。线程创建失败、线程 panic、信号安装失败和核心准入错误都明确失败，不伪装成完整报告。

完成后 stdout 只输出一份 JSON：`state` 为 `complete_success`、`partial_failure`、`cancelled` 或 `failed`，并包含核心的发布 `sequence` / `snapshot_hash`、实际覆盖计数、至多 128 条 issue、截断数量及起止时间。时间使用 `{ "secs_since_epoch": …, "nanos_since_epoch": … }`。只有完整覆盖且无故障的 `complete_success` 返回退出码 0，并令 `last_complete_success_at` 等于本次完成时间；其它扫描结果仍输出部分覆盖报告并返回 1。输出或编码本身失败同样返回 1。

`last_complete_success_at` **只属于本次调用**：命令不保存跨进程健康缓存，不读取普通写入成功时间，不宣称先前结果仍新鲜；每次启动从未检查状态重新准入。报告是扫描区间观测，不能用作未来读写许可、当前健康凭证或自动修复授权。离线阶段没有 HTTP 任务队列、在线 status/cancel 路由及自动周期策略；最终在线方案必须先完成有界锁外执行、具体后台 owner、恢复 drain 与前台资源公平性的独立设计和验证。

## 单盘核验与隔离导出

```sh
zxs-helper offline-check /srv/zxs/old-disk
zxs-helper offline-export /srv/zxs/old-disk VERSION_ID /srv/helper/new-file
```

在停止写入的可信原成员或一致的只读副本上操作。核验或导出只证明被检查的发布证据、单个版本与其数据完整性，不证明整个池可恢复、当前在线认证、最新历史或所有副本健康。导出目标不得已存在；`offline-export` 的输出路径须为有效 UTF-8，以便精确写入 JSON 收据，不可编码的路径在读取源盘或创建目标前拒绝。不要把隔离导出结果直接当作可重新加入池的磁盘。

## 历史恢复包

```sh
zxs-helper --root /srv/zxs/system package export HISTORY_ID /srv/private-helper/new-package
zxs-helper package check /srv/private-helper/new-package
zxs-helper --root /srv/zxs/new-system package restore /srv/private-helper/new-package \
  --primary-slot P01 --copies 3
```

保留原有完整历史包导出、独立核验和新池恢复能力，不是 Web 文件传输。导出目标不得已存在，父目录必须为当前用户拥有的私有 0700 目录；包的路径、发布证明、摘要、对象与历史结构仍由原核心实现核验。没有外部可信成员配置时，独立包检查仅证明包内证据与内容自洽，不赋予来源池成员关系权威。恢复要求目标池已安全初始化，保留目标身份、副本和新池恢复约束，导入形成目标池的新本地发布，不沿用外来发布作为本池权威，不能覆盖现有目标或绕过损坏证明；可用 `--operation-id` 指定经审查的幂等操作标识。

<a id="uninitialized-configuration-drafts"></a>

## 未初始化拓扑配置草稿

```sh
zxs-helper configuration template --production \
  --disks /srv/zxs/disk1 /srv/zxs/disk2 /srv/zxs/disk3 --output /srv/private-layout/custom.json
zxs-helper configuration register-disk --config /srv/private-layout/custom.json \
  --slot P02 --disk-id REVIEWED_DISK_ID --path /srv/zxs/disk4 --level 1
```

此子域只生成或编辑未初始化的配置草稿，不打开池、不初始化设备、不注册在线磁盘，也不提供账号初始化或远程客户端。`template` 生成随机池/盘 ID；必须显式指定 `--production` 才是生产草稿。`register-disk` 保留原草稿到完整文件名加 `.before-registration` 的备份，再以描述符锚定的原子写入发布 0600 文件。再次编辑保留紧前一份草稿。从读取、校验到备份和发布，整个事务持有父目录的排他建议锁，而不是会被原子替换的草稿 inode；同目录并发编辑（包括父目录别名）在读取前明确失败，须待前次编辑结束后重新运行，不会静默覆盖成功编辑或保存过时备份。锁不创建边车文件；标记检查及两次发布始终使用同一已打开的父目录描述符。此锁只协调救援草稿编辑，不能替代停止守护进程及排除其他写入者。已有池或管理凭据标记、已入池盘、复制出去的在线配置、链接、重叠根目录、非法拓扑及不可信文件均被拒绝；不能用不相关的 `--root` 绕过这些检查。

当前 Web 首次设置只创建管理员，登录后通过已挂载文件系统的目录浏览与检查完成首批存储激活。旧 Web 本机预设应用入口已移除，以上草稿工具不再构成可直接从 Web 应用任意 Config 的路径；仍不允许修改已初始化池。`--setup-disk` 只为既有离线恢复提供允许的原磁盘 ID。新目录选择、设备身份与中断恢复边界见[初始化与恢复](../management/setup-recovery.md)。

## 副本与发布约束

除系统盘外最低只需一块存储盘，后续备份层均可不配置；单盘没有冗余或故障盘容错。`configuration template --disks /absolute/primary` 可生成单盘草稿，增加路径时按顺序形成备份链；草稿本身不访问或初始化设备，实际使用仍须校验拓扑和设备。

正常元数据发布要求全部已配置元数据成员持久化成功；严格多数仅用于相应恢复/准入流程，不能自动把失联成员移出配置。`package restore` 默认一个副本；需要更多副本时显式设置 `--copies N` 并预置足够的备份链。

## 检查

运行 `cargo test --locked --offline -p storage-helper`。`test/helper/startup.rs` 覆盖命令边界、损失确认、元数据恢复、历史包字节一致性及拒绝覆盖/损坏证明；`test/helper/unit` 覆盖草稿拓扑、权限、锁和原子备份，以及巡检的真实信号、取消、future 销毁、线程失败和 Engine 锁释放；巡检集成用例实际运行命令并检查健康、缺盘和准入失败结果。测试不替代真实设备及生产恢复演练。

## 超额工作区只读库存

停止守护进程后运行 `zxs-helper --root /absolute/private-system-root workspace-inventory`。
命令读取现有配置、校验磁盘身份并持有原写锁，但不构造 Engine、检查准入额度、重放 journal 或修复数据。
每个可用位置的物理 `workspace.json` 单独输出一行 JSON（路径、ID、主体、状态、canonical 长度），
全部枚举结束才输出 `complete:true, authoritative:false` 和物理清单数。镜像不去重；长度不含生命周期预留，
不能代替 capacity 计费或权威发布核验。缺盘只表示未列出相应可用根，不宣称完整物理池健康。
格式、路径或 I/O 错误中断命令，不输出完整成功标记；已有输出仍是部分诊断，不得据此清理数据。
枚举器只保留配置根、一个目录游标和当前受单清单上限约束的内容，无每 ID 集合；因此既有超额池也可查看。
除原有 writer lock 文件可能被创建外，不修改配置锚、清单、正文或索引；该命令不授予超额池写入能力。

### 草稿发布失败的证据与诊断

草稿输出父目录可为公共目录，不会被改成 0700；文件仍为 0600、单链接普通文件，
全部操作锚定持有目录。rename 开始尝试即解除暂存自动清理，包含 EEXIST 的失败
回复都作为 `Unknown`，保留原 errno、目标/暂存名称及目录和文件身份。
成功 rename 后验证或目录屏障失败为 `Published`；不得根据非零退出码删除证据
或盲目重试。发布前写入/同步失败才清理暂存，清理失败与首要错误一并保留。
备份失败携带具名路径/source 上下文且不继续替换草稿；这不表示备份本身未发布。
CLI 最终诊断遍历完整来源链，并转义控制字符；不打印草稿正文或秘密。

隔离门禁 `test/helper/unit/private_io/publication-io.sh <helper-libtest> <zxs-helper>`
要求 `cargo test --locked --offline -p storage-helper --features rustix/use-libc --no-run`
生成的当前产物。它执行真实 rename 前/后 EIO、真实 EEXIST 及文件/目录同步故障，
核验阶段、errno、inode、暂存/目标保全、目录权限和屏障；同时核对真实 CLI 备份失败
的完整来源链和未修改原草稿。libc 测试后端不改变生产后端，也不冒充 NFS/掉电测试。
该修复不改变草稿两次发布的顺序或成功路径每文件一次 fsync、每发布一次目录
fsync 的预算；新增持有文件单链接/身份检查及发布后相对 stat 身份核对，错误时
才分配证据上下文。总工作仍为 O(草稿字节数)，没有额外全文件散列或读取。

## 显式逐盘新格式

`zxs-helper authority` 是独立新格式的离线存储消费者，提供先审核后确认的文件/备份发布、准确 operation 恢复、只读核验、隔离正文导出和独立 SQLite 重建。它不调用当前池 Engine，也不能转换或接纳运行池磁盘；首次创建必须显式 `--accept-new-format`，写入必须匹配审核摘要并 `--confirm-write`。具体命令、容量与故障范围见[逐盘权威切片](../design/independent-disk-journal.md)。

<a id="independent-content-check"></a>

### 新格式逐版本离线检查

```sh
zxs-helper authority check selection.json
zxs-helper authority export selection.json VERIFIED_COMMIT /srv/export/new-file
```

`selection.json` 显式选择路径、完整原身份和可选已知 checkpoint，不借 `--root` 打开系统池，也不自动判断或兼容其它盘格式。先停止所有相关写者；检查只读本盘已发布元数据与本盘保留 FULL，不打开原一级、下游盘或系统 SQLite，不创建锁文件、不修复或删除材料。

`check` 在完整元数据链、来源和 pin 校验成功后，分别读取各版本正文并核对声明长度及摘要。一份正文缺失或损坏不遮蔽其它版本：JSON 的 `copies` 保留原一级完整身份、history/file、commit、声明 body 摘要及长度，`content.status` 分别为 `verified`、`temporarily_unavailable` 或 `corrupt`。相同正文的不同版本不会合并。可据 verified 项选择另行执行的 `export`，导出仍重新核验，不把先前检查结果当读取许可。

仅无内容问题的报告返回 0；已初始化空盘明确输出 `state:no_published_content`，不声称恢复了文件。缺失或损坏输出 `state:content_issues` 并返回 1。元数据、身份、pin、文件系统安全、I/O 或预算错误不输出成功/部分健康 JSON，保留完整错误链并返回 1。输出本身失败也失败。原 `inspect` 保持原行为，不能用其退出码代替本命令的完整内容检查结果。

报告的 scope 为 `local-retained-published-content`，绑定捕获的本盘 checkpoint/sequence；它不是全盘未知文件/孤儿扫描、未发布操作清单、最新全局状态、三范围数据库一致性或完整系统恢复证明。检查仍受独立格式的对象、深度与 16 MiB 材料读取预算约束；元数据只捕获一次供本轮正文检查使用，逐份正文读完即释放，不为每个版本重新遍历整条证明链。打开选择本身仍执行原有 pin 准入，失败不产生检查报告。

救援专用的检查流程、逐项诊断、输出及退出策略位于 `helper/src/authority/check.rs`；通用协议解析、身份约束与可验证正文读取复用 Engine 的只读 `ContentSnapshot`，既有单版本导出也使用同一读取路径。构建依赖 ZXS 代码，运行不需要启动 ZXS 服务或管理面。此组织不表示完整自含依赖包，也不为已存在的普通池协议增加旧格式双读。

## 未激活部署配置包

`deployment preview` / `deployment prepare` 与独立 Web 现有源码可准备匹配版本的 systemd/Samba 配置，精确摘要确认后仅发布到 `/zxs` 下独立新目录，不覆盖现用配置、不执行系统命令、不启动或启用服务。部署包不是已初始化交接。源码未构建或运行，实际激活仍缺全生命周期停服互斥与完整初始化证明。见[配置准备边界](../design/helper-deployment-preparation.md)。

## 当前 daemon 生命周期互斥与根准备

新增 `host preview` / `host prepare` 和同名 Web 页面，只原子准备缺失的固定 root 所有 `/zxs`。当前 daemon 与打开 Engine 的 helper CLI 使用同一 OS 目录租约，daemon 持有到 Tokio 阻塞池结束。源码未构建/运行，不包含旧二进制、Samba/passdb 或任意第三方写者，完整首次初始化与凭据重置仍未实现。见[准确范围](../design/helper-service-exclusion.md)。

## 服务私有布局前置

`initialization layout-preview/layout-prepare` 及 Web 对应卡片可按准确本机非 root 服务账户，原子准备空 `/zxs/data/{state,management,disks}`。使用当前 daemon 同一生命周期租约，固定目的地、精确摘要确认，只交接新空树所有权，不递归 chown 现用数据。没有创建管理员/数据库/磁盘，仍不是完整首次初始化。源码未构建或运行。见[私有布局边界](../design/helper-private-data-layout.md)。

## 固定 systemd / Samba 文件安装

`deployment install-preview/install-files` 和独立 Web 可核对实际本机服务账户、主组、独立 Samba UID 与已安装准确程序，确认后先持久化原意图，再新建或核验固定 `/etc/systemd/system/zxs.service` 和可选 `/etc/samba/zxs.conf`。不覆盖不同配置，不改默认 smb.conf，不加载/启动/启用服务，不创建密码/账户或系统就绪标记。跨目录失败可能已安装部分文件，保留原请求与证据，只按原请求重试。源码未构建/运行/审查/CI，仍不是完整首次初始化或一条龙安装。见[安装与恢复边界](../design/helper-configuration-file-installation.md)。

## 已存在管理员密码重置

独立 Web 新增精确预览/确认后的固定系统管理员密码重置，持有当前 daemon 生命周期租约与管理凭据锁，验证真实服务身份和原管理权威，原子发布前先保留受保护的原操作。用户手动输入新密码，撤销全部 ZXS 会话；同一原请求可通过重新输入同一新密码核对或续作，绝不以失败响应推断未改密。没有创建管理员/磁盘，没有 Samba passdb 改密或灾备副本同步，没有服务启动。源码未编译/测试/审查/CI，详见[准确边界](../design/helper-administrator-password-reset.md)。

## 首次管理账户阶段

独立 Web 在准确新空私有布局中可显式创建首个 ZXS 管理员，并以原摘要/同一初始密码恢复中断操作。共用当前管理意图、凭据与权威 marker 格式，保留真实 setup/auth/主服务生命周期锁、服务文件所有权和无覆盖发布。状态仅是 ManagementReady，无首批磁盘、数据库或完整系统就绪；不创建 Unix/Samba 账户，不启动服务。源码未编译/测试/审查/CI，见[首次管理账户边界](../design/helper-first-management-administrator.md)。

## 首批空目录存储初始化

独立 Web 可在已完成管理账户后，预览并明确初始化 `/zxs/data/disks` 下全部已存在的空直接子目录。root 编排通过同一二进制的非 root 服务身份工作进程使用现有 activation/Engine 协议，完成前发布凭据灾备快照并核系统及逐盘 current/history SQLite。它不挂载、格式化、创建 Unix 账户或启动服务，不接纳外来盘；完整安装就绪与正常运行最终门禁仍未完成。原计划、生命周期交接、部分失败及未运行验证见[首批存储](../design/helper-first-storage-batch.md)。

## 独立 Web 的本地 Samba 密码重置

手工 root 控制台新增受支持的既有本地 tdbsam 改密，准确匹配安装配置与账户，密码只经 stdin 送给已核验的固定 `smbpasswd`。它不是 `restore-system` 或管理员改密，不创建账户，不启用服务，结果不明只查询原记录、不自动重放。源码未编译/测试/审查/CI，见[支持范围和恢复限制](../design/helper-samba-password-reset.md)。

## 已有池的空目录 Add 纵切

独立 Web 可预览、明确添加并初始化当前 production 池的准确空目录批次，或核对/继续原意图。root 计划与服务 UID worker 在 helper，现有服务和工具共用原生 Add 控制记录、archive/joint/activation 及永久回执。只接受固定布局中完整原根集合和本批新空根，不格式化、不接纳外来盘、不提供计划下线；移除、外来完整链接纳及完整就绪仍未完成。源码未编译/测试/审查/CI，无真实系统操作，见[范围与失败语义](../design/helper-active-disk-addition.md)。

## 非最终元数据成员的注销

本片新增独立 Web 的真实登记读取、准确 Deregister 预览/确认和原操作查询/续作，服务与 helper 共用原生持久阶段、D3 与永久回执。不擦盘、不卸载，也不保证剩余池保有全部访问，要求明确承认当前视图影响。成功后不为查结果重新探测退役盘。最后一个元数据成员、外来完整链接纳及完整就绪仍缺；原安全出口保留。源码未编译/测试/审查/CI，无真实系统操作，见[准确边界](../design/helper-active-disk-deregistration.md)。

## 最后元数据成员的终态注销

独立 Web 的既有注销入口增加最后元数据成员路径，必须单独确认结束活动存储。服务 UID worker 在打开 Engine 前选择受保护的 terminal authority，以独占原生 owner 完成实际 I/O 关闭和原终态发布；冷态按同一原计划核对永久回执，不重新探测退役根，不制造零成员可写池。终态内剩余登记仍可展示，但新的后继注销、metadata resume 和外来完整链接纳未完成。源码未编译/测试/审查/CI，无实际系统操作，见[完整边界](../design/helper-terminal-disk-deregistration.md)。

## 终态剩余登记检查前置

独立 Web 可对终态剩余登记读取准确 authority/registry/绑定边界及直接依赖，下载明确不可执行的诊断报告。没有新 operation 或意图，`executable:false`；剩余成员的正式终态 successor 写者仍缺。该检查不探测目标，不是完整内容/健康或外来接纳证明，已决定原操作的打开恢复仍可能同步。源码未审查/编译/测试/CI，见[边界](../design/helper-terminal-removal-inspection.md)。

## 终态原生未完成决定

核心增加 SYSTEM-only 的精确剩余登记选择/原请求恢复，现阶段没有最终 registry/Stop/D3 或取消能力，**未接 helper 写按钮**。既有 helper 检查可以显示其真实原 ID/摘要/阶段，仍不可执行。Server/Web 明确报告 recovery_required，metadata resume 被围栏；不是剩余成员已移除。源码未审查/编译/测试/CI，见[决定存储](../design/terminal-retirement-decision-storage.md)。
