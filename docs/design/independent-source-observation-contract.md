# 独立来源候选观察：只读传输子契约

本说明记录基于 `b120463214a32b59ce1d26a867036b65c5782026` 设计的 `storage_contract::storage_independent_source` 单节点候选观察契约。它仍未成为候选匹配 HTTP/Web 接口；后续 held-directory adapter 及另一个完整来源图的只读产品接口见[产品只读预览](independent-source-product-preview.md)。没有登记、接纳、执行、正文读取、容量或写权入口。

这不是完整来源图 DTO。当前只表达一个被观察的来源节点、其逻辑根检查点、全部所需 pin，以及一次有限候选列表的观察结果。以后完整图投影仍需准确表达两端 publication 的多来源边；本切片不能代替那项工作。

## 1. 边界与保证

`IndependentSourceObservation` 的构造和反序列化只准入**结构一致的显示数据**。字段私有，消费者只能借用或复制已准入的结果；没有构造 engine `SourceNode`、`SourceCandidateBatch` 或登记类型的转换。

一个内部一致的伪造 JSON 仍可能被准入。这里没有读取磁盘、重新计算 publication 哈希、核验自有链、证明图成员或物理关系，也没有认证 HTTP 会话。实际发布者必须来自真实 verifier，HTTP/Web 必须保留当前会话、scope 和请求拥有者边界。

中间的 `IndependentCheckpoint`、`IndependentNodeReference`、`CandidateRow` 是可构造的字段值，不是独立验证器的私有证明。只有外层观察类型校验跨字段一致性；它也仍只是显示类型。

## 2. 数据不能与旧协议混用

- `protocol` 只接受字符串 `independent_source_observation`。旧 foreign preview 的 candidate/publication/authority 三摘要结构不能解码为本契约。
- 完整身份保留 pool、disk、slot、非零 binding generation、incarnation、role。token 使用独立格式既有的 1..=96 字节 ASCII 字母/数字/下划线/连字符规则，不要求旧池 `pool_...` UUID 或 `P01` 槽位规则。
- `PublicationId` 和 `Incarnation` 是不同的显示字段类型，都接受64个小写十六进制字符，但不承诺已计算或验证摘要。没有把它们包装成正文 `ContentHash`，也没有自动转换。字段长度相同不代表哈希域相同。
- generation 和 sequence 使用 canonical 十进制字符串，避免 JavaScript number 损失。generation 不得为零；sequence 为零当且仅当 publication 显式为 null。缺失 publication 字段不是 null。
- 单节点 pin 集必须严格升序且无重复。它保留全部 pin，不挑最大 sequence、不按身份丢掉不同 publication。

根节点只能持有其准确根 publication，空根只有自身无 pin 节点。祖先角色必须低于根角色；与根相邻的角色按真实 verifier 规则拒绝相同 disk token。三级根到一级祖先是间接关系，不能把直接边的同 disk token 限制错误推广到它。

这些检查只能排除明显不可能的描述，不能证明这个祖先实际在根图中。逻辑根字段相等也不等于同一次 `SourceNode` Arc 捕获。

## 3. 状态与候选计数

逐候选状态只有：

- IdentityMismatch：完整身份不同。
- UnpinnedNode：完整身份相同，来源节点没有 pin。
- MissingPinnedCheckpoints：非空、唯一、规范排序的缺失 pin 子集，不得把当前 head 自称为缺失；sequence 至少能容纳所有声称已满足的 pin。空候选链须报告全部 pin 缺失。
- SatisfiesPinnedCheckpoints：完整身份相同、来源 pin 非空、候选有 head 且 sequence 能容纳全部 pin。此字段仍是服务器的观察报告，JSON 解码不重建 own-chain。

批次状态只有 NotObserved、IncompleteObservation、Ambiguous。空输入必须是 NotObserved 且物理计数为零。非空的0或1个独立匹配保持 IncompleteObservation；至少2个才允许 Ambiguous。没有 Complete、Unique、NotFound、Online、容量或写能力分支。

