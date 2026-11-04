# 登记对象图与 Candidate 绑定：实验验证范围

状态：独立源码批次，尚未编译或执行测试。已有纯添加的 GitHub 结果不覆盖本批；本机仅作源码编辑、定向格式整理和差异检查。所有运行验证交由专用 GitHub 验证分支，不合入未完成的生命周期执行入口。

## 本批真实能力

- 从已经准入的 Engine 配置、当前已发布序号/快照及成员 authority 准备登记基线。输出只有内容寻址对象，没有安装、当前 head 更新、磁盘访问或服务能力。
- 历史拓扑、当前登记、当前投票者、Detached 审计分别分页。Disabled 保留登记；Detached 只从提议登记集合移除，原历史身份及拓扑保持。nonvoter 可参与两种提议，不增加投票数。
- `PreparedDiskRegistry::verify_with` 从调用方提供的准确字节验证完整分页图；`VerifiedDiskRegistry::verify_successor` 对确切前驱和规定差量复验。缺页、坏哈希或重哈希后的越权差量不会回退到内存、系统 JSON 或其他成员。
- 新的严格 `disk-lifecycle-candidate` 同时绑定 authority hash、registry root hash 和 view generation。Genesis 与既有 membership-generation Candidate 继续使用原表示，原字节不被重写。当前普通成员准入明确拒绝 lifecycle Candidate，不能仅因 voter ID 相等便接纳。
- Publication 绑定验证复用真实 Candidate/FINALIZE/PUBLICATION/seal 规则，并核对 registry 的准确 voter slot。此验证不查询主生命周期 operation 是否成功；发布自身使用独立 operation，不能提前消费尚未终结的主操作 ID。

`VerifiedDiskRegistry`、`VerifiedRegistryCandidate`、`VerifiedRegistryPublication` 都是窄证明类型，不含根目录或 I/O 能力。它们不选择当前权威，也不能转换为现有活动 Config。`CurrentDiskRegistry` 的普通预览行为保持不变。

## 明确尚未完成

1. Prepared capsule 和 occurrence mask 此批仅为发布前摘要引用，尚不验证其内容、作用域或本盘持久屏障。它们不包含未来 registry/authority/Publication。Detached schema 不允许同次 D3 Publication 或 Committed capsule 字段，避免哈希环。
2. 尚无真正生命周期 authority 与规定 D3 snapshot 的联合认证、D3 活动切换或从磁盘恢复当前 registry head。可搬运对象图和 seal 绑定验证不能替代这些条件。
3. registry 内 pending exit 能阻止在这份提议图上继续跨代，但它尚未安装进运行池的持久操作预约；不能宣称已阻止所有运行时 Add/Restore 越过未完成生命周期。
4. WriteProjection、Detached 内容 occurrence 投影、bootstrap 禁探测策略、全部长期读写与维护资源关闭、目标恢复入口、D4/D5 及永久主操作结果仍须接线。计划下线和注销执行入口继续拒绝，HTTP/Web 不开放成功路径。
5. 最后 voter 的退出仍缺终止权威。当前准备明确报未实现，不能把至少保留一个 voter 固化为最终产品限制。Disable/Detach 后重新启用、恢复登记以及完成后的下一次 registry 过渡也不在本批。

## 待 GitHub 执行的定向用例

`storage-engine --lib membership_registry_` 共 8 组：

- voter/nonvoter Disabled 保留登记和历史拓扑，纯准备不改变原 State、Config 或文件字节。
- Detached 仅引用前置依赖，未知 Publication/Committed 字段拒绝，未完成预约不能跨代。
- 准确前驱、源边界、代数、历史绑定、登记、票数和预约的重哈希篡改拒绝；绑定不匹配和代数溢出拒绝。
- 299 条布局向量产生多页，逐个缺失/损坏必要对象均拒绝。此向量只验证分页，不宣称 299 个真实目录已经准入。
- 新 Candidate 的准确 root/generation/authority/pool/voters 绑定、重复 voter 与未知字段拒绝；既有两种 Candidate 表示/哈希保持；当前活动准入拒绝新类型。
- 在独立测试目录真实追加日志帧验证 Publication/seal 绑定，不依赖主操作成功；漏 seal 回执、错 Candidate、整条回执使用外来 slot、提前占用主操作均拒绝。该向量不冒充完整 D3 snapshot 或 capsule 验收。
- 合法根路径投影不改变登记基线，nonvoter 登记与投票集合分离。
- 最后 voter 的阶段限制、错误 root 拒绝及无来源文件改写。

该前缀也会被已有 `membership_` 过滤覆盖。另保留普通 activation、journal/proof、history-package 及恢复过滤，验证新增严格格式未改变现有资料的读写。生产检查、严格 Clippy、格式/架构门禁按合批一次执行；本批不要求为每个编辑重复完整 CI。

## 成本与信任边界

设 D 为登记和历史目录记录数，编码、完整读取及规定差量比较是 O(D) 数据量、BTreeMap 操作为 O(D log D)，准备会复制目录级映射，不扫描版本/正文或全池历史。分页复用现有 metadata-map 分区、深度和单对象 `MAX_METADATA` 边界，没有增加任意历史条数上限。每次 reader 回调必须遵守现有有界对象读取契约；解析前再次检查收到字节的单对象上限。

本批生产方法无文件写入或 fsync，也不缩减现有发布同步屏障。大规模内存、性能和物理掉电尚未验收。公开输入是提议或待验材料，只有准确前驱认证和对象图核对通过后才返回相应窄证明，不能把内容寻址完整性当作用户授权、目标资源准入或当前服务权威。
