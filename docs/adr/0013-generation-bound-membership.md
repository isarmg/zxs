# 0013：成员配置代际、历史证明与在线交接

状态：固定成员身份与恢复锚已接入；独立实验分支实现首个纯新增元数据成员的 fence、控制归档、pending/joint、代际激活及恢复闭环。首次批次已在 GitHub 定向验证，尚未合入或发布；重复纯添加的后继批次已通过 GitHub 定向验证，非投票与混合新增源码另批待验。另有 old-only 退出控制准备源码待验，尚无真实下线或注销激活，不包含排空及增删 HTTP/Web。下文早期阶段的完成条件只说明各自边界，不等于激活完成。

## 固定成员基础边界

保留已经发布的 Candidate、回执、日志及本盘证明原字节和哈希。新代际决定使用下面的严格新信封，活动盘封印显式绑定该代际；原封印仍作为不可变归档证据保存。不要求旧程序理解新材料，不维护旧程序兼容或旧二进制测试。读取用户现存历史按原始权威验证，不属于重新解释、升级或重置旧数据。

`ValidatedConfig` 在配置边界缓存固定成员身份：准确 pool_id，以及按 disk_id 排序的 slot_id/binding_generation 映射。正常/恢复/离线路径统一检查 Candidate 的池身份、准确成员集合及无重复；读取本盘证明时仍核验原配置锚。路径重绑定、成员输入顺序和资源限额不改变成员身份。

恢复读取任何元数据成员日志前，先通过持有目录 FD 读取并核验该成员的原始配置锚；即使日志为空，也不能通过编辑系统 `config.json` 改变已初始化池的成员。锚缺失、损坏或成员身份不匹配时，保留具体失败及 disk/slot 身份，不扫描该成员日志、不生成替代锚。仅按原配置的固定 N/Q 统计其余锚与日志都通过的成员；不足原多数则带原失败原因返回，不按在线数降低门槛。

原多数可用时允许恢复已完整证明的发布状态。被拒成员从本次磁盘资源集合隔离，原配置身份、原多数及持有 writer 锁保持不变，后续复制、清除和工作区处理不能重新打开该路径写入。普通业务写仍按原全成员规则阻断；这不代表整个文件系统只读，健康成员的已发布清理以及系统投影可沿原协议继续。补回原锚后必须显式重新打开，不能让旧恢复对象自动重新准入。

系统盘丢失恢复先记录并去重所有输入 seal/路径，再剔除不能核验的锚票。按完整保存配置分别统计原定多数，不合并不同配置的票；多个配置各自满足多数、外池/非成员输入或重复盘均拒绝。最终仍将所有输入 seal 对选定配置验证，坏锚不能把被较小配置排除的盘从输入证据中隐藏。未发布候选在成员不足时保持原未决语义，不补交、不合成中止。

该阶段无新持久格式或写入屏障。配置准入建立 O(M log M)、空间 O(M) 的成员映射；固定候选核验仍为 O(M log M)。恢复增加每个可读元数据成员的一次有界配置锚读取；发生隔离时额外建立 O(D) 的浅层句柄集合，不增加 fsync，不跳过原日志/对象/目录屏障。它只封闭已有池恢复准入，不实现动态成员切换。

## 首代 fence 实验边界（不作为完整功能合入）

实验核心入口 `begin_membership_handoff` 只停止原集合的新业务决定，不能激活目标集合、退役旧盘或取消交接。没有 HTTP/Web 接线，不应向运行池暴露这个尚无完整交接后半程的入口。目标 Config 只解析和绑定，目标路径不会被打开、初始化、挂载或清空。

独立不可变 `membership-transition` 对象绑定永久 operation_id、actor、准确原 Config、目标 Config、原已发布 Candidate/快照/序号，以及下一 fence 槽位。当前只接纳显式 Genesis 来源；没有把任意旧配置当作后续代际或提供缺省前驱。旧 Candidate、旧回执、原对象哈希和原日志前缀不改写。

对象闭包先在全部原成员持久保存，再逐盘追加新的 `MEMBERSHIP_FENCE` 帧。完整帧进入原哈希日志链；当前活动日志解析器显式识别该控制帧，不把它当作尾部截断或忽略的 sidecar。当前进程在首份帧前即停止业务写并使旧 scrub/准备凭据失效；零确认票及短写仍只能报告“已开始或可能持久化”，不能称旧多数 fence 已成立。查询区分观察并同步的票数、原固定 N/Q，以及是否齐全；观察票减少不降低原多数。

启动先从原成员完整日志接纳同一 intent 和 fence，再验证准确完整发布前缀，然后才转换持有日志句柄。存在合法 fence 时，普通恢复保持已发布读状态，禁用旧候选修复、PUBLICATION 追加、checkpoint 读写与清除维护。普通追加封装和显式 degraded recovery 也拒绝推进旧决定。日志同步、工作区内存对账及系统索引重建仍可运行；这不是整个文件系统零写入承诺。

部分失败没有回滚声明。相同 operation/actor/目标可继续补齐原全部成员；不同请求拒绝。对已观察到完整 fence 的交接，显式同意图续写可以修复所持日志的不完整尾部，只追加同一 fence，不进入普通候选恢复。若首份帧本身不完整且没有任何完整 fence，重启不能据此承认交接成立。缺 intent、错哈希/前缀、分叉 intent、重复成员 fence 或 fence 之后出现旧业务记录均在恢复写入前拒绝。

