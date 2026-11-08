# 在线目录成员管理

[服务端](../components/server.md) · [API](../15-api-and-errors.md)

**职责已修订：** 添加、注销/移除、外来接入和磁盘初始化迁往[离线辅助工具](helper-disk-management.md)，不再以 Web 作为这些操作的最终执行入口；用户发起的“计划下线”功能取消。下文记录迁移前仍需保留的实现、证明和中断恢复边界，不表示新职责已全部接线，也不能据此隐藏真实不可用状态或丢弃已持久意图。

本轮源码接通已有池的纯添加、原操作的认证取消，以及实际计划下线／注销协调器。这里只管理 ZXS 登记的存储目录，不管理 Debian 设备，不格式化、不挂载、不改权限、不探测系统盘。允许同一文件系统中的不同目录；目录数量不等于独立故障域数量。新增目录不自动迁移既有正文或改变现有文件放置规则。计划下线／注销使用独立退出入口，先完成真实 I/O 收束；不会自动搬迁正文或擦除文件。本文描述尚未联合验证的源码行为，不能当作已验收能力。

## 操作流程

1. 登录后在已有池管理页从 systemd 固定父目录的直接候选进入磁盘向导。目标须是该父目录的直接子目录，已存在、为空、服务账号拥有且为 0700，不能与控制目录、既有成员或另一选择重叠。
2. 每个新增目录分别设置唯一槽位、层级、上游槽位及 `metadata_member`。不勾选元数据即 data-only；允许一次混合新增 data-only 与元数据成员。其余池配置保持不变。
3. `POST /api/admin/disks/preview` 接收 `{operation_id, additions:[{selection,slot_id,level,upstream_slot_id,metadata_member}]}`。返回原请求、池身份、配置摘要、预览摘要和前后元数据成员数。预览只读，不能代表容量、硬件独立性或最终发布证明。
4. 用户核对后以 `POST /api/admin/disks` 提交严格 `{preview}`。提交重新校验身份、配置与全部目录，并把持有的目录 FD 交给引擎的纯添加入口，避免检查后通过路径替换转向另一目录。
5. 真实执行顺序为成员 fence、受管控制归档、pending enroll、joint decision、activation。归档存放在引擎专用受管目录，不要求用户另找目录。只保存控制证据，不复制已有 payload。
6. `GET /api/admin/disks/addition` 查询最新原意图；带 `operation_id` 可查指定操作。状态为 `adding/cancelling/recovery_required/completed/cancelled`；阶段还区分 `certifying_abort/abort_certified/cancelling/cancelled`。响应保留 `continuation:add|cancel` 和 `result`，后者仅在引擎确已 Published 时携带完整原 `OperationResult`。`POST /api/admin/disks/addition/resume` 接受严格 `{operation_id, continuation}`；服务器恢复原目标，不能更换目录或角色。
7. 用户明确勾选取消确认后，以 `POST /api/admin/disks/addition/cancel` 提交 `{operation_id, preview_digest}`。服务器先保存 Cancel continuation，再使用原 fence、完整控制归档和实际旧成员共享槽认证 Abort。没有完整归档时先仅从原旧成员补原 fence 和归档，不重新打开新增目标。归档完整时取消认证不额外要求所有旧成员在线，固定旧 Q 不随可用数变化。首取消 publication 仍须满足原资源的完整恢复要求。

## 幂等、恢复与授权

