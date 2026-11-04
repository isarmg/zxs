# 登记对象图与 Candidate 绑定：实验验证范围

状态：前一基线 `8b23ead0` 的 [GitHub 验证 41832402178](https://github.com/isarmg/zxs/actions/runs/41832402178) 已通过，覆盖原 8 组 `membership_registry_` 用例。本批按最新删除语义去掉恢复备份字段并新增用例，尚未编译或执行测试；前一结果不覆盖本次更改。本机仅作源码编辑、定向格式整理和差异检查，运行验证交由专用 GitHub 验证分支。生命周期执行入口仍未完成。

## 本批真实能力

- 从已经准入的 Engine 配置、当前已发布序号/快照及成员 authority 准备登记基线。输出只有内容寻址对象，没有安装、当前 head 更新、磁盘访问或服务能力。
- 历史拓扑、当前登记、当前投票者、Detached 审计分别分页。Disabled 保留登记；Detached 只从提议登记集合移除，原历史身份及拓扑保持。nonvoter 可参与两种提议，不增加投票数。元数据成员是登记的数据存储目录角色，不是操作系统设备；不同非重叠目录可以位于同一文件系统。
- `PreparedDiskRegistry::verify_with` 从调用方提供的准确字节验证完整分页图；`VerifiedDiskRegistry::verify_successor` 对确切前驱和规定差量复验。缺页、坏哈希或重哈希后的越权差量不会回退到内存、系统 JSON 或其他成员。
- Detach effect 只有 `occurrence_mask_hash`，没有目标根路径、恢复备份或 Prepared/Committed capsule 字段；旧字段严格拒绝，不提供兼容或迁移。当前尚无已激活的 registry，因此不涉及已部署 registry 的迁移。
- 新的严格 `disk-lifecycle-candidate` 同时绑定 authority hash、registry root hash 和 view generation。Genesis 与既有 membership-generation Candidate 继续使用原表示，原字节不被重写。当前普通成员准入明确拒绝 lifecycle Candidate，不能仅因 voter ID 相等便接纳。
- Publication 绑定验证复用真实 Candidate/FINALIZE/PUBLICATION/seal 规则，并核对 registry 的准确 voter slot。此验证不查询主生命周期 operation 是否成功；发布自身使用独立 operation，不能提前消费尚未终结的主操作 ID。

`VerifiedDiskRegistry`、`VerifiedRegistryCandidate`、`VerifiedRegistryPublication` 都是窄证明类型，不含根目录或 I/O 能力。它们不选择当前权威，也不能转换为现有活动 Config。`CurrentDiskRegistry` 的普通预览行为保持不变。

## 删除与下线的目标语义

删除直接移除 ZXS 当前登记，以及只在目标目录持有的内容在当前文件列表和可写索引中的 occurrence；不擦除目标目录原文件，不创建或要求可恢复备份、目标根恢复文件或 capsule。删除不以目标可写或先排空其最后正文副本为条件。原字节留在目录中，不等于这些历史正文仍可在线访问。

其他目录持有的共享内容、必要的完整控制历史、原始 Candidate/回执/永久操作结果和成员权威必须保留，不能因移除一个目录而全局删除共同引用。固定旧/新 Q、权威完整性及未决操作约束不取消；无法证明安全的控制交接不能返回成功。离线/下线则保留登记和逻辑数据，禁止目标 I/O，不应用删除的 occurrence 移除语义。

## 明确尚未完成

1. occurrence mask 此批仅为发布前摘要引用，尚不验证其内容和作用域；当前不会实际改变文件列表或可写索引。引用不包含未来 registry/authority/Publication，后置完成证据不得回填已哈希根。
2. 尚无真正生命周期 authority 与规定 D3 snapshot 的联合认证、D3 活动切换或从磁盘恢复当前 registry head。可搬运对象图和 seal 绑定验证不能替代这些条件。
3. registry 内 pending exit 能阻止在这份提议图上继续跨代，但它尚未安装进运行池的持久操作预约；不能宣称已阻止所有运行时 Add/Restore 越过未完成生命周期。
4. WriteProjection、Detached 内容 occurrence 投影、bootstrap 禁探测策略、全部长期读写与维护资源关闭及永久主操作结果仍须接线。计划下线和注销执行入口继续拒绝，HTTP/Web 不开放成功路径。删除不再有目标恢复文件阶段，移除该要求不代表剩余协议已经闭合。
5. 最后 voter 的退出仍缺终止权威。当前准备明确报未实现，不能把至少保留一个 voter 固化为最终产品限制。Disable 后重新启用和完成后的下一次 registry 过渡也不在本批；删除不承诺可通过恢复备份还原登记。

## 待 GitHub 执行的定向用例

`storage-engine --lib membership_registry_` 共 10 组：

- voter/nonvoter Disabled 保留登记和历史拓扑，纯准备不改变原 State、Config 或文件字节。
- Detached 仅引用前置依赖，未知 Publication/Prepared/Committed 字段拒绝，未完成预约不能跨代。
- Detach 请求只携带 occurrence mask 即可准备；旧恢复备份字段（包括 null）、缺失或非法 mask 拒绝。
- 根路径投影指向不存在目录、释放 Engine 后，voter/nonvoter 的纯 Detach 准备与对象图验证仍可完成，不读取或写入目标恢复文件，不改变原目录字节。这不证明活动删除已经支持离线目标。
- 准确前驱、源边界、代数、历史绑定、登记、票数和预约的重哈希篡改拒绝；绑定不匹配和代数溢出拒绝。
- 299 条布局向量产生多页，逐个缺失/损坏必要对象均拒绝。此向量只验证分页，不宣称 299 个真实目录已经准入。
- 新 Candidate 的准确 root/generation/authority/pool/voters 绑定、重复 voter 与未知字段拒绝；既有两种 Candidate 表示/哈希保持；当前活动准入拒绝新类型。
- 在独立测试目录真实追加日志帧验证 Publication/seal 绑定，不依赖主操作成功；漏 seal 回执、错 Candidate、整条回执使用外来 slot、提前占用主操作均拒绝。该向量不冒充完整 D3 snapshot 或生命周期激活验收。
- 合法根路径投影不改变登记基线，nonvoter 登记与投票集合分离。
- 最后 voter 的阶段限制、错误 root 拒绝及无来源文件改写。

该前缀也会被已有 `membership_` 过滤覆盖。另保留普通 activation、journal/proof、history-package 及恢复过滤，验证新增严格格式未改变现有资料的读写。生产检查、严格 Clippy、格式/架构门禁按合批一次执行；本批不要求为每个编辑重复完整 CI。

## 成本与信任边界

设 D 为登记和历史目录记录数，编码、完整读取及规定差量比较是 O(D) 数据量、BTreeMap 操作为 O(D log D)，准备会复制目录级映射，不扫描版本/正文或全池历史。分页复用现有 metadata-map 分区、深度和单对象 `MAX_METADATA` 边界，没有增加任意历史条数上限。每次 reader 回调必须遵守现有有界对象读取契约；解析前再次检查收到字节的单对象上限。

本批生产方法无文件写入或 fsync，也不缩减现有发布同步屏障。大规模内存、性能和物理掉电尚未验收。公开输入是提议或待验材料，只有准确前驱认证和对象图核对通过后才返回相应窄证明，不能把内容寻址完整性当作用户授权、目标资源准入或当前服务权威。