该首代 fence 入口本身仍没有联合配置决定、完整退出归档、完整新成员 readiness、目标激活、后继过渡链、排空计划或完整操作回执 API。Q_old fence 只证明原多数受到隔离，不证明新成员可写。最终完成前不得合入或推送为在线增删能力。

同步预算：每次新交接/续写先保留现有 immutable-object 原子写及目录屏障，再为每个尚未追加的原成员写一个完整日志帧并同步；相同完整帧复用原回执，不再追加。原边界捕获扫描一个原成员日志，恢复仍检查完整原多数日志；不能用此实验宣称启动或吞吐性能提升。

## 只读归档检查前置

实验 `inspect_membership_metadata_archive` 只在原全部 fence 齐全、持有原成员资源且日志不需修复时运行。它按原盘身份流式解析每份完整日志，核对实际末帧等于本次已接纳的原 fence，记录准确字节数、内容摘要与原 slot/binding。历史成员选择同时匹配已认证链内的准确 Candidate 哈希和序号，并继续调用原固定成员校验；不能仅凭小于冻结序号就接受另一候选。

检查遍历日志引用的 Candidate、原 snapshot 的 maps/events/control_auth、history_roots 目录与对应全部历史图，按同一原 State 核对历史投影。PREPARE 必须指向该 Candidate 的 snapshot，ACCEPT 指向 Candidate 自身；独立 FINALIZE 核对原成员 ACCEPT，REPROTECT 复用原恢复计划验证。每份 PUBLICATION 的完整 seal 与实际原盘帧绑定。读取允许从合法副本选择同一哈希对象，但不会修复源副本。

输出仅为来源检查结果，不是持久 ArchiveRoot、归档安装证明或逐目标 readiness。版本本盘证明、protection-proofs、清除墓碑、正文与保留工作区尚不在这个入口覆盖范围。旧 journal 不能复制到目标的活动日志；旧本盘证明也不能当作目标盘自己的提交证明。

复用的 snapshot closure visitor 与原恢复修复共用完整遍历，但原修复同步语义保持：仅 state-page/metadata-index 使用原去重缓存，snapshot root/control_auth 仍逐次修复。检查本身不写入任何来源文件。

成本边界：日志流式读取；每个不同 Candidate 仍需重建一个 State、验证其完整历史投影并遍历 snapshot。共享对象去重仅用于统计和同一 Candidate 重用，不保证对不同历史版本共享图只读取一次。总工作量是各候选图大小之和，历史累计增长时可能接近二次量级；峰值内存含一个 State/历史图及唯一对象地址集合。该只读预检尚未做大规模性能验收，不能宣传为线性归档或作为热路径执行。

## 独立持久控制归档实验

`Engine::build_membership_control_archive` 仅在原全部成员已接纳同一 intent 的完整 fence、原 seal/配置锚仍有效且日志无需修复时开始。调用方显式提供现有私有空目录；拒绝与配置中的原盘、目标盘和系统目录词法重叠；另复验原盘 named/held 根身份与系统 writer-lock 身份，拒绝已移位原根，以及归档根和已持有原根的直接 inode 别名。这些只证明准入时身份一致，不证明准入后的并发目录重挂接隔离，也不证明任意祖先/子目录 bind-alias 的完整物理隔离；目标路径未准入且不会为检查被打开，部署方仍须提供独立的目标目录。归档根的 `frozen-control-metadata` 格式及独立 `membership-control-archive` 哈希域不修改原 Candidate、Config、回执或日志编码。

历史日志保存在 `journals/<原 disk_id>/<原 slot_id>/<原 binding_generation>/journal.bin`，从 genesis 到之前已接纳的各自 fence 原字节完整复制，并保存原始 `disk.json` 与配置锚字节。seal 的 device/inode 只是历史记录，不把归档目录伪造成旧物理盘；无目标活动日志、无新盘 Receipt。目录与文件操作使用受限 held FD、独立流式字节复制、不可变原子写、文件和目录同步，不使用硬链接或 reflink。

归档包括每个日志引用的 Candidate、独立 Finalize、Publication、完整 PublicationSeal、snapshot maps/events/control_auth、history_roots 与全部 history-manifest 图、REPROTECT recovery-plan 和 membership intent。成员清单与发布 seal 清单复用 `metadata_map` 分页；每页与固定根均受 `MAX_METADATA` 限制，不把随历史增长的列表塞入单个根对象。控制对象沿原 kind/hash 保存原格式的 canonical 编码，不承诺保留外部写入的非 canonical JSON 排版；原 journal/config/seal 则保存实际原始文件字节。封装发布 seal 使用单独域，不改写其中原回执编码。

完成根 `archive-root.json` 最后发布：先只用归档自身的 loader 重放全部原日志、核对原 fence Receipt 与精确发布前缀、接纳各 snapshot 并核对同 State 的全部历史投影。这个验证不传原盘资源，不允许健康源副本掩盖归档缺页。独立入口 `verify_membership_control_archive(path, expected_root, expected_transition)` 要求调用方可信持有的预期根和过渡摘要；自声明的目录绑定文件不能替代这个信任输入。原盘不可用时仍可验证，验证不写源或归档。

部分失败保留已写内容和来源，原字节冲突拒绝覆盖。同绑定可重新检查并补齐，完成根同步失败不得报告成功，也不能声称已回滚。没有完整绑定 marker 的非空目录（包括初次 marker 写入可能留下的未知临时文件）保留且拒绝自动接管；不承诺所有初始失败都能无人工处理续作。完整同绑定已存在时，保留并忽略经受限 FD 确认的单链接普通原子暂存文件，绝不把暂存内容当作权威或自动清理；同意图仍可补齐。已存在且匹配的对象/日志在重试时重新验证并完成同步。

