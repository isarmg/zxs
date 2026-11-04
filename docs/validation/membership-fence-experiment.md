# 成员恢复与首代 fence 实验验证

本页对应独立实验分支，不表示已有池在线增删已经交付，也不授权在真实运行池调用尚无激活/取消后半程的交接入口。协议边界见 [ADR 0013](../adr/0013-generation-bound-membership.md)。

## 已验证

- 原 N3/Q2 的一个锚缺失、损坏或成员身份不符时，隔离其 held resource，恢复合法多数的原完整发布状态；原 N2/Q2 不缩小阈值。坏锚不隐藏重复/外池输入，多个完整配置分别自称多数时拒绝选择。已发布清除在坏锚盘不执行，补回原锚并重开后可以完成。
- 首代 fence 对象先持久保存，再进入原哈希日志；原 Candidate、原 Config、原证据及日志前缀不重写。全部成员同步后同意图重复调用不增加 fence 帧。
- 有 fence 时普通恢复和 preserve-pending 恢复都只重放已完整证明的发布前缀，不补原 PUBLICATION、不修复旧候选、不保存 checkpoint、不进行清除维护。已有 checkpoint 不绕过 fence；业务变更、旧 journal 追加、显式 degraded recovery 和新 scrub 准入均被阻断。
- 在第 1/2 票后中断，以及三个写入位置分别发生 partial-write/sync 故障后，重启按可验证完整帧恢复；原 N/Q 不因在线票减少改变。同 operation/actor/target 可以修复已持有日志的不完整尾部并继续同一 fence。首帧不完整且没有完整 fence 时不认定交接成立。
- 错 actor、冲突 intent、错冻结前缀、缺少可验证 intent、fence 后的旧业务记录均拒绝。拒绝不截断原证据。系统目录丢失后从原盘重建仍保留 fence 和原发布状态；空 genesis 也不能绕过 fence。

核心专项见 [membership_transition_tests.rs](../../test/engine/unit/engine/membership_transition_tests.rs)；锚专项见 [membership_recovery_tests.rs](../../test/engine/unit/engine/config/membership_recovery_tests.rs)。

2026-11-04 本地验证结果：

- fence 专项 8 项通过，覆盖六个 append 故障组合及两种部分票边界。
- storage-engine 单元测试 1059 通过、18 按既有条件忽略；包括原日志编码固定向量。
- storage-engine 全目标严格 Clippy、包级 rustfmt、文档 contract-check、架构检查及 diff whitespace 检查通过。
- 之前的多数恢复修正另外运行了 35 配置、59 恢复、6 清除测试；不以这些数字替代 fence 的新验证。

## 跨构建旧池证据

先由保留的未改格式基线构建创建真实临时目录池，内有一个正文版本和两个永久操作结果，再由当前候选执行 fence 并重开。状态精确相同，两盘原副本均可离线检查和导出完整正文。原有 93 个非日志文件逐字节相同，两份旧日志仅追加且原前缀相同。

随后以同一保留基线构建尝试打开 fenced 池，解析器明确返回 `LogCorrupt`，具体来源为未知 `MEMBERSHIP_FENCE`。拒绝前后全部 99 个盘内文件的 SHA-256 及文件集合相同。该证据表明旧程序不静默忽略新 fence，也未在拒绝时截尾。

基线来自本地保留构建，不是重新下载并认证的公开 1.0.0 发布二进制；不能将这项证据表述成所有历史发行物均已实测。

## 尚未验证或实现

- 没有运行真实设备、真实断电、硬件缓存撒谎、跨主机 writer 或磁盘克隆/回滚测试。
- 未重跑全仓测试、Samba 环境验收、HTTP/Web 或本阶段未改变的完整集成矩阵。临时目录故障注入不等于物理掉电验证。
- 首代 fence 阶段没有旧/新联合决定；后文增量增加控制联合决定，仍没有认证后继配置链、完整退出归档、完整新成员 readiness、数据排空、目标激活、旧盘退休或完整在线操作 API。
- 当前 fence 入口只冻结原业务决定，不授权新集合写入；该入口本身不打开目标目录。恢复仍可能同步已有日志并重建系统投影，不承诺全文件系统零写入。