- 管理记录位于系统管理目录，原 operation_id 的请求、目标配置和归档绑定永久保留。同一操作同一预览返回原结果；同一编号不同预览拒绝。每次只有一个添加协调器运行，未完成原意图阻止另一计划接管。
- 客户端断开不会取消已接管的阻塞任务。停机先关闭准入，再等待该协调器释放。服务重启时未完成的控制记录显示需要恢复，而不是伪称后台仍在执行。
- 每个关键引擎阶段重新校验当前管理会话。失效会话、同源及 CSRF 仍由现有 HTTP/授权层拒绝；预览摘要不代替授权。元数据归档后撤权不会自动获得最终 activation 权限。
- 失败响应不是成功回执。已经接纳原意图的失败保留具体阶段、错误码及原因，返回需要恢复；界面保留原编号并查询/继续同一操作。未知结果不能换编号重发。
- 完成必须匹配原 operation_id、原归档的 transition hash、原 actor 和 Published 结果；`membership-add` 是添加完成，`membership-add-cancelled` 才是取消完成。完整引擎回执永久保留，不按本次请求改名。取消来得太晚时返回原添加结果；控制投影丢失时先查询实际已发布回执，不重新选择相反结果。
- AbortCertified 不是取消完成。旧 Joint 多数、不可达 AbortQ、缺少原 fence、归档或原资源时明确失败并保留具体阶段，不能用“还没看到成功”推断可取消。部分 Abort 失败也不会自动转回 Add；重放旧 commit 保持持久 continuation。只有用户再次明确选择 `continuation:add` 才尝试恢复添加，核心仍按真实共享槽拒绝任何已冲突的决定。
- 取消保留新目录中的 reservation、pending seal 和准备材料；不删除目录、不擦除正文、不重用已取消 operation_id。新计划须使用新 operation_id。界面关闭、刷新、超时或响应丢失都不是取消。
- 首次 setup 完成记录保持不可变。后续配置必须沿已持久化 Add/Exit 记录的完整摘要链连接首次配置，且匹配最新意图端点，才能进入引擎自身的代际证明恢复；这条管理准入不代替引擎对成员权威、归档和发布收据的校验。

<a id="ordinary-disk-conversion"></a>
## 普通盘转换为新一级盘

当前已实现的集成转换只接受已有活动系统和两个不同的直接候选：普通来源与
另外一个严格空目标。来源保持只读，目标接纳成新的一级盘，源相对文件名和
空目录保留在该盘的根下，不额外包一层目录。它不是对源设备原地格式化，也
不是把文件塞进旧盘某个目录后声称完成盘转换。首次尚无 Engine 时，应先用
空盘向导建立活动系统；外来 ZXS 的完整历史迁入不经过这条 contents-only 路径。

1. `POST /api/admin/storage/conversion/preview` 接收原 `operation_id`、发现所得
   `source`、空 `target`、新 `slot_id` 和 `metadata_member`。服务端强制
   `level=1,upstream_slot_id=null`，不接受客户端指定更高级别。先排除路径
   父子关系和相同 device/inode，再按真实 mountinfo 拒绝别名；完整清点来源
   文件/目录/正文总量，绑定源观察、清点摘要和实际成员添加预览。只读预览不
   初始化目标，不创建转换持久记录。
2. 返回 `OrdinaryDiskConversionPreview`，带源清点、完整目标添加预览、原注册
   operation、独立 import operation 和合并 `preview_digest`。容量估计不含
   所有元数据与额外备份副本，不是空间保证。只复制文件正文和目录名称；不
   保留源权限、owner、时间戳、ACL、xattr 或外部历史，必须明确展示此范围。
3. `POST /api/admin/storage/conversion/commit` 只接受 `{preview,
   acknowledge_contents_only:true}`。当前会话重新核验，摘要/源清点/空目标
   复验后先持久化原意图，随后用 typed 添加协调器接纳一个一级盘。客户端断开
   不取消后台所有者；有操作记录以后不得悄悄生成另一个编号替代它。
4. 添加永久完成后，重新核对确切新 disk identity、角色、路径及空命名空间，
   才通过正常工作区事务写入根内容。根创建 operation、预分配 workspace ID
   和最终完整 `CommitRequest` 分别在对应持久化动作之前保存。添加和正文导入
   是两个持久阶段，不宣称跨阶段原子。源发生变化、目标冲突或后续错误时
   保留源、新盘、工作区及证据，不以删除或覆盖来回滚。
5. 只有准确原 import operation 的永久 `workspace.commit` 发布回执能让整体
   状态变成 `completed`。状态中的 `registration` 单独报告添加结果；仅注册
   成功不是转换完成。`GET /api/admin/storage/conversion/status` 可查最新意图，
   `?operation_id=…` 可查原意图；未知指定编号返回 `OPERATION_NOT_FOUND`。
   重启后按保存的完整提交请求核验真实回执，不把同 operation 的其他请求、
   本地 completed 字段或仅有工作区 ID 当作成功。