此归档仍不包含版本本盘 proof、protection-proofs、清除墓碑、正文或保留工作区。因此不是完整退出归档、新成员 bootstrap/readiness、联合决定或激活证书，完成不赋予任何新写权，也不允许退役旧盘。尚无 HTTP/Web 接线。

成本：构建进行来源预检、来源复制检查和归档独立检查；对象写入在一次构建内按地址去重，但每个不同 Candidate 的完整图仍须读取、接纳与验证。总工作量仍为各候选图大小之和，累计历史可能接近二次。清单编码额外占用成员与发布数量的内存；峰值还包含一个 State/历史图及唯一对象地址集合。每个新对象/原身份文件保留原子文件与目录屏障，每份日志保留完整流式复制与持久化屏障，末尾根另有原子写与目录同步；不宣称吞吐、停写时间或线性复杂度提升。

## 纯新增成员的 pending 控制 bootstrap 实验

`Engine::enroll_pending_members(source_archive, expected_archive_root, directories)` 只接纳当前原 fence 的纯扩容目标。按 disk_id 比较原全部 DiskConfig 的每个字段，保留 pool/mode/全部全局设置；新增成员须使用全新 disk_id/slot_id、binding_generation = 1 且为 metadata_member。删除、旧根/槽/角色/路由/限额改变、同槽替换、退役后再入籍及后继代际都不在此入口内。较宽松的已有 fence intent 不会被改写；目标不受支持时不打开或写目标目录，原业务仍停写。

调用方为每个新增成员提供已存在私有目录的 held File 和 disk_id。入口核对数量、准确成员集合、named/held 身份、直接 inode 别名及配置路径重叠；不创建目标根、不改权限、不挂载、不清空。所有新根先准入，再逐盘取得与普通 Engine 相同的 `.writer.lock`；系统根锁仍叫 `writer.lock`。旧成员使用原 Engine 的 held resource 和锁，不重新 flock。新资源和锁只留在独立 pending 集合，不进入普通 disk_resources/journals。

全部准入成功后，新根先原子写独立 `disk.json`：严格 `membership-pending` 格式绑定本盘物理 device/inode、pool/disk/slot/generation、transition、target Config 摘要及可信 control archive root。旧成员根 seal/config/application-log 不改写。没有向普通 DiskSealFormat 增加双读或默认字段；旧初始化看到 disk.json 就拒绝，普通活动 seal 解码拒绝 pending 格式。不会用普通 genesis seal 掩饰尚未激活的新盘。

每个目标 metadata 成员，包括保留成员，都拥有 `metadata/membership-pending/<transition_hash>/member.json`、`control-archive/`、`objects/member-control-bootstrap/<hash>.json` 和 `journal.bin`。成员 marker 区分保留原 seal 与新增 pending seal，并绑定准确物理身份。bootstrap 明确 `control-metadata-only`，绑定本成员、入籍摘要、过渡/目标配置/归档根及准确冻结边界；空 genesis 使用显式边界变体，没有缺省 joint 字段或新写权。

来源归档先以外部可信预期根验证，并与 Engine 当前原 fence 的实际完整回执尾和准确历史检查结果比较。内部消费者持有来源 ArchiveStore，将原身份 journal/config/seal 和原 kind/hash 对象独立复制到每个目标；清单沿原分页对象复制。每个目标最后只用自己的 loader 验证整个控制闭包，再发布本目标 archive-root。健康原盘或另一个目标不能为缺页验证补票；同意图续作可从持有、复验过的独立来源归档补回缺失的同哈希对象，不覆盖冲突字节。

pending Journal 使用现有有界 frame/parser、OpenJournal 和 JournalWriter 持久原语，但其 namespace 只接纳 `MEMBERSHIP_CONTROL_BOOTSTRAP` 一种记录。原活动 reader/writer 拒绝该类型；pending 拒绝全部业务类型。记录沿 intent 的 fence_sequence，candidate_hash/object_hash 都是本 bootstrap 对象摘要，首 previous hash 为零，最多一帧。它从不 rename 为新代活动日志。仅在本成员 append 和同步成功后保存真实 Receipt，公开状态只投影本次已核验同步的 control 进度；一个成员成功、另一成员失败不代表全体成功，更不代表业务激活。

同进程调用 `resume_pending_members` 使用已持有资源和原可信根，重新核对当前原 fence、成员身份和全部材料；每次重开 pending journal，不复用 I/O 状态不明的 writer。首帧不完整时，只有完整控制闭包验证之后才允许截到零并补写同一帧。已经完整的帧重同步文件及目录后复用；第二完整帧、首完整帧后的任何字节（包括短尾）、错对象或其他成员日志都拒绝且保留。完成回执前后还核对 disk→pending member→archive 的 held/named 目录身份；member/archive 的 held FD 保留至回执，避免复制到已被换名的子目录后为另一位置报告成功。

普通 Engine 重开不会隐藏启用 pending。系统目录丢失时仍先由原成员恢复原 fenced Engine，再用外部可信 root 和 held 新根显式重新入籍/续作；只凭新 pending 盘重建不支持。初次 pending seal 未完整成立时，遗留 lock 或未知原子暂存材料没有足够持久归属，重启会保留并拒绝自动接管；不能承诺所有初始失败均自动恢复。同进程仍持有此次已准入根/锁时可继续没有未知材料的首次 seal。全部新 seal 已成立、对应 member/archive 绑定 marker 也完整建立后的分阶段失败可以按同意图核验续作；任一初始 marker 不完整而留下未知材料仍拒绝自动接管，损坏或冲突材料不会被自动修复为另一内容。

