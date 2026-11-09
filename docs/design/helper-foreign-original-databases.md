# 外来完整链：原完整 State 与三范围数据库

[前置整批实物材料](helper-foreign-complete-chain-material.md) · [磁盘职责](helper-disk-management.md) · [任务清单](../../TASKS.md)

候选从 `ac6e1096` 开发，本次提交父基线 `b7f6c95a`；保全旧 `695bf` 候选后迁移到已发布的 helper → 同版内嵌 ZXS 有界管道。原生 State/数据库推导在 Engine，root 协调和服务 UID worker 在 `src/server/src/offline/foreign_chain`；helper 仅承担独立 Web/CSRF/任务槽及传输，不增加 helper → server/contract 生产边。本片尚为本地开发，实际验证见末节；未操作真实磁盘、系统、服务或密码。

## 先完整原链，再推导数据库

只从原 complete-chain operation 进入，要求原 root 整批材料回执存在、每个必需低级目标的原 capsule 仍完整。二级不能少一级，三级不能少一级或二级。沿用原来源/账户/凭据/当前 Config/全部目录身份与停写确认，不重选目录、不另开 Add、不先纳入高层或低层。

每个目标独立打开自己真实的原 capsule、原父目录 reservation 和物理 binding，完整重验原始库存、来源证据和历史投影；不会读取另一个健康目标来替代本目标的缺件。只读 published owner 不创建 stage、reservation 或 marker。

### 原 native State 来源

从该目标内 `source` 的原 journal 和 metadata 对象读取完整固定 genesis 发布前缀：

1. 每笔候选必须符合原 Config 的真实成员，sequence 从 1 连续、previous snapshot 精确相接，原本地 journal 的 Prepare/Accept/Finalize/Publication 与对应 receipt 匹配。
2. 原 PublicationSeal、来源登记及本地 Publication frame 按现有原生校验器重验，完整 Snapshot 的所有 map、control auth 和 events 从其认证对象闭包恢复，再经过普通 `AdmittedState` 边界，而不是把 SQLite 行拼成 State。
3. 原 Config 的完整登记身份列表与摘要一并保留，不凭数量或路径推断原盘身份。每笔原 operation/result/principal 必须与发布一致。前一 State 的永久 operations/principals/events 必须全部原样保留，新笔只增加自己的准确原事件；不重写主体、不丢旧操作、不中止历史后伪造新 genesis。
4. 该 State 的全部历史图必须等于候选绑定的历史根。结束时检查准确原来源检查点和不变锚点。

当前只支持最多 32 个固定原盘、2,048 笔完整 genesis 发布。generation、registry/成员变化、降级恢复、reprotection、未发布尾部、缺全 State 页或不完整前缀均保留拒绝。该证据说明**所选来源本地已经发布的原前缀**，不证明别处没有更新的发布，更不是当前池的投票或接纳权限。

### 实际数据库镜像

在完整原生前缀通过后，复用现有纯投影编码器生成：

- 原 SYSTEM 当前库，含原完整 State 的系统目录、永久 operations/principals/events；完整历史时点证据仍在原 native 前缀中，不伪称这个当前 SQLite 已变成所有历史 State 的统一活动数据库。
- 每个**原登记身份**的盘根 current/history，从每笔原 State 流式折叠，包含原版本、路径和已支持的递归目录副本；不是把旧盘身份换成新低级槽位。
- 全部原 FILE/目录内部数据库，保留原文件、历史身份和已支持的审计闭包。共享编码器对不支持的 graph-only lifecycle 来源仍明确拒绝。

输出地址为隔离的 `system/index.sqlite`、`original-disks/<原disk_id>/...` 和 `histories/...`，不使用正常活动盘的安装位置。原来源缺失正文仍保持原缺失事实；元数据镜像正确不代表正文恢复或可读取，不能复活 RETIRED 节点。

## 计划、原请求续作与完成

独立 Web 在查询到原材料批次完成后提供：

- `POST /api/disks/foreign-chain-databases-preview`：只依原 operation/request digest 重建每个原目标的预期数据库与现状；返回原 State、前缀、数据库清单摘要和各目标 `not_started` / `preparing` / `verified`。
- `POST /api/disks/foreign-chain-databases-prepare`：明确确认 `prepare-original-three-scope-databases-for-entire-chain`，必须与前次整批原数据库预览完全相同。

每个低级目标的预期 original State、前缀及数据库清单须一致，自己的 capsule digest/层级仍分别匹配原完整链。服务账户在第一个目标写入前检查**全部**目标和预期摘要，不以单目标成功代表整批成功。

root 首先保存 `.zxs-helper-foreign-chain-<operation>-databases/plan.json`。各目标在自己原父目录下保存独立 reservation：

```text
<lower>/.zxs-foreign-databases-<operation>.json
<lower>/.zxs-foreign-databases-<operation>-complete.json
<lower>/foreign-chain-databases/
  binding.json
  intent.json
  databases/...
  complete.json
  residue/...                 # 仅保留可识别的原子写暂存残留
```

父 reservation 先于子目录创建；子目录以真实 device/inode 与完整原 intent 绑定。SQLite 采用不覆盖写入，按实际文件与目录同步，随后从该目标自己的原 native 前缀再次推导期望库存，对真实全部镜像逐字节承诺验证。没有以保留 manifest 的自摘要代替原生推导，也不将一份原盘库复制三次冒充三范围。

