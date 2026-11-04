# 在线目录成员管理

[服务端](../components/server.md) · [API](../15-api-and-errors.md)

本轮源码接通已有池的纯添加、原操作的认证取消，以及实际计划下线／注销协调器。这里只管理 ZXS 登记的存储目录，不管理 Debian 设备，不格式化、不挂载、不改权限、不探测系统盘。允许同一文件系统中的不同目录；目录数量不等于独立故障域数量。新增目录不自动迁移既有正文或改变现有文件放置规则。计划下线／注销使用独立退出入口，先完成真实 I/O 收束；不会自动搬迁正文或擦除文件。本文描述尚未联合验证的源码行为，不能当作已验收能力。

## 操作流程

1. 登录后在已有池管理页使用现有发现目录设置、浏览和检查。目标须在已授权发现范围内，已存在、为空、服务账号拥有且为 0700，不能与控制目录、既有成员或另一选择重叠。
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

## 计划下线与注销

- 磁盘卡分别提供明确的下线／注销预览，操作对象是 `expected_disk_id + slot_id` 指定的当前登记，服务器从已认证 registry 获取准确 binding_generation。历史 Config 中仍保留的条目不是当前登记授权。
- `POST /api/admin/disks/exit/preview` 接收 `{operation_id, slot_id, expected_disk_id, action}`，action 为 `planned_offline` 或 `deregister`。预览绑定注册目录、配置摘要、已发布 source sequence/snapshot 和 topology generation。提交严格 `{preview}`，再次比较完整源与当前会话，不能通过过期预览注销另一绑定。
- 下线保留登记与全部字节，发布 Disabled 并关闭该目录全部 ZXS I/O。注销移除登记，按引擎的已验证 occurrence mask 隐藏目标独占的当前文件；历史元数据及磁盘字节保留，不写目标数据备份或恢复文件。
- 实际协调器调用 `begin_disk_exit`、受管控制归档、pending enroll、joint decision、`activate_pending_disk_exit`。只有后者验证真实 retained D3 发布及当前 registry/view 后返回的原 `disk-planned-offline` / `disk-deregister` Published 结果，才可记录 completed；控制准备、固定多数或本地状态标记不能冒充成功。
- 系统管理目录永久保存原动作、actor、准确原配置、预览、archive/transition 绑定和完整回执。添加／退出共用一个受停机等待保护的协调器门，未结算持久意图互相阻止新计划。HTTP 断开不释放任务所有者，错误后保留具体阶段并显式继续原 operation_id。
- 恢复根据引擎 `disk_exit_status` 的真实 `Fenced/Activating/Published` 分流；只有 Fenced 可以重建原控制准备，Activating 直接继续退出，不能因 HTTP phase 缓存而重开已 stop 的目标。GET 仅在引擎同时确认实际当前 view 与永久结果时补出已发布状态。
- 首次 setup 记录不改写。管理恢复准入流式校验 Add/Exit 意图的配置摘要边，要求当前端点能连接到初始激活配置；同配置的 Disabled→Detached 不产生循环。全部池设置和历史绑定不变。该控制记录只用于准入，引擎随后必须验证真实 authority、持久 stop、registry 和 D3，不靠控制记录降低 quorum。
- 后继 Add、再次 Exit 和 Disabled→Deregister 已有真实 registry-lineage 与恢复源码；旧操作回执可在后续代际继续查询。Disabled→Deregister 复用原 durable stop，不能重开目标。唯一 old-only voter 或 data-only 退出目标缺席的准备/恢复已有[严格证明源码](../validation/missing-exit-members.md)，前者须原固定 Q 与全部 retained target，后者保留完整原 metadata 格式。最终元数据 voter 退出、没有已发布源边界、必要历史闭包或另一 retained metadata 成员缺失，以及重新启用／重新登记仍未实现或明确拒绝；不会伪造完成或创建绕过核心的路径。

## 验证边界

本轮遵照用户指令，只修改源码和测试源码，仅执行固定 Rust 1.99 rustfmt 与 git diff 空白检查，没有执行测试、构建、模型检查或 CI。新增源码覆盖纯 data-only、连续代际、mixed、原请求重放、不同意图冲突、持久恢复和会话失效，以及目标路径变成不可访问链接后取消、固定 Joint 多数拒绝取消、原结果永久重放、取消发布后控制投影丢失／重启和取消前会话撤销场景；退出新增测试源码还覆盖真实两种退出结果、目标文件逐字节保留、当前登记与当前文件可见性、原回执重放、管理完成投影丢失后重启、预览过期／动作冲突／会话撤销和无回执拒绝完成；不把测试源码的存在视为已通过。

真实设备、物理掉电、生产挂载及部署验收没有执行。后续仍需实际执行各持久阶段中断、目录替换、权限变化、空间不足、掉线、并发请求、客户端断开、关闭服务、刷新和重登等场景。本批退出源码尚未得到功能通过证据；上述核心拒绝分支、所有权尾部和恢复路径仍须实际验证。

新增成本只在管理控制面：每个已持久阶段增加一次原子控制记录同步，GET 仅查看已恢复的内存永久回执，不扫描全部历史、不重新探测目标目录。完整归档、固定旧槽证明及首 publication 的既有同步预算不减少。管理锁与引擎所有者贯穿协调器，客户端断开不释放所有权；普通正文路径不增加此状态机成本。