范围仍只有控制元数据，不含版本本盘 proof/protection-proofs、清除墓碑、正文、保留工作区、旧/新联合决定、完整 readiness、新代活动日志恢复、激活证书、数据排空、旧盘退休、HTTP/Web 或操作完成回执。原业务持续 blocked，原格式和原哈希不变；不是在线增删已交付，不得作为完整能力合入或推送。

成本：入籍比较为 O((D_old + D_target) log D_target)，新根/锁/状态按成员数增长。当前每次续作再次核对来源与原历史，并逐目标复制和独立检查归档；时间仍受各候选完整历史图之和乘目标成员数量影响，可能接近历史二次量级，不声称线性或短暂停。每个新 seal、member marker、控制对象与归档完成根保留原子文件及目录同步；归档日志完整字节复制保留独立持久屏障。pending 首次日志创建为文件+目录同步再追加文件同步；完整帧复用为文件+目录同步；首帧半尾修复再增加截尾文件同步。没有削弱原发布或原归档屏障。

这些临时目录和受控换名测试不证明任意祖先 bind-alias、并发挂载/重挂接、恶意特权进程替换所有名称、跨主机 writer、硬件故障域独立或物理掉电。普通 0700、inode 和 flock 不承担这些额外证明。

<a id="pending-joint-decision"></a>

## 纯扩容持久联合决定实验

此阶段继续使用同一已准入 pending 集合和原全部成员 fence。`resume_membership_joint_decision` 从当前真实控制材料构造唯一 proposal，不接受调用方自报票，不新建过渡 intent，不改原 Config、State、Candidate、旧回执、原日志或永久 operation 结果。正常完成仅指本节的控制联合决定全部安装，原业务仍 blocked；不得称为在线添加已完成或用它退役原盘。

每次调用先清除旧观察，重验外部可信源归档与当前原 fence，再通过 held 目录独立核验全部目标的 seal、member enrollment、控制 bootstrap、单帧控制日志及本目标控制归档。控制日志必须已完整；缺控制页或首帧不完整须先显式 `resume_pending_members`，联合入口不替它制造控制准备成功。来源或任何目标控制复验失败时没有本次 joint proposal/票观察。

proposal 使用独立 `membership-joint-proposal` 哈希域，绑定 transition、原/目标准确 Config 摘要、operation/actor、冻结 Candidate/snapshot/sequence、fence 序号、可信归档根。原成员 identity/fence 清单、每目标完整 enrollment/bootstrap/真实 control Receipt/原或 pending seal 采用 `metadata_map` 分页；根仅保留有界引用，不把全部成员塞进单个大根。所有 proposal 依赖在首票前完成编码与单对象上限检查。联合票只有固定身份、摘要和固定帧回执，分页证书沿相同有界编码器建立；没有以成员数增长的大数组根。

每目标独立持久位置为 `metadata/membership-pending/<transition>/joint/`。固定 `proposal-binding.json` 先绑定准确本盘 physical identity 与 proposal 摘要，随后安装依赖、只从该目录重新加载并验证。非空但无完整 binding 的目录（含初始原子暂存残留）拒绝自动接管；已绑定位置不能换 actor、target、archive、member 或 proposal。完整同绑定的原子暂存普通文件保留，不作为证据或自动清理。

joint `journal.bin` 复用有界 frame/parser 和真实 file/directory 同步，但 namespace 仅接纳 `MEMBERSHIP_JOINT_ACCEPT`。原 active、control bootstrap、joint 三 namespace 按记录种类穷尽区分。每成员 Acceptance 对象绑定同一 proposal、本盘 disk/slot/generation/device/inode、enrollment 摘要、bootstrap 和真实 control Receipt；帧的 candidate_hash 为共同 proposal，object_hash 为本成员 Acceptance。Receipt 外层 disk/slot 不在原 frame hash 内，因此不能只用共同 proposal 作为 object_hash，也不能复制另一成员帧后改写 Receipt 身份。

每成员最多一个同绑定完整帧。已完整帧重开并完成 file+parent 同步后复用；任何第二帧或完整帧后短尾均拒绝保留。首帧未完整时，只在同一已绑定 proposal 和完整控制/依赖重新验证后，显式 resume 才可截到零并补原票。I/O 不确定 writer 不复用。

原和目标多数分别由原固定 N/Q 与目标固定 N/Q 决定，按 disk/slot/generation 的准确身份集合计数。交叠成员的一份合法本盘票可以分别计入两个集合；new-only 票不能补原 Q。`membership_joint_decision_status` 是最近显式尝试观察的投影，分别给出原/目标同步票数和 Q、全部真实票及完成证书已同步的具体成员。双 Q 不等于全部 target 票，也不等于全部证书安装。调用方不能把公开可变 status/布尔值当作后续激活权威。

正常本节完成要求原全部 fence、全部目标控制闭包、全部目标 joint 票和全部目标完成根。证书使用确定排序的全部 target 票，避免同 proposal 的不同多数子集生成不同 root。每目标先写分页票清单和独立 `membership-joint-certificate` 对象，随后只用本目标 loader 完整核对 proposal/intent/配置/成员依赖、所有票、本目标实际 joint 单帧以及本目标 control archive/bootstrap。最后发布并重同步 `certificate.json` 及父目录，名称身份再核对后才计入完成成员。任何局部失败保留材料，不声称回滚或全体完成。