6. `POST /api/admin/storage/conversion/resume` 只接受原 `{operation_id}`，可显式
   继续原注册阶段，以及尚未留下任何根操作/工作区/提交请求意图的导入前窗口。
   所有导入写入必须先持久化这些意图；仅有开始标记不等于已经写入。已有任一
   写意图或其结果不确定时，当前入口只核验原发布结果，否则保留 `recovery_required`；
   不生成替代工作区、不重新清空目标。现阶段尚无一般性保留工作区自动续传，
   不能承诺一键继续所有中断位置；可以使用下述停止并保留出口。
7. `POST /api/admin/storage/conversion/stop` 接收原 `{operation_id,
   acknowledge_retained_state:true}`，仅用于原注册已完成后的导入阶段。它先
   持久化唯一 `stop_operation_id`，进入 `stopping`，再由核心拒绝未决决议、
   活跃提交准备、不同主体或目标。根创建只按原 operation 校验已发布回执，
   预记但不存在的工作区不会重建；存在的原工作区仅沿原生 abort 标记关闭，
   保留其暂存字节。核心随后发布独立 `directory.import.stop` 收据，围栏原
   import ID；该回执才能使状态变成 `stopped` 并放行新的独立转换。
   `stop_result` 与导入 `result` 分开，停止不是内容转换成功，也不注销新盘。
   如果原 import 已精确发布则返回 `completed`，不能把它改写为停止。
   停止中断或响应丢失只查询/继续同一个 stop 意图；普通 resume 不再执行导入。
   管理缓存丢失时从原永久停止收据恢复，不单凭本地状态解开围栏。

一次转换在来源扫描开始时取得已准入的准确 Engine，并随私有类型化准备值持有
到登记和导入结束。较慢的扫描或登记不会因后台健康样本超过 15 秒而把同一已
准入流程重新派发失败；新的独立请求仍受原健康时限限制。准备值不序列化、不
作为永久授权，导入前继续核对服务停止、当前会话、准确目标身份及核心写入围栏。
尚未进入导入的恢复请求必须重新经来源准入准备，不用不受健康门控的恢复 backend
替代初次准入；已完成的永久回执查询不重新扫描来源。

状态读取先持有可取得的转换协调门再读取记录，不用旧快照推断刚完成的写入
需要恢复。前笔原主体的已完成工作只经准确永久回执检查是否结算，不阻塞凭据
恢复后的新管理员开始独立工作；这不允许新主体读取旧来源详情或接管未完成意图。
无编号的最新状态查询对已精确结算的其他主体记录返回 `operation:null`，明确
编号查询仍拒绝跨主体读取。即使协调门正忙，缓存的完成/停止终态也必须重新
匹配永久回执，不能仅凭本地控制投影报告成功。

发现和转换各自有界，扫描不跨 symlink 或嵌套挂载。普通来源若含 ZXS 或不明
存储保留材料会被拒绝；损坏 `disk.json` 不能被当作普通文件带走而丢失历史语义。
文件字节复制仍走现有命名规则、资源配额、提交门槛和同步发布证明。新功能
没有放宽 quorum、真实设备独立性或生产持久性要求。

### 外来 ZXS 的设计边界

用户目标是每盘可移植的自身内容及来源闭包，系统盘只保留可重建的快速索引。
当前实现仍有共同配置/多数派证明、绑定物理目录的 seal，单份 SQLite 复制或
改写 pool identity 都不能达到该目标。只读识别的级别也不证明可迁入历史完整。
因此目前不开放可执行的外来盘迁入，不修改源盘、历史哈希或证明。

现有只读入口是 `POST /api/admin/storage/foreign/preview`，输入仅为发现所得
`source`。它与添加/退出共用生命周期选择门，持有已验证的源目录描述符，
执行前后重新核对父/子目录观察和管理会话，排除已登记、停止和系统目录。
初始 seal 的物理位置变更不会放宽正常可写准入；只读检查可以保留原 seal
字节并另记当前物理观察，验证可读取的逻辑闭包。