保留每个输入行，包括别名和拒绝理由。不同的 checkpoint/identity/status 事实不能属于同一个未变化物理组，因此不同匹配事实数量不大于 matching count，不同拒绝事实数量不大于 physical-minus-matching。相同事实仍可能来自别名或真实独立克隆，**不能从字段相等证明同一物理目录**。

这里没有序列化 dev/inode 作为永久盘身份，也没有因候选顺序决定选择或默认关联。完整物理关系仍由服务器当前持有目录的捕获与真实 batch verifier 建立。

## 4. 资源与错误

当前显示子契约有独立上限：1 MiB 输入字节、128个候选行、每个节点/缺失集合最多4096个 pin、总计最多10,000个显示 pin 引用。最后一个上限计算 node pins 加各行 missing pins；它不是核心10,000 own-chain条目预算，也不放宽核心16 MiB metadata 或256深度等限制。

`decode_observation` 在解析前检查字节预算，复用 `strict_json` 拒绝重复字段，再执行类型准入。通用 `Deserialize` 只提供字段和集合结构限制；直接使用它的调用方仍须在外层限制原始字节，不能绕开 HTTP/body 边界后宣称具备1 MiB检查。

数组反序列化有单集合限制，准入不截断、不删错行来伪装完整结果。未知字段在各结构和 internally-tagged unit 状态中都被拒绝，防止 `not_observed` 等单位分支吞掉额外 online/writable 字段。超出预算或任何结构矛盾直接失败，不返回部分成功类型。

原始JSON错误和字段准入错误分别保留来源。这里不承载实际 verifier 的 I/O/损坏错误包；未来 HTTP adapter 必须另行保留领域错误的输入位置、阶段、错误码和请求关联，不能把失败转成 NotObserved。

时间成本为有界字段检查、pin 的二分子集检查和候选事实集合插入；设候选数R、pin引用数P，除字符串比较成本外为 `O(P log MAX_NODE_PINS + R log R)`，内存为有界显示字段和 `O(R)` 借用索引。没有 I/O、锁、fsync、持久化或新依赖。

## 5. HTTP 401 与旧响应顺序

该观察类型只能处理成功 body，不能替代 HTTP response envelope、认证或面板生命周期。401/session_expired 不是候选状态。

未来接线必须保留现有 Web 顺序：

1. 凭据层按 credential generation 处理 HTTP 401，不让旧登录撤销新登录。
2. `ui::accept_read_response` 先检查当前 session epoch，并处理同会话401。
3. 再按面板 owner、请求序列和请求侧来源捕获丢弃旧成功或普通领域错误。

准备中的18项 engine生命周期用例只约束真实 verifier 的进程内响应，不覆盖上述 HTTP 决定。新35项契约测试拒绝把认证状态塞进候选枚举，也不构成真正401、登录切换、fetch/abort或浏览器结果的验收。

单节点候选观察仍待接入真实 HTTP 请求及来源选择生命周期。安全 held FD producer、完整图 wire、只读 HTTP admission/scope/背压和向导展示已由上述后续切片实现，不等同于候选对应或唯一性判断完成。取消接收不等于服务器 I/O 停止，元数据匹配不等于当前在线。不得从这个显示类型进入写入、SMB、默认真实盘选择或接纳列表。

## 6. 准备与验证状态

根测试 `test/contract/unit/storage_independent_source.rs` 当前有35项，使用**符号JSON fixture**，覆盖完整身份、数值精度、pin/计数矛盾、别名/克隆语义、字节/集合预算及等号边界、未知/重复字段、单位变体、旧协议和认证状态拒绝。它们不是18项真实临时盘生命周期测试，也不是磁盘哈希或HTTP集成证据。

初稿阶段未运行验证。后续产品切片已运行本模块35项测试、完整图7项测试及相关源码检查，准确范围见上述产品文档与交付收据；早期窄 reader harness 和编译拒绝探针也不能替代完整应用及浏览器验收。候选对应、角色转换和迁入写入仍未实现，不能称为外来三级盘功能全部完成。