`verify_membership_joint_decision` 不补对象、不追加或截断日志。它独立读取每目标闭包，重同步实际 control/joint 日志和完成根/父目录；依赖对象本身读取认证，持久顺序依赖原票/完成根之前已成功执行的对象屏障，而非声称此次重新 fsync 所有页。`resume_membership_joint_decision` 则对同 proposal 依赖执行不可变写/现有对象重同步，可从当前全部实际控制和 joint 票重建缺失的同哈希 joint 页，再独立验证。控制归档缺页仍需先显式 pending resume。verify 不会从健康目标或源归档补齐缺页后冒充独立成功。

系统目录丢失时，仍须原成员恢复原 fenced Engine，再携同一外部可信 archive root 与 held 新根显式恢复 pending，最后 resume/verify 原 joint proposal。这只是 joint 阶段的恢复方式；完成下一节激活之后由固定目标多数恢复，不再要求原多数在线。joint 本身不迁移正文或工作区、不赋予普通写权，也不完成 operation；目标活动发布才产生永久业务结果。

成本：一次调用重新检查来源/原历史，并在控制准入与证书阶段逐目标独立遍历控制归档；历史累计成本仍是各 Candidate 图之和乘目标数，可能接近历史二次。proposal/票/观察与 held FD 内存按成员增长，每目标完整保存成员依赖和全部票，因此全体 joint 元数据空间按成员数近似二次；无性能改善或短暂停承诺。每新增 binding/依赖/页/根保留不可变原子文件和目录屏障；已存在对象在 install/resume 中重同步。每 joint 首帧保留 opening file+parent、append file 同步；完成确认另有 file+parent 同步。没有削弱原发布或旧归档屏障。

<a id="first-pure-add-activation"></a>

## 首个纯新增代际的激活实验

核心入口 `Engine::activate_pending_membership` 消费当前真实完成的全部目标 joint 证书。仅支持 Genesis 来源，原全部磁盘的角色、路由、绑定、设置与数据放置不变，新增根只成为元数据成员。它不复制正文、不增加任何版本的副本数，不声称已排空或完成任意增删。激活后再次 `begin_membership_handoff` 明确拒绝；新代日志看到更晚 fence 同样阻断，不能把后继代际重新标成 Genesis。

### 权威与原始历史

`membership-generation` 权威绑定准确 joint proposal/certificate 全部依赖和冻结 ArchiveRoot。可搬运图验证与实际目标准入分离：离线证明只验证有界不可变对象图；活动准入还核对本目标物理身份、真实 joint 日志、本目标完整控制归档及固定目标多数。不会因为传入 data-only 根就跳过本盘票验证并宣称成员已准入。

新 Candidate 是严格 `membership-generation-candidate` 信封，显式包含 `authority_hash` 和原决定内容；所有未知字段、错误格式或缺字段拒绝。原 Candidate 规范字节和哈希完全不变。运行时按“归档中的准确 sequence + candidate hash”解析原候选，按新 authority hash 与固定目标成员解析新候选；仅比较序号小于 fence 不足以授权历史。目标配置中的路径是被认证的原始地址，恢复后的实际路径只作为资源投影，不能重新 hash 成另一权威。

业务结果中的 `membership_epoch` 从已准入成员权威投影：Genesis 为 `1`，本阶段唯一支持的首个后继为 `2`，不借用可独立变化的 `topology_generation`。激活前永久结果保持原值，不回写为当前代际。

权威图保留 joint 原分页依赖，但本阶段的便携容器将这些成员控制对象合并为单份有界 JSON。激活前以 `MAX_METADATA`（64 MiB）检查完整编码，超限明确拒绝；不在新版本的每份正文证明内重复嵌入整图。这个聚合上限限制可激活的成员规模，不宣称任意大小成员集合可用。冻结业务 State/全部历史对象不嵌入便携权威或历史包。

### 持久顺序

1. 继续持有原根、原 writer 锁和 pending 新根锁；验证所有实际目标 joint 证书及归档。先计算固定 activation 结果、checked 拓扑递增和权威编码预算。
2. 每个保留或新增根发布相同 prepared generation marker。marker 不等于活动 seal，不允许普通 open 接管 pending 根。所有目标元数据盘从各自控制归档安装原始控制对象；data-only 盘只安装便携权威和原 Candidate 在冻结发布清单中的路径证明，不复制全池 State。
3. 显式 promotion 核对旧 active 或准确 pending 前驱 seal、held/named 身份，再原子写严格 `MembershipGeneration` seal。新日志为 `metadata/membership-generations/<authority_hash>/journal.bin`；原日志、pending 和 joint 日志都不被改名或当成新活动日志。
4. 新日志从认证的冻结 State 启动；最后一条若为合法 degraded Publication，按原恢复规则保留其永久 operation 结果覆盖层，不把 prepared snapshot 中的原结果当作最终结果。第一条普通决定只能是预先计算的 activation Candidate：序号 fence + 1、准确旧 snapshot、确定 transaction_id、新 topology_generation 与永久 operation 结果都固定。普通目标全成员 PREPARE/ACCEPT/FINALIZE/PUBLICATION 和完整 seal 成立后才恢复业务写入。
5. 系统 `config.json` 最后写入，仍只是可重建投影。投影失败、响应丢失或部分 publication 不能回退冻结旧 State；重开先选代际、查目标日志，再核验/续作固定首条决定。同 operation 的重复激活返回原永久结果，不重复增加拓扑代数。

