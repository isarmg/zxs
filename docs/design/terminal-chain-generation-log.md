# 终态链原生 generation 日志与安装后冻结 Current

[真实 epoch 决定](terminal-chain-io-decision.md) · [原首笔 Publication](terminal-chain-first-publication.md) · [辅助工具](../components/helper.md) · [任务清单](../../TASKS.md)

基于真实父 `110b1740`，保留 Backup 普通 SYSTEM 数据库保留式安装与未解围栏边界。本片在原目标真实 epoch 决定后，把实际首槽日志放到准确 generation 地址，将完整原生对象放到普通不可变对象目录，再从这些安装后的实际字节消费冻结 Current。**不是活动 generation 完成：不写 `metadata/pool-membership/generation.json`，不替换 seal、bootstrap/head 或数据库，不消费 Stop，不创建活动 Engine/writer，也不返回完成 readmission/H03。**

按要求未审查、编译、测试、静态检查、CI 或浏览器验收，未运行辅助工具或操作实际系统/磁盘。五项 native 回归、一项 HTTP 输入与原路由安全边界扩展均只是未运行源码；只格式化明确修改的 Rust 文件。

## 真实路径与消费者

`TerminalManagement::prepare_chain_generation_log` 必须重新打开实际原目标、完整当前来源、Publication 和冻结 Current，并通过新的只验读消费者 `epoch_decision::verify_existing` 重读两根完整 epoch 计划/完成收据与原目标真实 native choice。该消费者不补缺件、不重新投票。保存过源报告或只有计划均不能安装。

安装位置全部由当前已验证的原 disk 与 core hash 计算，不接受客户端提供路径或 generation：

- `metadata/membership-generations/<core_hash>/journal.bin`：原目标实际首槽的完整三帧字节，独立复制，不移动原日志、不硬链接、不追加下一槽；
- 普通 `metadata/objects/<kind>/<prefix>/<hash>.json`：完整 core、State/snapshot/page、registry、history、Candidate、Finalize、Publication 和 seal；
- 同 generation 目录的 `chain-log-owner.json` 与 `chain-log-complete.json`：这个原安装请求的精确选择与读回完成证据，**不是通用活动 authority.json 或新盘 seal**。

原日志三帧逐条绑定原真实 ACCEPT/FINALIZE/Publication receipt，必须无 torn tail、无额外槽，原首槽上限 16 KiB。读取实际安装后的日志仍需逐字节相等及 native framing/receipt 校验。普通不可变对象逐个拒绝旧 flat 布局、损坏和异内容；同 hash 既存相同字节可以重复同步，不覆盖其它内容。

`PublishedProof::read_generation` 从普通对象地址重读完整 admitted State 和所有精确依赖，核 Publication/seal 和安装后日志。随后从完整 checkpoint/mask 与该真实证明构造 `CurrentNamespaces`，重新比较精确 registry。这是安装后的冻结读消费者，不是将它赋给运行态 Engine，也不授予内容写入、下一笔发布或 Stop 消费能力。

## 原操作恢复

SYSTEM 保存 `chain-generation-log-selected.json` 后才开始目标写入。选择绑定原 native manifest、完整实际三帧字节、已完成 epoch/计划/实际决定 hash，编码仍受 metadata 上限。generation 目录必须是本请求新建，或原选择下的未完成目录；来源未选择时不接管已有目录，即使为空。目录只允许本片固定名称和有界安全 staging，未知材料不忽略。

顺序是 SYSTEM 选择、原目标目录 owner、完整普通对象、实际 generation 日志、读回完整冻结 Current、目标完成 marker，最后 SYSTEM 完成收据。所有目标写入都保持原真实目录/writer owner，开始/完成时重验原选择、实际 epoch choice、Publication 和 bootstrap 仍停在原前驱。

- 没有目标完成 marker 的原请求可以明确续存缺失不可变对象；已有异内容保留拒绝，不按新请求绕过。
- 目标完成、SYSTEM 完成未写时，只能重读完整实际目标后补原 SYSTEM 收据，不能重写日志。
- 任一完成标记已出现后，缺日志、普通对象、owner 或对应完成标记都拒绝重建；不会用旧隔离材料自动修好后冒充原完整目标。
- 不提供取消、换盘、裁剪或清理；失败可能已经保存部分或全部材料，保留原 operation 和全部证据。

source-only 状态不打开目标。只有 source completed 才显示 `generation-log-preserved`，分项 `target_reverified_this_call` 和 `installed_current_reverified_this_call` 均为 false；显式原请求安装/读回成功才为 true。选定未完成返回 `generation-log-pending`。这两个状态都不表示活动服务可启动。

## Web、成本与下一步

独立 `POST /api/disks/terminal-chain-generation-log` 只接受原计划、停服确认和单独“材料日志不是活动 writer”确认。保留 root/0.0.0.0/无登录/无自启、Host/Origin/CSRF、凭据 revision、host/service worker 互斥、单任务和退出排空。成功或错误后清除旧操作表单，查询原状态再续作。拒绝客户端上传 journal/native objects、指定活动 authority、消费 Stop 或强行替换。

完整原生输入仍受既有 8192 对象、128 MiB 限额；普通地址安装沿用完整材料容量和临时写入预算，SYSTEM 另留实际 manifest 编码四倍加 1 MiB。已完成目标只核验，不要求再次分配完整副本。一次请求会重复完整源链、Publication 和已安装 State 校验，generation 目录最多 64 个条目；这是有界正确性成本，不宣称性能或掉电验证通过。

**仍缺专属通用活动 authority/可信冷启动 selector、完整 serving/数据库闭包以及新 epoch/Stop/活动 Current/writer 的共同切换。** 这些前置未齐时不能仅因日志已在 generation 目录就让普通恢复选举它，更不能先消费 Stop。现有 PortableAuthority、普通 Engine 发现和写者准入没有放宽。H03 外来多级链仍须完整低级链和最终权威，不能用本原保留盘路径代替。
