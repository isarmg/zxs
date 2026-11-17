# 全新空池的唯一三范围布局：同一决定与逐盘只读闭包

**状态：待确认，尚未实施或冻结。** 本次只请求确认下列三项：

1. 假定不存在需要支持的旧版本。只定义一种格式、一套编码和一条正常路径；只能显式
   初始化全新的空池。普通打开用真实 genesis、封印、登记与闭包准入，不发明新版标记。
2. FILE、DISK、SYSTEM 和目录本地材料由**现有同一次**原生决定绑定，新增有界、闭集的
   scope 根、逐盘清单和局部材料引用。SQLite 不是第二个决定点。
3. 每块登记盘能从**本盘材料**，按指定的已证明决定，只读核验自己的目录、容器、
   稿、历史、备份 occurrence 及 DISK 记录。离线读不是全池写入授权。

**不在本次请求内：**向任何盘新增管理认证或其他受保护控制材料的副本、H05 的
SYSTEM 救援包及其 writer、恢复 CLI／wire、认证恢复与激活；也不隐含批准其他业务
ADR 的持久字段、协议或行为。第一阶段不会赋予 data-only 盘投票权。未知数据原样保留。

## 为什么做这三项

现有 FILE SQLite 能核对一个已捕获文件的投影，DISK current/history 能核对 placement、
备份目录及明确的 before/after，SYSTEM 索引能核对非秘密业务事实。SQLite 完整、目录
存在或 hash 自洽均不能证明：这个容器属于这一登记盘，或它的稿、基准和不可变正文
都是同一次发布的完整集合。没有首节点的手动容器同样必须有归属和完整库存。

保留[现有全配置成员发布](0002-publication.md)及[代际交接](0013-generation-bound-membership.md)：
正常发布要求原配置全部 N 成员；既有决定和恢复按原固定成员及代际的
Q=floor(N/2)+1 验证，不按在线数重算。局部 material 和[离线盘日志](0015-independent-disk-journal.md)
均不选举全池权威。已有 Candidate、永久回执及其原哈希字节不改写。

## 新池入口和拒绝边界

- 保留现有 Config DTO、单值语义字段、盘封印及 Snapshot 的既有格式字段；不改枚举值、
  不增加版本号、第二个 marker 或版本选择器。单靠原有标签不能证明布局：原固定地址的
  genesis 必须含准确初始化意图、同一 Config 和完整 scope 根；封印、成员／注册代际及
  五区域的 `RootBinding` 必须匹配。对外 HTTP／SMB wire 与现有原请求不变。
- 仅明确的初始化请求能对**全部**原选定根操作。在已有互斥和 held 句柄下核对
  named/held 身份、根互不别名、与既有登记不冲突、每个根确实空。空目录本身不能
  证明它是新盘，特别不能重置已登记但失去标记的根。genesis 含全部原盘的注册与
  五区域绑定、真实空库存和同一初始化 operation；各原根落盘、同步、读验后才成功。
- 普通 open、显式 restore、管理打开及成员激活均在创建持久写锁文件、重建数据库、
  补锚或改封印之前拒绝缺件、实际编码或封印不符、未保存绑定、身份冲突。部分初始化只能
  凭已保存的原 bootstrap 意图及原绑定显式续作；绑定不全保留拒绝。既不鉴别历史版本
  也不读取、迁移或重写不符材料。离线、权限失败、根被换绑均不是空、损坏或已销毁。

## 接线和编码

每一行都是唯一的规范结构。首列是既有锚位置、既有对象或新增不可变对象的**类型域**，
不是额外的版本字段。对象只含列出的必需字段；行内记录由所属 map 决定类型。只用
下述显式联合表达不适用；不得以缺项、空字符串或任意 JSON 作为分支。所有 ID、根、
代数、哈希沿对应 contract 类型和原规范编码。严格读取拒绝重复键、未知字段、非规范
十进制 u64／摘要和多余证明节点。对象采用各自有域的 BLAKE3 规范 JSON；引用不能跨域。