正常新业务只写目标活动日志。全部成员正常发布规则不变；缺员时即使目标 Q 足够读取已完整证明状态，也不能新写。原 placement/route、正文和历史证明保持原样，新成员不会收到伪造的旧正文 Receipt。

### 启动、恢复与离线消费者

`open` 只读发现代际并核验已有活动 seal、本盘 joint 与归档，不自动创建 marker、提升 pending seal 或覆盖坏成员。单个损坏 marker/归档隔离该资源，固定 N/Q 不变，其余目标多数可以读取完整已发布状态；两个完整但相互冲突的权威不按票少或时间新择优，直接拒绝。只有旧 fence 而新目标还是空目录时，不把未开始激活的新根误判为原池坏锚。发现候选只补未知 disk_id，已知盘始终使用当前已绑定路径。

prepared marker 或 seal promotion 中断后，调用方显式使用 `Engine::resume_membership_activation(system_root)`；该入口从真实同绑定材料续作，不猜测未知非空目录归属。没有完整初始绑定的遗留材料仍保留且拒绝自动接管。新代完整发布以后，系统丢失可由固定目标多数恢复，包含“原多数已不足，但一个原成员加一个新增成员满足新多数”的情形。路径重绑定继续核对 disk/slot/binding 与物理 device/inode。

每个活动根保留原配置锚与独立 `generation.json`。data-only 本盘旧证明读取准确 Candidate 的冻结清单 inclusion proof；新证明直接绑定代际。缺页、错误 hash 或缺权威拒绝，离线工具不回系统目录或其他成员偷读。已有正文 proof 字节不变；新代内容墓碑可与旧正文证明组合验证。独立历史包只在新 Candidate 包中增加便携权威，旧包继续原字节读取。历史包导出、新旧永久工作区收据恢复和当前清除/reprotection 均沿所引用 Candidate 的确切代际。

scrub 按确切旧 publication 的序号顺序恢复已承认不可用的副本摘要，再按新 publication 更新；后续补齐可撤销旧缺失状态。它同时检查新日志与每个元数据成员的独立冻结控制归档，不允许健康副本补齐该成员的缺页。归档逐页、逐帧读取连接原 observer，保留取消与限速；不同对象缓存复用原 scrub 的内存上限，重复读取不当作新对象收费。资源预算耗尽沿原 `ResourceLimit` 语义报告未完成，而不是盘坏、隔离或健康完成。

### 成本和范围

原全历史控制归档的重复验证成本仍可能接近历史二次量级，激活需要按目标成员复制、独立检查，并向 data-only 根安装冻结清单路径证明。便携 joint 元数据仍随成员增长；没有暂停时间、吞吐或线性复杂度改善承诺。每个新 marker、独立对象和 seal 保留现有原子文件、文件同步与目录屏障；普通发布及旧归档屏障没有削减。

上述首次批次不包含第二次添加、计划下线、删除/恢复删除、退休根返回、正文排空、管理权限/API/Web，也不表示硬件/物理掉电、跨主机或特权克隆回滚安全验收。首次批次证据与下述后继实验必须分开，不能用文档描述代替验证。


## 重复纯添加的后继实验

本批只支持每块新增盘均为 metadata voter，不支持仅新增 data-only 或混合新增；原有 data-only 保留。该 metadata-voter 批次已通过 GitHub 定向验证，准确提交与范围见[重复添加验证](../validation/repeated-membership-activation.md)。它不开放下线、删除或 HTTP/Web。

`Origin::Successor` 明确绑定前驱 authority hash 与该次交接的源配置。前驱配置必须是被认证前驱的 target 配置；恢复路径只允许路径投影不同，成员、角色、路由、绑定和设置保持一致。每次 fence 追加到源代自己的活动日志，目标仍有独立固定 N/Q 的 joint 证明；前驱激活必须在冻结日志中完整发布，不能只凭一个准备中的 marker 建立下一代。

单代 authority 节点保留 `{joint, archive}` 的原规范字节和哈希。后继便携信封使用严格 `membership-generation-chain`，平铺保存 head 与按哈希寻址的前驱节点；不把祖先完整信封递归嵌入下一代。缺节点、错哈希、循环、多余节点、不相接的配置或未完成前驱拒绝。epoch 由已验证链长度 checked 递增，不能由拓扑代数或任意输入数字指定。

每份冻结归档只有一个共享的内容哈希对象树，保存当前段及所有可达祖先段所需的 root、authority、intent、snapshot、history 与 seal 对象。后继日志段以 transition hash 区分，Genesis 段保留原路径。各段分别用其准确成员、authority、起始 snapshot 和首条规定激活决定验证；只把 Publication 纳入历史 sequence/hash 索引。不同段同序号冲突拒绝，Prepare/Accept 不会被提升为历史权威。

归档复制和目标独立验证按平铺祖先链遍历，各对象按地址去重，共享 observer 与唯一对象预算；所有实际读取仍计字节并检查取消。归档内持久 config.json 原字节仍是 Genesis 锚，不能错用当前三盘/四盘配置验证它；journal/seal 的所有者则是该段当前代际。后继 seal 必须绑定准确前驱 hash。最新 State 只从最新冻结边界及其降级 publication 覆盖恢复，不把祖先快照叠加或回退为当前 State。

