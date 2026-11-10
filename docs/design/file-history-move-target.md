# 跨盘移动的目标文件材料与决定前置

本片承接[联合恢复封套](file-history-move-recovery.md)和[共同投影](file-history-move-projection.md)，新增真正的目标文件材料准备。它不是只计算摘要：生产入口会在每个准确目标 route 盘复制全部准入 FULL 正文、保存完整逻辑图／历史审计／拟议源 D，并生成和独立读验目标文件 SQLite。**仍不发布 namespace，不写 ACCEPT，不返回 move 或 SMB rename 成功。**

## 准备与只读验存

`prepare_file_history_move_target` 要求原投影和已查看的 `FileHistoryMoveRecoveryDigest`。在创建目标材料之前，重新比较当前共同投影与原系统封套及全部配置元数据成员封套。没有任何一份、原 actor／operation／slot／配置／来源不符、已有写工作区或已取消时，不开始目标复制。不会顺手补齐缺失的恢复封套；须显式走原恢复入口。

每个目标盘使用当前 enabled registration、准确 pool／disk／slot／binding、持有的 DiskHandle 和 I/O gate。材料位于盘根的单槽私有 `file-history-move-target/`，而非目标在线文件容器。`target-intent.json` 绑定原恢复摘要、完整联合投影摘要、准确物理登记身份、目标新 file ID 和目标文件库记录摘要。初次独占创建；已有目录必须带完全相同的 intent。没有 intent 的残缺目录、不同 operation 或不同原投影均保留并拒绝，不能接管为新目标。

冻结 route 的每个成员都必须完成，不能只保主盘或少数副本。每份目标包含：

- `joint-metadata.json`：完整目标逻辑 HistoryManifest、映射后的全部旧 D/undo 前缀和同一 operation 的拟议源 D；规范字节与当前独立重建的联合投影完全一致。
- `.zxs-file.sqlite`：新目标文件的版本、全部分支、父边、路径／标签节点、标签、备注、合并及导入生命周期。文件内部库仍不携带外部路径和 placement 权威。文件身份、history、原拟用 sequence、严格 schema、全部有界行／关系及规范 records hash 均读回校验，并与**独立投影的期望摘要**比较，不只核数据库自身摘要。
- 每个新目标版本的 `version.json`、完整 `checksums.bin`、`content/<creation_name>` 和原复制归属记录。旧到新版本映射仍由原 operation 确定，所有目标路线使用同一集合。

文件库编码器新增仅供这种未发布投影使用的类型化审计输入。它要求新、未删除且没有自己的复制／删除 receipts 的目标身份，按原完整文件行验证器校验。**不伪造一次 copy 或 delete 收据来“骗过”数据库归属索引**，也不更改普通已发布文件库的来源规则。

## 完整正文及原复制归属

来源持有真实内容读租约，通过已有完整副本选择与验证入口取可用副本。每份目标使用显式完整字节复制及已有长度、完整 hash／分块清单校验；不调用硬链接、reflink 或 payload 去重。复制读以 64 KiB 为取消检查上限；原来源准入的已有验证及 OS I/O 仍可能延迟取消，不宣称所有底层 I/O 可立即打断。

新正文的实际复制 fd 一直持有到原复制归属记录落盘。记录固定 version ID、device／inode、长度、mtime／ctime；正文必须只有一个硬链接。最终完整读取目标自己的 metadata、checksums 和正文，再检查同一原 fd／命名 inode 和属性。相同字节的新 inode、多出来的硬链接或中途变化均不能沿用先前准备。

已存在的正文只有与完整原复制归属记录成对存在，并重新通过所有校验后，才可重用。正文存在但归属记录缺失，包括崩溃发生在二者之间时，**保留并报恢复所需，不把“字节相同”当成亲自完成独立复制的证据**。反方向缺正文也不自动重建旧 inode 证明。已有损坏正文／元数据不覆盖；需要将来的显式处置协议，不在本片自动删除或重新初始化。

这些 inode／属性记录约束本实现建立的副本及正常重试，不是防 root 篡改的签名、共识或存储后端物理 extent 证明。任意自摘要／进度字段都不取代完整目标字节及当前资源验证。

## 决定前的真实边界

复制循环结束后，再对**全部目标**独立做一次完整读验，不把循环进度当证据。`verify_file_history_move_target` 提供同样的纯只读验存，不创建目录或填补文件。冷重开须先按原恢复封套恢复同一投影，再验证原目标；不能在重开时重新采样时间、目标身份或 slot。

返回 `FileHistoryMoveTargetReport` 仅说明这次观察到的文件材料，包括每份版本数／FULL 字节数、目标路线和文件库逻辑摘要。它不是 `OperationResult`、可反序列化许可证或以后仍然有效的 payload lease，没有转换成 journal 决定的入口。每次后续使用仍需重验资源和当前来源。

**仍缺：**系统和盘根完整历史数据库的一致目标证明、move 永久收据与两端审计归属、物理材料进入真实候选的完整集合证明、N 成员 PREPARE／ACCEPT／FINALIZE／PUBLICATION 同一决定、双端读取句柄／目录页围栏及原决定冷恢复。当前主线盘根 current/history SQLite 是派生实现，不能把它的存在自动当作这些证明。本片不伪造拟议 State 的业务收据去生成所谓完整三库，也不先 copy 成功再 unlink。

决定前任何错误保持原源 namespace 和目标在线 namespace；已开始准备的错误明确报告原材料可能部分保留，继续同一个 operation。实际移动成功、源 SMB 隐藏、目标独立编辑和 ACCEPT 后恢复规则仍未开放。单槽、无自动取消／清槽及系统丢失恢复边界沿用原封套。

后继[共同决定输入](file-history-move-decision-inputs.md)把本片读验得到的每份原复制身份与源 D 后文件库、完整未接受 State 和三范围要求一起持久绑定。该后继仍不提供 ACCEPT 或 move 成功。

## 资源及验证状态

联合逻辑材料最多 12 MiB，目标文件库最多 65,536 行／12 MiB 行数据／16 MiB SQLite；intent 和 inode 记录各有固定读写上限。总 FULL 大小、checksum 长度和路径开销使用溢出检查，目标检查配置容量及实际剩余空间，并保留元数据恢复 reserve。复制按版本／目标顺序进行，只保留当前源读租约和有限文件句柄；没有预读全部正文、无界 worker 或同时打开全部历史。完整验存会额外读取全部目标字节，不宣称吞吐优化或恒定成本。

新增八项根 `test/` 集成测试源码：全部 route 的独立 FULL／文件库、旧 D/undo 落库但不伪造 receipt、原 operation 冷重验、缺归属记录不接纳、相同字节换 inode／硬链接拒绝、自摘要正确但内容不同的 SQLite 拒绝、恢复副本不足／取消不开始复制、来源变化不继续。均只针对临时池中的准备阶段。

按用户要求未运行代码审查、编译、格式化、测试或 CI。以上是生产代码和测试源码范围，不是执行通过或物理断电证明。完整 move、真实 SMB、三库及真实 journal 决定恢复仍未验收。