实验在这些后续阶段完成并单独验收前，不应作为“在线增删磁盘”合入或推送。


## 只读归档预检增量

后续独立分支增加了日志/控制元数据闭包检查，测试入口为 [membership_archive_tests.rs](../../test/engine/unit/engine/membership_archive_tests.rs)。覆盖两次历史发布、原身份日志摘要、无源写入、旧 snapshot/历史图/control_auth/独立 Finalize 损坏、健康副本读取而不修复、日志截断、未齐全 fence、同序号不同 Candidate 及阶段对象错绑。

本增量的最终源码通过 6 项归档专项、59 项恢复/检查点回归、82 项日志回归，以及严格全目标 Clippy、包级格式、契约、架构和 whitespace 检查。未再次运行整个 engine 或全仓矩阵。

该只读预检提交本身未写出持久归档；下节为后续独立控制归档增量。仍没有从目标盘独立恢复整个旧池的验收。完整本盘证明、墓碑、正文、工作区与目标逐盘检查仍待实现。其时间复杂度是各候选图大小之和，不是唯一对象数的线性遍历。

## 独立持久控制归档增量

后续实验增加 `build_membership_control_archive` 与必须接收可信预期根/过渡摘要的 `verify_membership_control_archive`。专项入口为 [membership_control_archive_tests.rs](../../test/engine/unit/engine/membership_control_archive_tests.rs)。原日志、配置锚与盘 seal 原始字节位于按原 disk/slot/binding 分隔的历史命名空间，绝不进入目标活动日志。归档完成只证明所列控制闭包，不证明版本本盘 proof、protection-proofs、墓碑、正文、workspace 或目标 readiness。

验证使用临时目录，包含两次历史发布后移走全部原盘目录，单独验证归档；原 Candidate/Publication/PublicationSeal、日志、配置和 seal 字节保留；缺旧页、独立 Finalize、history/control_auth/seal、错误日志前缀、截短日志、修改原身份及错误预期根/过渡均拒绝。还覆盖同绑定重试、三个完成前中断边界、真实目录 I/O 冲突、外来目录/符号链接/未确认初始 marker 材料的保留拒绝，以及分页清单、空 genesis、REPROTECT 恢复计划、重排成员后的同绑定重试。另用可解析的截短日志及重算摘要确认自生成清单不能替代原已接纳 fence Receipt。原盘或系统根被重命名，再把目的放进已移动根下（包括旧名已有替代目录）时，在首次归档写入前拒绝。

初始 marker 未完整建立却留未知临时材料时不会自动删除或接管；需外部核对，不能说全失败边界都可自动重试。完成标记只在归档自身完整验证之后原子发布；同步报错时不报告成功。当前停写持续时间取决于全部历史检查和复制及 I/O 恢复，没有短暂停上界；预复制与末尾冻结增量复核尚未实现。

2026-11-04 本增量最终源码的 `membership_` 过滤测试 62/62 通过，其中新增持久控制归档 13 项、既有只读归档 6 项、fence 8 项；其余为匹配该过滤词的既有成员相关回归。storage-engine 全目标严格 Clippy、包级 rustfmt、文档 contract-check（2840 个本地链接）、架构与 whitespace 检查通过。没有把之前的 59 恢复/82 日志或全 engine 结果冒充本次重跑；未执行全仓矩阵、真实设备/断电、部署、HTTP/Web 或目标激活验收。准入时 named/held 身份复验不证明准入后并发目录重挂接、祖先 bind-alias 或硬件故障域隔离。

## Pending 入籍与逐目标 control bootstrap 增量