旧 inclusion proof 按 Candidate 的代际选择当时冻结根，文件字节无需重写。data-only 和独立历史包携带完整便携权威，不读系统目录或其他成员。启动遇到同一可验证链上的旧 marker 与后继 marker 时选后继；两个不可比较权威仍拒绝。系统丢失时，看到后继但不足后继固定多数不能退而用祖先多数恢复成可写。

保留的不同过渡归档仍可能各自物理存放完整控制闭包，成本随历史与代际增长；本批仅消除同一归档内重复对象及递归嵌套归档，不宣称跨归档物理去重或线性验证。没有用硬链接、共享可变文件或后台补齐冒充独立成员复制，也不宣称新增正文副本。

## 代际必须保留原始候选字节

动态配置代际应由独立的、旧成员授权的过渡证明绑定。过渡指向原成员描述、准确旧提交前缀及新成员描述；历史 Candidate 按经验证的前缀/序号边界查证其原始成员，不按最新 Config 重新解释。前缀必须绑定准确 Candidate/快照哈希链，不能仅凭可伪造序号决定归属。

当前 Candidate 的原字节及其 object hash、旧日志与永久操作结果保持原样。对旧 Candidate 强加必填成员摘要会使已经发布的池无法读取，不能作为在线增删前置条件。也不能用可缺省字段或重新生成旧候选来掩盖断链。新增过渡对象/记录在生产实现中仍须有严格类型、独立域哈希、不可变引用及拒绝未知内容的规则。

## 联合交接的选定方向

采用单协调器交接，切换期间暂停业务写入。在线读取可继续，复制可以分批执行；不承诺持续无暂停写。当前实验先 fence 再做完整控制归档，停写时长受历史规模、I/O 与失败恢复影响，故障期间可持续停写，没有已验证的短暂停上界。预复制不可变历史与正文、最后冻结复核增量以缩短停写的方案尚未实现。不要把系统目录 flock 当作系统盘丢失后的全池 fencing。

持久权威需要三类独立材料：

- 不可变成员描述及从 genesis 开始的链：每次过渡绑定原代际/摘要、新代际/摘要、前驱过渡摘要、永久 operation_id、请求摘要和准确的冻结提交前缀。新代数采用 checked_add；同代不同摘要不能以时间或在线票数择优。
- 旧代际停止写入的持久 fence 与联合决定：旧/新集合分别使用其原定多数 Q_old/Q_new；不得对并集取一个多数或按在线数缩小门槛。正常交接完成仍要求旧、新配置全部成员同步，而非仅双多数。
- 历史恢复归档与每个新成员自己的 bootstrap：归档保存原成员身份下的日志/证明及完整依赖；bootstrap 绑定归档根、覆盖序号、新成员身份和联合决定。不把旧日志复制成新成员的活动日志，不改写旧回执署名。

这些描述/证明必须以不可变对象和真正可验证的日志记录实现；系统配置、进度文件及 SQLite 只能是投影。仅新增 JSON 任务状态或一个最新快照没有权威效力。

## 持久顺序与恢复选择

1. 保持旧成员 writer 锁，并按验证后 held FD 取得所有新增成员锁。拒绝冲突目标、重叠目录、非空新目录和身份替换；不创建挂载、不改权限、不擦除数据。
2. 解决旧代际的在途决定，固定过渡身份。在旧集合持久写入同一 fence；一旦发出首份 fence/ACCEPT，超时、取消和部分失败均不能表述为已回滚。恢复取得旧、新两侧所需可信完整观察并沿原过渡继续；若证据不足，保留数据并阻断。
3. 在让新集合取得独立写权前，至少 Q_old 的持久 fence 必须成立。每次启动（含系统盘丢失恢复）在写入前持锁读取当前代际完整 Q 的日志；任何旧多数与已 fenced 的旧多数相交，所以返回旧盘不能悄悄复活旧写者。正常结束要求所有旧成员留下停止旧代际写入的证明。
4. 新旧多数共同决定同一目标、前缀与归档根；新集合逐成员接收并验证全部恢复依赖及自身 bootstrap，再取得全部新成员的 readiness。联合决定不可回滚。归档缺页、错误哈希、短读或未完成同步不能产生 readiness。
5. 全新成员持久保存同一完成证书，证书绑定双侧决定、完整新侧 readiness、原结果及新拓扑/放置。完成证书可移交旧物理盘的证明责任，但不能代替新集合对更晚 fence 的启动观察。
6. 仅完整持久闭包成立后，原子发布新 Engine 配置/状态并允许后续写；系统 `config.json` 最后更新为可重建投影。旧锁和目录仅逻辑退役，数据保留。失败的系统投影更新不能把已成立的交接改报为“未发生”。

首个纯添加使用前节明确的权威、bootstrap 与固定首条发布；重复纯添加由前述后继链实现，退出与退休隔离仍未实现。本节描述一般方向，不表示任意成员过渡已支持。

## 保留历史与排空

历史 Candidate 永久按已认证的提交前缀所属代际及成员摘要查证原始成员描述。当前成员负责保存归档，不改变历史表决者。原 Candidate、ACCEPT、FINALIZE、PUBLICATION、永久操作结果和原发布证明保持字节/哈希不变；新增 reprotection 是当前成员的独立证据。检查点不能裁掉 fence、过渡链或任何恢复/离线导出依赖。

排空的权威计划覆盖所有未清除保留版本、历史本盘证明、显式清除/删除证据、策略与来源路由、工作区及永久收据引用，不只遍历当前 HEAD。目标是独立完整复制，逐块与最终摘要核验，并完成文件和父目录屏障。容量计划须预留峰值，迁移期间旧内容保持可读。

