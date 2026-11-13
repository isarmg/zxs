# 冻结前的手动 SMB 工作材料恢复（独立后继）

基于不可变候选 `310e0ff96064b7bfa06b6157e15511dce9f28a85`，在独立分支开发。
本片不属于该候选的验证范围；代码和回归已写，尚未远端编译/运行，没有本机编译或测试。

手动 writer 从 Open 开始将已有、已计费的工作记录写为 strict `smb-manual-working-before-save-v1`。
记录持有原 caller entry/FileId/context、原 DraftBase、代数、主体、operation 与同目录 live 名称。
阶段明确区分 Working 与 Frozen-reference；关闭/脱离标记仅是诊断，不是成功收据。
自动 writer 保留原行为；旧 `smb-protocol-work` 不能被新 Working reader 接管或迁移。

新增 `RecoverManualWorking` 显式原 operation 与工作记录 basename。helper 仅传
`--working-record NAME`，不直接读 spool。服务器用持有的 private directory 限定一层 entry，
严格验证记录与 payload basename、operation、principal 一致，不遍历猜测工作材料。

只有 Working、尚无完整 Frozen 记录时，query 返回 unfrozen-working，不冒充原 Save 收据。
显式 resume 才冻结当前保留 live 字节，并按原 context/base/generation 严格保存；这不是对一个
从未存在的完整请求重放。若完整 Frozen 已存在，则只能沿原请求查询/重放，不能重采 live 字节。
Frozen-reference 与完整 Frozen 冲突则拒绝；完整 Frozen 材料缺失时，只允许按保留的完整 revision
查原发布收据，不用最新 live bytes 重造请求。

已新增两个真实单 primary fresh-format fixtures：首次 Flush 前断连/服务重启后 query 不写、显式
resume 经 authenticated framed RPC 冻结并保存保留字节，HEAD 不移动；旧格式/路径穿越拒绝且材料
原字节保持。helper 新增明确 locator 的只读默认解析回归。这些尚待远端执行。

partial-freeze 修复、持久证据显式清理与后继 live-byte 接回可写句柄仍未在本片完成。
部分冻结正文继续占预算且不覆盖、不自动删除，不把保留材料描述成已可无损完成所有恢复。

## 后继严格性修正（仍未执行测试）

`406662311` 的冷 Working 路径曾未在保存前推进原工作记录，不能把其 Working 字节当成完整的
“尚未送达”证明。新记录因此使用 `smb-manual-working-before-save-v1`，旧名称严格拒绝并原样保留，
不迁移。固定顺序为：冻结正文同步 → 完整 Frozen 记录同步 → Working 的 Frozen-reference 同步
→ 核心 Save。任一前置失败都不继续 Save。显式 resume 与活动 writer 更新同一工作记录时共用
私有 advisory lock；query 不创建或取得该锁，不改变记录。

没有完整 Frozen 记录且仍是新 Working 阶段时，先查询原 operation。任何已知结果（包括未决结果）
都不能证明可以重新冻结；缺完整证据时明确拒绝。只有明确 NotFound、当前读权限成立且没有原冻结
正文或 budget owner 残留，才保留首次显式冻结路径。部分正文/预算残留不按哈希认领，也不改名、
覆盖或清理。Frozen-reference 已存在但完整材料缺失时，仅可凭原完整 revision 查询原收据，
找不到收据就拒绝，不读取最新 FD 来重建请求。

另增四项实际引擎回归：冷保存先写引用、完整冻结材料缺失仍只查原结果；部分正文或预算残留保留；
已发布 operation 配旧 Working 外观拒绝重新包装；冲突 revision 和旧模糊格式原字节保留。
全部断言引擎状态及 spool 原字节不变。测试代码已写，尚未编译或执行，不能当作通过证据。

不确定结果覆盖在测试 Backend 返回边界注入 InDoubt、RecoveryRequired 与查询连接失败，保持真实
引擎原收据和状态不变，验证这些结果均不能变成 NotFound 许可；不宣称这模拟了核心 journal 崩溃。
resume 只有先严格读入且原主体/op 匹配的工作记录才可取得锁，随后在锁下重新准入；未知 locator
或旧格式不能借失败请求创建无限 lock 文件。query 始终只是严格读取和查询，不创建锁文件。

所有未发布 Save 路径现在共用 Frozen-reference 准入，包括直接 Frozen RPC；完整副本已同步但引用
尚未同步的崩溃切点不能绕屏障提交，必须由显式 Working resume 完成相同 revision 的引用后继续。
另加回归验证该切点；cold resume 成功后仍停留旧内存状态的活句柄，其下一 Dirty 也不能把同一
operation 的持久 Frozen-reference 降级成 Working。阶段更新在记录锁内保持单调。

本片合计新增六条安全回归，含真实引擎和磁盘材料，以及明确标注的 Backend 结果不确定注入。
没有新增自动删除、改名或 partial-body 认领。所有运行结果仍待独立 CI 验证。

未送达但已有完整 Frozen-reference 的正文篡改/丢失另有回归：即使当前 live FD 恰好等于篡改正文，
也只能查询原结果，resume 仍按原 revision 的 hash/size 验证并拒绝，不采纳当前字节。

## Successful frozen-body retention integration (unverified successor)

Normal manual Flush and explicit recovery Resume retire the redundant private frozen
body only after exact permanent-result verification. Query remains read-only: even a
known receipt does not authorize an unlink or budget update. Retirement rereads the
strict original record, compares the complete request, and rechecks the permanent
receipt under the budget ledger lock before validating and unlinking the held private
body. The engine draft and live writable FD are not retirement targets.

Directory sync precedes durable budget-owner reduction. The query record and its
64 KiB metadata charge remain after body removal; repeated successful saves therefore
release body capacity, not unlimited operation metadata. Unknown results, changed
bytes, malformed records, and unexpected namespace evidence preserve their material
and conservative charge. An interruption after unlink can retry from the exact
receipt and retained record, without reconstructing the removed body.

The private directory and service-local lock exclude service-internal replacement;
verify followed by unlink is not an atomic inode-conditioned unlink against a hostile
same-UID process or administrator. No stronger filesystem guarantee is claimed.

The successor now registers the retention regression child under
`smb_rpc::tests::manual_recovery_tests::`, alongside Working and Frozen safety tests.
These changes have not been compiled or executed locally; GitHub execution is required
before admission into a release candidate. They do not inherit the frozen candidate's
validation evidence.