独立实验增加 `enroll_pending_members`、`resume_pending_members` 和 `pending_membership_status`。只支持原全部配置字段不变、全新 disk/slot/generation-one 元数据成员的纯扩容；持久 fence 中不受此阶段支持的目标保持原样并继续停写。新 `membership-pending` seal 不由旧普通 decoder 双读；原成员的 seal/config/application-log 不改，所有目标自己的控制材料和单帧日志都在独立 pending 命名空间。它仍不是 readiness、联合决定、新代活动恢复或业务激活。

端到端专项见 [membership_pending_tests.rs](../../test/engine/unit/engine/membership_pending_tests.rs)，日志 namespace 专项见 [pending_tests.rs](../../test/engine/unit/journal/pending_tests.rs)。临时目录覆盖：

- 逐目标不同 bootstrap 摘要和真实同步回执、四份独立完整控制归档、全部原文件字节保持、同意图重试不重复日志；新资源不进入普通 disk_resources/journals。
- 删除、旧根/binding/角色/capacity、全局设置/mode、非一代或非元数据新增盘等不支持目标在任何目标写前拒绝；held 名称消失/替换、重复 held 根、同一个 `.writer.lock` 已被持有、外部预期归档根错误均拒绝。
- A 的 pending seal、member marker、bootstrap 对象或日志复制到 B 均拒绝且保留；pending/active 日志混放、第二帧、完整帧后的短尾与错误对象绑定均拒绝。
- 本目标归档缺 snapshot 页时独立验证失败，其他目标的健康页不能补票；明确同意图续作可从可信来源补回同哈希，来源也缺页则失败。
- 第一个和第二个新增 seal 完成后的故障分别检查。同进程保留已准入 held 根和锁可继续；重启时未绑定的锁材料拒绝自动接管，全部新增 seal 已成立则可显式恢复。破损 seal/member/object/archive 均保留，不覆盖成预期内容。
- member/archive/object 分阶段失败、首个目标回执之后失败、pending journal 的 partial-write/sync 错误均不报告虚假全体完成。重开验证后恢复同一首帧；文件及目录 opening sync 故障另由日志专项覆盖。
- 在 member marker 或 archive 完成处受控暂停，换名整个 transition 子目录或其 control-archive 后继续：held 目录链与当前约定名称不符，不能发成功回执；不把已移动归档冒充当前位置的闭包。
- 普通重开保持原 fence，不隐式启用 pending；系统目录丢失先从原盘恢复同一已发布状态和 fence，再显式恢复 control 进度。仅以新 pending 根恢复系统明确拒绝。

该页只描述实际临时目录与故障注入范围；未运行真实盘、物理断电、跨主机 writer、祖先 bind-alias 或任意并发挂载/重挂接。尚无 HTTP/Web、联合决定、完整证明/正文/workspace 搬迁、激活或退役。停写成本仍包含原历史和来源复验，以及每目标的来源图复制遍历和本地独立闭包检查，不承诺短暂停或线性成本。

保留的旧本地构建 `baseline-probe` 另对当前 pending fixture 实测：初始化返回 `DiskAlreadyEnrolled`；仅以该 pending 根配置启动、以及调用旧 `restore_from_disks` 均返回 `DiskIdentityMismatch`，旧 strict seal decoder 在 `control_archive_root` 未知字段处拒绝，未到业务状态读取。目标全部 14 个文件的路径集合和 SHA-256 在三次拒绝前后相同。该 binary 是本地保留旧构建，不是重新下载认证的官方发行物，也不是全部历史版本兼容性矩阵。

故障证据须分层理解：阶段完成后的 failpoint、真实普通文件占据目录名的 I/O 冲突、破损内容保留拒绝，以及 Journal 的短写/文件同步/目录同步注入是不同测试。复用的不可变原子写与 fsync 语义继续保持，但本增量没有逐一执行所有 seal/member/object/archive 屏障的完整断电矩阵。