返回的 `ForeignDiskPreview` 提供原盘身份、级别证据种类、原 seal 摘要、
复核摘要、本盘最后完整发布 checkpoint，以及按原一级身份绑定的历史摘要。
`temporarily_unavailable_versions` 保留缺失正文的版本编号，既有 purged/
migrated 状态分别统计，不把它们混成缺盘或擅自恢复为可用。分支、目录树和
提交计数来自完整已验证清单，不是当前视图内容复制。响应始终携带
`execution=unavailable`；这些证据既不授予作者权限，也不证明原系统全局最新。
缺少有效闭包、未知格式或损坏正文直接拒绝，不修复、不生成目标投影。
当前来源分组要求一份连通历史的原一级身份唯一；同一连通历史跨多个原一级
身份时明确返回 `FORMAT_UNSUPPORTED`，不能在尚无新图证明时擅自拆分。空历史
和缺少原始版本发布证明的历史也不猜测来源身份。

该显式检查的工作量与保留元数据及本盘可验证正文成正比，持有一个管理任务
和生命周期选择门；不会在发现轮询中自动扫描历史。服务固定限制为 64 MiB
累计元数据读取、16 MiB 日志、10000 个版本、8 GiB 本地正文校验、128 份历史摘要及
1 MiB 完整编码响应。超限整份拒绝，不返回被截断的“完整预览”。目前没有
迁入执行、失效引用裁剪或新逐盘提交协议，不能将只读检查通过表述为加入成功。

### 外来历史的持久审核计划

只读检查之后，可为已有、已登记的一级盘建立重建审核计划。当前映射范围是该
一级盘的空根，或其中一个已存在的空目录；并不创建新的物理一级盘、不将外来
盘登记为本系统成员，也不执行降级。每份已验证历史须明确一个目标子目录名，
完整覆盖全部来源组并按命名规则拒绝碰撞；多个原一级来源仍分别保留身份。

1. `POST /api/admin/storage/foreign/plans/preview` 接收 `operation_id`、原发现
   `source`、已检查的 `source_review_digest` 和 `target`。目标含 `slot_id`、
   必填且可为 null 的 `parent`，以及以来源 `group_id` 为键的 `history_names`。
   服务端重新读取全部源证明和本盘正文，再经核心检查目标当前登记、一级角色、
   空目录及名称。预览绑定完整来源摘要、当前物理路径/device/inode 观察、目标
   登记身份、发布 sequence、分支/HEAD 和每个映射路径，不预留或写入目标。
2. `POST /api/admin/storage/foreign/plans/save` 接收完整预览与
   `acknowledge_plan_only:true`。重新核对当前会话、来源和空目标后，才在系统
   管理目录的 `foreign-reconstruction-plans/<operation_id>.json` 独占创建不可变
   审核记录。该记录是审核消息的审计副本，不是引擎权威或迁入授权。响应始终
   为 `validation=required`、`execution=unavailable`，阻断原因固定为
   `portable_import_writer_unavailable`。
3. `GET /api/admin/storage/foreign/plans/status?operation_id=…` 只读取当前主体
   自己的原计划；没有全局 latest。相同编号和完整预览的保存重试返回原审计
   记录，不重新打开来源，仍标记 `required`。同编号不同内容、损坏记录、未知
   格式或其他主体一律拒绝，不覆盖或修复原记录。写入结果不明时查询同编号，
   不以新编号假装原保存失败。
4. `POST /api/admin/storage/foreign/plans/revalidate` 接收原编号和审核摘要，
   再完整读取来源并重新映射同一目标。仅完全一致时返回
   `validation=matched_at_observation`；此状态只描述本次观察，不持久缓存为
   有效、保留空间或解除执行阻断。之后普通查询仍为 `required`。源盘替换、
   原路径变更、目标 checkpoint 推进、目标不再为空或名称变化需重新审核，
   不自动改写已保存计划。
5. `POST /api/admin/storage/foreign/plans/execute` 校验当前主体、原计划和摘要
   后固定返回 `FORMAT_UNSUPPORTED`。即使复核刚刚匹配，也不创建目录、工作区、
   新登记或迁入收据。尚缺逐盘可移植写协议及新目标完整历史发布证明，不能将
   保存计划或复核匹配表述为迁入完成。