| 位置／类型域 | 必需字段和含义 |
| --- | --- |
| 现有固定 genesis 配置锚 | `config,initialization_operation_id,scope_closure`。只在新池原固定 genesis 地址写入；scope 内引用 Config 自身摘要，不能引用包含自己的 anchor 摘要。 |
| 既有 Snapshot 对象 | 原 Snapshot 的 `format,sequence,business_generation,topology_generation,maps,events,control_auth` 及新增的 `scope_closure` 摘要。原受保护引用仅沿原权威路径使用，不新增其正文的落盘位置。 |
| `scope-closure` | `pool_id,origin,sequence,membership,system_material,disks,counts`。`disks` 是登记身份到逐盘清单摘要的有界 map；不是“现在可见”的盘列表。`system_material` 指向原系统目标上的非秘密 SYSTEM 业务投影描述。 |
| `system-scope-material` | `pool_id,sequence,business_generation,topology_generation,configuration_hash,material`。`material` 只能是下述 SYSTEM 业务 SQLite 引用。原 Snapshot、Candidate 和原成员权威继续独自负责完整全局状态及控制权限。 |
| `disk-scope-manifest` | `registration,root_binding,origin,sequence,inventory,disk_current,disk_history,local_changes,retirement_obligations,counts`。后两张业务表分别绑定本盘准确变化链、已有原生终态决定的义务；空表也有根。DISK 两个 SQLite 引用分别核对其准确登记与逻辑内容。 |
| scope 内的 material 引用 | `scope,identity,logical_root,representation,body_inventory`。`scope` 仅 `file,directory,worktree,disk_current,disk_history,system`。身份分别为 FileId、仓库/根目录身份、WorktreeId、准确盘登记或 PoolId。`representation` 为 `sqlite{schema,image_hash,image_bytes}` 或 `native{root_hash}`；native 只供已定义原生材料的目录仓库／worktree。 |
| 逐盘 inventory 行 | `object,ownership,location,lifecycle,entity,operation_id`。`object` 为下列按 kind 分派的严格记录；`ownership` 为 `original{registration}` 或 `replica{owner,replica_id,source_chain}`；`location` 为 `anchor{root_id,directory_id}` 或 `entry{root_id,parent_directory_id,name}`。登记根、五区域及实际实体均独立核验。 |
| 局部正文 inventory 行 | `kind,source,relative_name,bytes,content_hash`。`kind` 仅 `full,draft_extent,draft_base,directory_snapshot,worktree_delta`；`source` 分别使用该类型已获批的版本、稿代数或快照原条目身份。文件相同字节不能合并不同来源。 |

- `origin` 在 sequence=0 为 `genesis{initialization_operation_id,config_hash}`；发布为
  `publication{operation_id,request_digest,predecessor}`。第一笔若原 Candidate 的
  `previous_snapshot` 为 null，`predecessor=genesis{anchor_hash}`；其余为
  `snapshot{snapshot_hash}`，且逐字匹配原 Candidate。scope 不引用本次 Candidate、
  Publication 或包含它的 Snapshot 的摘要，因而不产生自引用。
- `membership` 严格区分 `genesis{config_hash}`、`membership{authority_hash}`、
  `registry{authority_hash,registry_commitment}`；均核对原决定上下文。登记身份是
  pool/disk/slot/binding generation 全元组；副本持有位置、原容器 owner 是不同事实。
- `object` 为 `ordinary_directory{directory_id}`、`file_container{file_id,material}`、
  `directory_repository{repository_id,directory_id,material}`、
  `worktree{worktree_id,material}` 或 `backup_replica{replica_id,material}`。
  普通目录不伪造文件，未生首节点的 FILE 必须包含稳定 ID 和完整稿。不同路径
  occurrence 不按 ID 或哈希去重。对象内 Born/Unborn、保留归属、稿/快照/备份/终态的
  细节仅使用相应已批准的严格格式；若尚不足以表达，则不宣称该对象范围已实现。
- `counts` 为 `inventory_entries,materials,body_entries,canonical_metadata_bytes`；
  闭包总数再含 `disks`。计数从完整实际记录重算，不能由自报摘要替代。分页使用
  [既有 radix 身份及路由](0006-bounded-metadata-roots.md)；闭包根不超过 8 KiB，
  scope 叶最多 128 条且不超过 64 KiB，单条装不下就拒绝。本地 FILE／目录 SQLite
  沿其 16 MiB 镜像／12 MiB 记录／65,536 行门；DISK、SYSTEM 依各自现有限额，
  不能用 FILE 限额冒充通用上限。准备阶段保留 64 MiB 元数据预算及 32 盘边界；
  实际正文大小独立计量，预算失败先于写入，不截断清单。