2026-11-04 本增量最终验证：`membership_` 79 项通过（其中新增 pending 17 项；一个保留临时目录的 cross-build fixture 默认忽略，并已单独显式运行通过），`journal::` 89 项通过（含 7 项新 namespace 测试），`recovery` 119 项通过。这些过滤集合有交叉，不相加作为独立测试总数。无 dev/test 目标的 storage-engine library `cargo check`、全目标严格 Clippy、包级 rustfmt、契约 2842 本地链接、架构及 whitespace 检查通过。首轮一个删除目标测试夹具自身拓扑非法、一次 Clippy 大枚举提示均已修正并重新验证；未运行全 engine/全仓、HTTP/Web、SMB、真实设备、部署或物理断电矩阵。


## 纯扩容持久联合决定增量

独立阶段的实际 Core 入口为 `resume_membership_joint_decision`、`verify_membership_joint_decision`、`membership_joint_decision_status`。协议与成本见 [ADR0013 联合决定](../adr/0013-generation-bound-membership.md#pending-joint-decision)。专项为 [membership_joint_tests.rs](../../test/engine/unit/engine/membership_joint_tests.rs)，三 namespace 测试继续使用 [pending_tests.rs](../../test/engine/unit/journal/pending_tests.rs)。

临时目录测试包括正常 2→4、3→5；3 个旧成员加 3 个新成员时，新 3 票加旧 1 票达到目标 Q=4，但达不到原 Q=2；原 Q 达到而目标 Q 未到；双 Q 达到但未齐全票或未安装任何证书。每目标的 Acceptance/帧均不同，旧全部文件字节、Config/State 和普通资源集合保持，业务全程 blocked。

逐四目标测试 binding/依赖阶段中断、真实普通文件占据 objects 目录、joint 首帧短写、append sync、opening file/directory sync，以及完成根可见后的额外确认 file/directory sync 注入。错误不计虚假同步票或完成成员，重开原 Engine、显式 pending 后仅续原 proposal。确认同步注入发生在复用原子写原语之后的独立完成确认屏障，不冒称逐个原子 rename/所有底层同步点均完成断电注入。

同绑定不同 proposal、无绑定未知材料、外来成员帧、第二帧、完整帧后短尾、重复/外来/改 slot/generation/acceptance/frame hash 的证书票均拒绝保留。某目标证书缺页时 verify 失败，健康目标不能补票；显式同 proposal resume 可重建准确哈希页并独立复验。控制日志损坏后旧成功 status 清除，不根据旧观察报告完成。系统目录丢失从原盘恢复原 fence/State，再显式恢复 pending/joint，proposal 不变。

在依赖或完成屏障受控暂停并换名 member、control-archive 或 joint 子目录，不产生成功票/完成成员，替代空目录不被当作持有身份；较早成员完成后再被换名，末尾全体 held/named 复验同样拒绝全局成功。另覆盖 singleton 日志在只读打开后被替换，拒绝为已移走文件发当前名称的回执。257 个合成条目的独立布局测试证明分页分裂，不把这些条目当成真实成员票。此证据不证明任意特权进程对所有名称/内容的并发替换、祖先 bind-alias、并发挂载/重挂接、跨主机写者、物理故障域或真实掉电。

本阶段仍未激活新集合，不提供已有池完整在线添加/管理/删除。HTTP/Web、正文/本盘证明/workspace 排空、后继代际、只凭目标启动和完整操作结果均未执行或实现。最终执行命令、数量与环境另记验证归档，不把历史通过数当作本轮重跑。

2026-11-04 最终源码验证：`membership_` 95 项通过、1 个既有导出夹具默认忽略，其中本阶段新增 joint 专项 15 项；`journal::` 91 项通过；`recovery` 119 项通过。过滤集合存在交叉，不相加作唯一测试数。production library check、全目标严格 Clippy、包级 rustfmt、架构、2845 个本地文档链接与 whitespace 检查通过。两个早期测试编译夹具错误及一次缺显式文档锚已修复；以上为修复后结果。仍未跑全 engine/全仓、HTTP/Web/SMB、真实设备、部署、物理掉电或全逐屏障故障矩阵。