预览、保存与复核共用现有生命周期选择门；慢速检查由受管理任务持有，前后
核对当前会话。源读取沿用上述预算，完整预览限 1 MiB，请求和持久记录限
2 MiB，超限整份拒绝。保存只增加系统盘的单份审核文件及目录同步，不发布
数据盘事务；已有记录查询只作有界控制读取。目标映射摘要含当时的业务基线，
未来执行仍须重新证明来源、当前目标和实际复制正文，不能把摘要等同写能力。

后续向导须先验证本盘闭包并在新目标预览来源→目标身份映射：外来一级保持
自身结构；二级按每个可证明的原一级身份分别重建，不把多个同名来源合并；
外来二级及以上备份盘须在同一计划显式设置并纳入全部所需低级链，三级同时包含二级和一级；不允许虚占位、先接高层后补低层，也不能以改角色绕开已要求的完整链证明。
指定目录需保持空且逐名称拒绝碰撞。裁剪仅适用于新目标投影中已证明失效的
引用，不自动删除源历史；正文仅在暂时未接入磁盘上的历史记录必须保留并标记
“内容暂不可用”。缺盘、缺 `directory_commit` 标记或尚未收齐证据都不是失效证明。
原有有效目录 checkpoint、文件版本和节点祖先仍须保留可验证引用；新投影不能
继承不再匹配的显式目录提交证明。该架构尚待新协议实现和故障验证，不是本轮
普通内容转换已交付的能力。

## 计划下线与注销

- 磁盘卡分别提供明确的下线／注销预览，操作对象是 `expected_disk_id + slot_id` 指定的当前登记，服务器从已认证 registry 获取准确 binding_generation。历史 Config 中仍保留的条目不是当前登记授权。
- `POST /api/admin/disks/exit/preview` 接收 `{operation_id, slot_id, expected_disk_id, action}`，action 为 `planned_offline` 或 `deregister`。预览绑定注册目录、配置摘要、已发布 source sequence/snapshot 和 topology generation。提交严格 `{preview}`，再次比较完整源与当前会话，不能通过过期预览注销另一绑定。
- 下线保留登记与全部字节，发布 Disabled 并关闭该目录全部 ZXS I/O。注销移除登记，按引擎的已验证 occurrence mask 隐藏目标独占的当前文件；历史元数据及磁盘字节保留，不写目标数据备份或恢复文件。
- 实际协调器调用 `begin_disk_exit`、受管控制归档、pending enroll、joint decision、`activate_pending_disk_exit`。只有后者验证真实 retained D3 发布及当前 registry/view 后返回的原 `disk-planned-offline` / `disk-deregister` Published 结果，才可记录 completed；控制准备、固定多数或本地状态标记不能冒充成功。
- 系统管理目录永久保存原动作、actor、准确原配置、预览、archive/transition 绑定和完整回执。添加／退出共用一个受停机等待保护的协调器门，未结算持久意图互相阻止新计划。HTTP 断开不释放任务所有者，错误后保留具体阶段并显式继续原 operation_id。
- 恢复根据引擎 `disk_exit_status` 的真实 `Fenced/Activating/Published` 分流；只有 Fenced 可以重建原控制准备，Activating 直接继续退出，不能因 HTTP phase 缓存而重开已 stop 的目标。GET 仅在引擎同时确认实际当前 view 与永久结果时补出已发布状态。
- 首次 setup 记录不改写。管理恢复准入流式校验 Add/Exit 意图的配置摘要边，要求当前端点能连接到初始激活配置；同配置的 Disabled→Detached 不产生循环。全部池设置和历史绑定不变。该控制记录只用于准入，引擎随后必须验证真实 authority、持久 stop、registry 和 D3，不靠控制记录降低 quorum。
- 后继 Add、再次 Exit 和 Disabled→Deregister 已有真实 registry-lineage 与恢复源码；旧操作回执可在后续代际继续查询。Disabled→Deregister 复用原 durable stop，不能重开目标。唯一 old-only voter 或 data-only 退出目标缺席的准备/恢复已有[严格证明源码](../validation/missing-exit-members.md)，前者须原固定 Q 与全部 retained target，后者保留完整原 metadata 格式。最终元数据 voter 退出、没有已发布源边界、必要历史闭包或另一 retained metadata 成员缺失，以及 Detached 重新登记仍明确拒绝；不会伪造完成或创建绕过核心的路径。
- 活动 metadata 组中，保留的 Disabled 目录可通过[认证重新准入](../architecture/retained-directory-readmission.md)恢复历史原角色。原登记和 binding generation 不变，仅目标建立独立新 I/O epoch；旧 stop、字节、永久收据与已隐藏 occurrence 保留。该核心接口不代替服务端认证，也不允许将 Detached 或终态池借此重新启用。