现有 `protection/backup_paths.rs` 会遍历作用域中全部 Available 保留版本，取得 `valid_replica`，并通过 `Prepared` 与新 Placement 重新发布副本证明；此路径可以复用完整复制/校验与发布原语，但它只认识当前固定槽位，不等于池级排空。旧 Candidate/操作结果仍保留，目标盘需要本次重发布自己的新证明，不能复制旧盘本地回执便声称目标已持久化。

冻结写入后复核计划依据、授权及全部引用，补齐增量，再将成员变化与对应放置/拓扑作为同一决定发布。现有读租约保留原资源/锁直到结束；旧资源尚被引用时不关闭、不解绑、不删除。迁出后旧目录返回按退休绑定隔离。最后健康副本、证据不全、容量不足或未决操作均阻止退出；逻辑移出从不授权物理擦除。

## 分阶段可验证门槛

1. 当前阶段：固定身份、准确集合、旧配置锚不可被系统配置重解释；原 Candidate 规范字节/哈希保持，真实临时目录恢复与系统盘丢失重建继续通过，缺锚时不重写材料。
2. 代际权威：不可变描述链、双侧决定、持久 fence、按代际历史 resolver、每成员 bootstrap、系统配置丢失后恢复。每个持久屏障前后分别注入进程退出和 I/O 失败；覆盖同代分叉、漏旧代、改旧摘要、旧/新各侧缺盘和退休盘返回。
3. 数据迁移：逐版本完整计划、复制/校验/同步证据、峰值容量、租约/工作区/策略冻结、切换与永久收据。失败后能查询同一操作并继续，不重新登记目标身份。
4. 真实核心操作通过后才公布预览/提交/查询 API，再接 Web。覆盖同幂等键同意图重试、异意图冲突、撤权、重启、响应丢失与重复点击。不能用不可执行占位按钮提前宣称阶段完成。

临时目录故障测试不证明物理掉电、硬件缓存诚实性、跨主机 fencing 或恶意/克隆回滚磁盘安全；这些均另行验收。只有配置代数或本机 inode/flock 不能覆盖这些边界。


<a id="data-only-addition"></a>

## 非投票新盘与混合新增实验

后续独立批次扩展为 pure-add 的全部新增设备，不再要求每块新增盘都投票。相同 voters 的 data-only 添加也必须经过准确新 intent、全部旧 fence、每代归档、joint 与活动发布，不能直接改系统配置。旧/新 N/Q 仍只按各代 metadata voters 固定计算，设备数量增长不增票。

新增非投票根与 metadata 成员资源分开持有，原身份预检、根描述符与写锁均保留。非投票根使用独立 `data-ready.json`，包含精确 pending seal、physical identity、transition/target/archive 绑定；它没有 control archive、control journal 或 joint vote。全部新增 seal 先同步，ready 文件及目录再同步并确认 held/named 目录身份。已绑定的同意图显式续作可补缺失 ready；合法原子临时文件须匹配本 op canonical ready 的完整字节或严格前缀，保留原临时文件并重新原子写完整记录。未知非空初始材料、冲突字节或坏正式 ready 不能接管、吞掉或覆盖。

包含新增 nonvoter 的 `fenced-device-decision` proposal 通过分页引用绑定准确非投票新盘清单，certificate 的每一 metadata vote 绑定同一 proposal。已有 `fenced-joint-decision` 哈希对象表示不变且只接受没有新增 nonvoter 的过渡；不得把数据盘藏在旧形态中。缺 ready、缺 dependency 页、设备身份重复或自改 seal 无法替代已认证身份。runtime 激活、冷启动和独立本盘 proof 均沿前驱链定位该盘最初准入；普通发现不补写 ready。

新数据盘先仅安装自身 readiness、配置锚与 portable authority，既无正文也无旧正文 proof，不宣称补齐历史副本。之后实际业务可以在新 primary 写入或按新合法备份路由生成副本；旧版本 placement 不变。数据盘作为后续代际既存资源时，按旧候选的准确归档根保存 inclusion proof。全部新增资源和锁进入活动集合，系统配置仍最后投影。

此批仍待 GitHub 执行，不扩大为下线、删除或 API/Web；具体定向故障与数据验收见[非投票与混合新增验证](../validation/data-membership-addition.md)。

## 关联

[本批次验证范围与待验项](../validation/first-membership-activation.md)。

[首次激活](0012-management-and-storage-activation.md)、[普通发布](0002-publication.md)、[持有磁盘资源](0007-owned-disk-resources.md)、[事务恢复](../10-transactions-and-recovery.md)、[磁盘格式](../08-full-storage-and-disk-format.md)。

## 退出控制准备：并集参与者与独立多数

退出意图显式区分计划下线和注销，绑定准确目录身份；目标投票集合变化不删除历史拓扑记录。退出控制 proposal 分页保存 target 与 old-only 依赖，真实票按两侧身份分别计数。可搬运证书验证接受达到固定 oldQ/newQ 的真实子集，不强制每个退出成员投票；实际本地准入仍要求该成员自己的日志。

此阶段只扩展控制证据，不能生成可激活的 membership authority。原当前登记、内容可见性与资源保持不变，永久下线/删除结果不得提前出现。当前 fence、归档和运行时同步驱动仍暂需原全员，缺席退出的采集路径尚未完成；不能把实现限制提升为联合多数公理。最后 voter 的退出还需独立终止权威。准确行为、未实现闭环和待执行测试见[退出成员联合控制准备](../validation/exit-membership-control.md)。