partial 只按同一原 intent 续作。已存在的异内容、未知目录、物理替换、缺原生证据、矛盾主体或新格式不匹配均拒绝，不删除、降级、修权限或隐式迁移。单个目标已有 `complete.json` 后缺 SQLite 也不能在 retry 中补造。各目标全量同步读验后，还在原父目录保存绑定数据库目录 device/inode 的不可变完成记录；整库目录丢失也不能误当首次 partial 重新创建。内部 complete 已存在但父完成记录尚未持久化时，只能先完整重验原库再补最后阶段，不重建缺镜像。整批 root 完成记录只在每个原目标真实完成、同步并独立重验后保存；已有整批完成记录时任何目标非 verified 都拒绝写入。

Web 首发前保存并读回原 operation 选择器，可下载完整数据库计划；断连或响应未知保留原编号和数据库摘要，刷新后显式查询，不自动重新执行。当前 helper 仍要求原来源和全部原根在原位置保持不变，即使单个 native capsule 已能独立读取，也不扩大为任意来源失联后的重新绑定。

## 全链新登记身份提案

`POST /api/disks/foreign-chain-identity-preview` 只读取原 operation/request digest。root 必须先找到完全匹配的原数据库 intent 和完成记录；服务 UID worker 仍重新打开每个原 capsule，重建各自原生期望并校验本地三范围镜像，检查全部原 State/清单一致，且准确来源身份仍属于原 Config。任何目标缺件或非 verified 均拒绝，不使用完成 marker 或健康同伴替代实读。

提案同时覆盖所有真实低级根和原高级根，以原整批请求、当前池、层级、槽位及 device/inode 生成独立新 disk ID，并保留原来源完整登记身份。拒绝当前/历史/原盘 ID 或当前槽冲突；复用普通 `ValidatedConfig` 检查完整新链拓扑，当前 Config 的其它成员和参数不变。相同原绑定产生相同提案，换目录、账户、池基线或原操作就不是同一提案。现池仍须全部 Enabled，历史停用/退役根不能重用。

新身份只是**全链登记提案**，不是原盘一对一重命名，不把来源的原盘数据库贴上新标签；尚未映射文件/仓库/永久操作身份到联合 State。提案不保存新的活动 Config、seal、selector 或数据库，不执行 Add，不 reserve 新成员，不消费 Stop、启 writer 或服务。Web 只在整批原数据库完成后提供提案和摘要下载，没有“接纳/启用”按钮；响应明确 `target_identity_proposal_only=true`，其它激活字段保持 false。该查询打开既有 ManagedPool 时仍可能执行普通 Engine 恢复，不能把它说成对现池所有字节绝对只读。

## 返回值不授予正式接纳

`original_database_scopes_prepared=true` 只对应上述整批原身份镜像。以下始终 false：

- `target_identity_databases_installed`
- `foreign_admitted`
- `changes_existing_pool_membership`
- `starts_service`
- `complete_installation_ready`

正式外来纳入仍缺**全链提案到完整目标 State/对象身份的实际映射、当前池与外来历史的联合 State、联合三范围数据库、全部低级成员共同 authority/epoch/Publication、Config/locator/Stop/fence/serving 切换**及相应原请求冷恢复。本片不调用普通 Add，不写活动 seal/Config/selector，不开启 writer，不使用 backup writer，不把救援库或旧身份库当作新目标权威。只有这些剩余条件真实闭合后，才能进一步实现纳入。

## 资源与实际验证

原始检查仍受 capsule/foreign preview 的元数据、日志、版本、正文预算约束，前缀重放保留相邻 State，不缓存所有历史 State。本片镜像集合最多 4,161 个数据库、192 MiB，原盘和对象编码器还有各自更紧的预算；SQLite、投影和原生状态会有同时驻留的额外有界内存，不能将镜像字节上限说成进程内存上限。

每目标 database intent 最多 2 MiB，root/IPC 仍为 64 KiB，每个数据库准备根最多 64 个顶层保留项。重复检查原生来源与完整镜像是实际 I/O 成本，预览/查询不是常数时间。未减少校验、同步、owner 或退出排空要求。本轮实际结果：

- 通过：原架构策略、来源 `verify --metadata`、Foundation 消费者索引与依赖文档检查；四个受影响第一方包的格式检查在最后错误转换修改前通过，修改后 `git diff --check` 通过；独立 Web 的实际 JS handler 在 DOM/transport doubles 下覆盖整批门禁、双击单请求、旧提案失效、写前持久选择器失败、中断保留与刷新不自动重放。没有真实浏览器验收。
- 未通过：生产组合 check 初次发现旧候选 `canonical_json` 的 contract→engine 错误类型转换缺失，已补 `Ok(...?)`。后一次在 helper 报缺新 Operation 枚举；源码的三个新枚举及 Engine 重导出均存在，但共享 target 的已存 contract 元数据都没有新枚举，本次未重建 contract。“共享旧缓存导致”仅为未经复验的原因推测，并非已确认根因或已修复结论。最后一次生产 check 未通过；本次对齐 `b7f6c95a` 的组合未编译、未测试、未运行 CI，既有轻量结果也不覆盖本组合。
- 停止/未执行：收到用户“本地也不再编译，直接提交”后，没有继续编译、清缓存重试或测试构建。新增五项 native 集成、一项 native unit、八项离线模型、两项请求门禁测试只有源码；Rust 测试、Clippy、真实嵌入进程链、磁盘/主机操作及本候选 GitHub CI 均未执行。没有沿用旧 CI 结论。

## 后续对象命名材料

后继的[完整对象命名准备](helper-foreign-object-identities.md) 在上述全部原库验证后绑定现池完整 State，并将确定命名表单独持久化到每个真实低级根。该命名表仍不改写对象引用、不生成联合 State 或活动目标数据库，不改变本页关于正式接纳未完成的结论。
