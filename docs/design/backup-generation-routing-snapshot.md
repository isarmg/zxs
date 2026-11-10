# Backup generation 的未发布路由快照

状态：基于真实 `adedd04c` 的生产代码阶段。本提交未审查、编译、测试、静态检查或 CI，未操作真实磁盘。它仍不是最终 generation、迁移成功、活动 writer 或源围栏解除。

## 实际新增链路

原始所有成员已经在固定 native 槽选择同一个 Backup registry/placement 后，原后台 worker 继续执行：

1. 重新观察全部原成员的真实决定，重新独立验读每个目标的原生完整源 State、正文、对象数据库、实体镜像与 writer routes。
2. 从每个目标自己的源快照推导同一个**未发布 routing State**，不从另一目标补齐缺页或取代验证。它只改 `placements`、`file_backup_paths` 和 `topology_generation` 三个字段。所有旧 operations/principals/events、D/undo、退役回执、正文状态、目录/文件节点、分支、标签、备注、私有 native 字段均保留。
3. 每个文件全部保留版本使用完整原低级链和所有明确选择的新目标，不猜副本数，不只迁移 HEAD。目标集合按层级/slot 规范排序，分叉节点不是前一兄弟的下级。直接上游名前缀来自已决定的每个目标祖先链。
4. 真正调用 native snapshot 编码器，生成全部 map root/page/control-auth 对象，在各目标 `backup-generation-inputs/` 中逐文件 no-clobber 写入并读回。此处不是只有摘要的新模型，也不是缺永久操作的 portable HistoryManifest。
5. 全部目标的输入文件读回后才分别写 `generation-inputs-prepared.json`。进度新增 `GenerationInputsPrepared`，不写 PUBLISHED、原生首个 Candidate、PublicationSeal、`disk.json`、普通 `metadata/` generation marker，也不替换 SYSTEM 配置。

生成的路由快照故意保留原 `sequence`，不伪造本次操作结果或事件。`intended_first_sequence` 只是从真实原 fence 加一得到的后继槽。最终 publisher 必须在真正 Backup authority 下添加准确原操作回执、共同 registry/placement 和全目标确认，再形成实际首个 Candidate；不能把这里的 snapshot hash 直接当成那个已发布快照。

原本不用的旧 backup path 记录保持不变，旧终端盘只从**这份未发布拟议路由**移出。原盘的 bytes、native journal、决定、数据库与当前 Engine State 全部不动。

## 原决定恢复

- 正常冷开依旧先恢复真实原 State/fence；原持久任务只补相同的缺失材料。名字、设备/inode、源边界、决定和全目标绑定变化均拒绝，未知或不同 bytes 保留。
- `BackupDecidedTargets::inspect_generation_inputs` 是全目标只读观察。
- 新 `resume_generation_preparation(&mut self)` 是显式离线写入：调用者必须授权目标材料续作。它只在先前 `open_decided_backup_targets` 已通过独立保存的决定摘要、完整原目标集合及源证明认证后可用。它不访问旧源或 SYSTEM、不重新投票、不借自摘要接纳外来盘，只补原 writer routes 与本阶段材料。源掉线并不迫使它丢弃已决定且完整的目标数据。
- 已存在运行态 `disk.json` 或 `metadata/` 的目标仍拒绝这个冻结能力，不能绕过活动生命周期/退役门禁。

## 支持范围与硬边界

- 继续支持现有受限固定 genesis、单一级来源完整链、替换一个旧终端的明确 1–8 目标整份复制任务。每个新下级祖先必须在同一批，完整父/子仓库仍在相同目标集合。
- 如果某保留版本没有完整原低级链，或含其它原路由、缺明确 primary owner path，则拒绝整个本阶段，不扩大义务、填默认路径或暗移部分文件。
- 先保留全部目标的独立空间预算；单目标上限为 4096 个 native 对象、64 MiB 完整材料、1 MiB 清单、16 MiB 路由写集。调用已有保留式存储，含目录/临时文件余量，每盘另留 16 MiB；完成文件另由这部分余量覆盖。无其它盘补贴、链接或去重副本。
- 存储使用相同 bytes 的幂等补缺。未知格式/名称、截断、不同 bytes、容量不足、目标失联都停止并保留材料，不做旧格式迁移或清理。
- 只保留一个首目标编码和当前另一个目标的重建结果，依次验证；每次真正执行仍要遍历完整目标证据和已保留 State，尚未做吞吐/内存实测。

## 最终激活仍缺的条件

本片不能清 `blocked` 或 `membership_fence`。活动 Config 仍有单后继限制；最终 Backup `PortableAuthority` 分支、真实新盘 seals/journals、完整新范围历史库、首个已决定业务 publication、镜像到实际 serving locator 的安装、未来备份策略写者及原决定活动恢复尚未完成。目录/文件镜像路径和拟议逻辑 owner path 不是同一种地址，不得直接混用。旧 policy/receipt 不能被此材料隐式改写；未来策略的真实后继规则还需与最终 authority 共同选择。

完整外来高级盘接纳仍由 helper 离线一次闭合低级链后执行；本片不注册外来盘或创建占位。由于先前原 native Backup 选择已可能排除 Abort，原池可能持续停写，应向操作者保留真实阻塞状态，不能为了恢复可写回退到未认证旧权威。