- SQLite 镜像须从 held 根按身份导入受限连接，核对完整 schema、实际页字节与大小、
  严格逻辑行和域摘要。重建出等价行并不等于重建了原镜像字节。只在可信写集与已验证
  谱系允许时复用未变分页；冷开、可达集合改变或失去证明时重新完整准入。

## 一次发布，逐盘只读

1. 现有 owner 在同一来源捕获权限、原请求、成员代际、真实对象及材料清单。准备时
   在原目标留存正文及局部材料，同步并从每一实际 held 根读回。源、备份、未知临时文件
   不因重试被清除；不同文件系统不是一组原子 rename。
2. 在现有线性化点重验选择、代数、权限和根。同一 Candidate 的 Snapshot 承诺唯一
   scope 根，仍由原 Publication／seal 和永久回执证明。决定前不呈现半成品；
   决定后仅凭原 operation 与原清单补齐安装，不分配新身份、不重复发布。
3. 本盘只需自己的逐盘清单、scope 根、登记表到本盘条目的 inclusion path，以及本盘
   所需原生对象、FILE／目录／DISK 镜像和正文；沿 layout 的原私有不可变对象命名，
   不另建 mutable latest。根中跨盘材料或非秘密 SYSTEM 投影的**摘要**不是把其正文
   复制到每盘的许可。不得复制认证数据库、密码验证值、控制认证正文、会话、密钥，
   或把含它们的原生对象／整库／日志作为“证明”间接复制。
4. 只读核验从调用者**独立已验证的原决定**出发：对真实 genesis anchor，或对已按原
   成员、代际、Publication／seal 核验的准确 Candidate/Snapshot，提取并固定其
   pool、sequence、origin 和 scope 根。再由该根的认证路径定位本盘清单，在本盘独立
   读完其对象、原路径和正文。无需另一盘补局部数据；若只有自报的本盘摘要或无可信
   决定证据，只能报告材料可读或证据不足，不能报已发布。不是用单盘数字最大的
   sequence 选择前沿。缺件、不同根、变更实体、冲突或预算耗尽均保旧拒绝。
5. 一块盘的结果只对该准确前沿和该盘范围有效；其它盘离线、不可读或在本盘缺行
   都不是对象销毁证明。只从已证明终态沿原规则处理。已决定的业务不能由 SQLite 失败
   撤销，但缺少 scope 安装须报告原 operation 的未完成状态，不谎报可用。

## 后续 H05 不是本次决定

H05 最终必须能从**任意一块已登记盘**（包括 data-only）离线重建该盘持有的最后
一个完整 SYSTEM 前沿；不是只救援本盘文件，也不承诺仅凭单盘证明全池不存在更新决定。
其他盘缺失的正文不虚报恢复。不降低固定 N／Q，不自动提升成员、激活在线 writer
或恢复旧会话。本次不决定是否或如何制作受保护副本、加密及保管、完成时机或 H05 的
读写协议。具体敏感数据、权限、恢复输入与输出须另行提案和明确授权；本阶段不添加
救援包 writer、恢复 CLI、公共 wire 或凭据副本。

## 集成后分别验收

- 真实新池首启、空 scope、首次无前驱快照的发布与冷重开；缺少必需字段、错误封印、部分初始化、
  离线与换根均不产生修复写入，保留原材料与准确错误。
- 同一操作的 FILE／DISK／SYSTEM 和真实物理对象对应；正常写入、改名、零节点稿、
  稀疏 worktree、保留区、独立备份；在决定前后中断，只查／重试原 ID。
- 本盘只读核验须拒绝伪造／移植的登记、漏对象、错误路径、截断图、损坏 SQLite、
  镜像逻辑等价但字节不同，以及原决定证据不足；检查没有新增受保护数据路径。
- 各边界的实际页数、元数据量、FD、读回／关闭错误和耗时，以及真实 SMB、冷启动、
  掉盘、物理断电证据分别记录。六路代码合并后再执行；本文不代表已测试。