## 保留 Disabled 登记的管理入口

- `GET /api/admin/disks/registrations` 只投影当前已认证 registry，区分活动成员、终态管理权威与未准入。观察健康不决定登记；Detached 不出现在可恢复登记中。每个 Disabled 项明确返回可恢复的历史角色或 `readmission:null`，不从目录存在推断可用。
- `POST /api/admin/disks/readmission/preview` 接收严格 `{operation_id, slot_id, expected_disk_id}`。纯元数据预览绑定服务器原路径、pool/disk/slot/binding、原 actor、配置摘要、authority、registry generation/root 和已发布 sequence/snapshot/topology；不打开停用目标。
- `POST /api/admin/disks/readmission` 接收严格 `{preview}`。在共同生命周期门及引擎锁内重新比较完整预览并刷新当前会话，随后才打开准确原目录。现有核心状态另提供原 actor，重试不能仅凭可修改管理缓存的 actor 接管已经 Fenced/Activating 的操作。真实目录身份、固定成员和新 epoch 仍由核心认证。
- `GET /api/admin/disks/readmission?operation_id=…` 只读原状态；省略编号读取最新意图。`POST /api/admin/disks/readmission/resume` 仅接受原 `{operation_id}`，显式恢复原目标和 actor。新增、退出与 readmission 的未完成意图互相排斥，首 setup 配置摘要链同时接纳 Readmission 边，但不把该管理链当核心权威。
- 完成必须来自已准入核心的永久 `disk-readmit` Published 回执，准确匹配原操作、actor、归档 transition、目标登记及新 `io_admission_epoch`。本地 completed 缓存不能独立宣称成功，后继下线仍可查询旧回执，不能借旧回执重新打开目标。
- Web 使用准确当前登记及原角色显示预览；关闭未提交预览、离页、重新登录和迟到响应不能改换已提交操作。未知结果保留原编号，只查询并允许明确继续。每次 GET 仅释放自身读取槽，旧结果不能清除新请求的 owner。
- 活动池入口可单独使用。终态池恢复能力由独立 Terminal retained 协调器接入同一严格 authority 判别契约；未提供可恢复角色时界面不开放按钮。Schema、OpenAPI、固定向量和 Rust 编解码检查已同步，仍不代表实际网络、浏览器或掉电验收通过。

## 验证边界

本轮遵照用户指令，只修改源码和测试源码，仅执行固定 Rust 1.99 rustfmt 与 git diff 空白检查，没有执行测试、构建、模型检查或 CI。新增源码覆盖纯 data-only、连续代际、mixed、原请求重放、不同意图冲突、持久恢复和会话失效，以及目标路径变成不可访问链接后取消、固定 Joint 多数拒绝取消、原结果永久重放、取消发布后控制投影丢失／重启和取消前会话撤销场景；退出新增测试源码还覆盖真实两种退出结果、目标文件逐字节保留、当前登记与当前文件可见性、原回执重放、管理完成投影丢失后重启、预览过期／动作冲突／会话撤销和无回执拒绝完成；不把测试源码的存在视为已通过。

真实设备、物理掉电、生产挂载及部署验收没有执行。后续仍需实际执行各持久阶段中断、目录替换、权限变化、空间不足、掉线、并发请求、客户端断开、关闭服务、刷新和重登等场景。本批退出源码尚未得到功能通过证据；上述核心拒绝分支、所有权尾部和恢复路径仍须实际验证。

新增成本只在管理控制面：每个已持久阶段增加一次原子控制记录同步，GET 仅查看已恢复的内存永久回执，不扫描全部历史、不重新探测目标目录。完整归档、固定旧槽证明及首 publication 的既有同步预算不减少。管理锁与引擎所有者贯穿协调器，客户端断开不释放所有权；普通正文路径不增加此状态机成本。
