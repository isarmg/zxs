# 待决定：Foreign 首片只冻结原链，不改外来盘上的一个字节

建议此次**只批准**完整原链的盘根绑定与保全意图。它是之后真正逐盘接纳的前置条件，
不是已接纳、安装、投票、已设 fence、可服务或可切换。选择 SYSTEM 单写而不先在高级
原盘上加私有文件，是因为今天的 `ForeignMigrationPlan` 对原盘整树做闭合库存：先在
原树里 mkdir/写 marker 再把该名字排除，会改变或削弱既有原物证明。不能用此提案授权
这种排除或先停住当前池等待尚不能完成的迁移。

**唯一新持久文件。** 复用既有受保护 SYSTEM 根，为每个原 `OperationId` 保存一个
`foreign-enrollment/<operation_id>.json`；无目标盘文件，无新 helper/Web/IPC/公开 API。
严格必填、拒绝未知字段的规范化 JSON，`format="foreign-preserved-chain-intent"`。
`operation_id` 决定唯一地址，原操作不能通过换 digest、路径或主体另存第二份。
使用既有不可覆盖写、文件及父目录同步，读取只能用原 SYSTEM 持有句柄。普通引擎
不得仅因有这个文件而启动 Foreign 任务；既有 preview/status 和只获准建立其它材料的动作
不得因此开始写它。只可从原获准完整接纳动作的内部入口准备，绝不返回已设 fence 的成功。以 `object_hash("foreign-preserved-chain-intent",
record)` 给后继内部消费者精确承诺；无第二套摘要算法。

**要批准的字段**（名称也作为本提案的边界，变更需重新审视）：
- 顶层 `format,operation_id,principal,request_digest,chain_digest,registration_proposal_digest,
  current_configuration_hash,current_sequence,current_snapshot_hash,current_state_hash,
  target_configuration_hash,recipe_hash,original,roots`。前三个 digest 保持原请求各自已有
  算法/域，不能当作互相可替换的 64 位十六进制。`recipe_hash` 新用途
  `foreign-preserved-recipe` 对已重算且有上限的准确 `ForeignAdmissionRecipe`；Config、
  State、快照须按现有基线/发布各自域取得，而非名字推导；只在经过验证的空 genesis
  使用显式 null 的 current_snapshot_hash，非空必须有准确原 Publication 的 snapshot。序号和 device/inode 以现有
  `u64_string` 表示。
- `original` 是现有带完整原 registrations 的 `ForeignOriginalStateReport`，只在这个
  SYSTEM 文件中存一次。`roots` 严格按 1..N、无重复。每项固定
  `level,registration,upstream_slot_id,metadata_member,path,device,inode,material`；
  `registration` 为完整 pool/disk/slot/binding_generation，首项 upstream 显式 null。
  `material.kind="lower"` 须有原 native `capsule_plan_digest`、经实读的
  `capsule_inventory_hash`、`original_scopes_hash`；`material.kind="source"` 仅用于
  最后一项，须有 `original_registration,source_evidence_digest,source_inventory_hash,
  seal_byte_length,seal_bytes_digest,journal_byte_length,journal_bytes_digest`。原文件摘要
  为逐字节 BLAKE3，不以对象/JSON 哈希代替；每种库存承诺用 `foreign-preserved-inventory`
  对完整规范化原 Inventory（含所有目录、文件路径、准确长度和原字节摘要）计算。
  `original_scopes_hash` 使用 `foreign-preserved-original-scopes` 对该根独立重验的完整
  `ForeignOriginalDatabasesReport` 计算。外层用途不同，目标、原物和共同证据不能换用。

**首次及冷重试。** 从既有准确请求、真实 FD、服务账户/凭据、原计划与注册提案重新获取
输入，拒绝调用者自行提交记录或所谓“verified=true”。同时持有原池和全链的排他资源，
检查全部现有登记、命名路径与实际身份、完整 Config/State、每个低级胶囊及三范围原像，
高级根须重放原生历史、逐物品验证库存和原 seal/journal。既有全链 2..32 级、且当前
根数+新增整链≤32，管理和 SYSTEM 不充当数据盘；全部现行 64 KiB helper/IPC 上限不变。
本次新 SYSTEM 文件上限也定为 64 KiB；超过预算在写前整批拒绝，不删除原字段。当前池
须有真实现有根，所以本片不宣称从入口可加入完整 32 新层。保留生产设备分离、FD/锁、
字节、目录、条目/组件预算及现有单原域和生命周期拒绝。新写先确认独立 SYSTEM 上的
实际分配及 inode；已存在时先原封重读/重新验证，完全相同才返回同一准备态，不因空间
下降重写。不同、损坏、根缺失或无独立证据，保留原文件，明确失败。写前/同步失败只可
按原操作重新检查，不推断成功；SYSTEM 无后续文件时绝不把现有池 fence 或启动现场 writer。

**未批准，不随这片实施：** 逐根真正非空 enrollment 的本地目录/崩溃所有权，任何对原
树的排除或写入，FULL/FILE/DISK/SYSTEM 的联合安装、票决、seal 移动/换位、混合 selector
冷恢复、活动 Config/locator/服务，新增分批/压缩公共信封，更大总根数（例如 64），
多原域及 D/undo 等生命周期。它们必须有单独的准确设计。仅一套新建规范；不设计历史
版本读取、双写、迁移或补字段。

**源代码验收边界：** 单根身份相同但物理根/祖先/原 seal 某字节改变；低级胶囊错 operation、
角色或范围；当前 State 同序号不同内容；已保存原计划后的缺根和空间变化；完整 4 层及
总根数 32/33、64 KiB 准确边界；前/后持久中断；任何失败不得改变 source/lowers 的字节
或启动权限。独立真实验收和原 900 秒 Backup 用例不由这份 ADR 替代。
