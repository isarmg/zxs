# 文件容器唯一当前名称

状态：按用户明确要求实施，当前尚待准确提交的 GitHub 定向验证。
文件容器名就是 SMB 暴露的文件名；切换到改名前的正文节点不回滚名字。
本文不表示完整时间线、真实 Chrome、Samba 或磁盘备份已经验收。

## 名称与身份

`file_locations` 的最后组件是容器当前名称。`file_names` 仅保留为必须吻合的
投影镜像，读取不允许回退到该镜像、冻结节点路径或 `creation_name`。
同一 FileId 的当前 occurrence 使用同一叶名；父目录位置仍属于具体 occurrence。
改名须检查全部最终路径的碰撞和路径预算，保留原授权、代数、CAS、路径事实及恢复证明。
冻结提交／节点的原始 path 和 creation_name 是来源证据，不为显示最新名字而改写。
历史选择恢复正文时使用当前容器名，下载名称和历史读取 DTO 同样取当前名。

FileVersion 不再持久化独立的 `storage_name`，改为严格的 `VersionStorageId`。
它有且只有 15 个小写 ASCII 字母或数字，在本容器内唯一；当前生成器对 VersionId 做
域分隔摘要后取 15 位，分配及 FILE／history／State 准入显式拒绝碰撞。
不能把截断摘要自身当成唯一性证明，也不在改名时重新生成身份。

已安装版本叶名为 `storage_id-current_name`。改名更新全部仍有正文的版本叶名，
版本 ID、15 位身份、正文及 inode 保持；原 creation_name 只用于来源／准备材料。
历史读取 DTO 的 storage_name 是派生值，不是第二份可独立设置的名称。
共享 contract 验证器同时验证严格身份、原名称政策及与 download_name 的逐字一致。
前缀不消耗用户名称预算；真实文件系统组件上限仍由持有的目标目录负责检查。

## 改名与恢复

准备阶段捕获原容器与各正文的真实身份，验证完整目标和无覆盖条件。
决定后持久化有界的 inode 绑定意图，再逐项执行 no-clobber 叶名改动和外层布局改动。
重试只接受同一容器、同一版本集合、同一 inode 的原名或目标名，拒绝陌生目标、
同名替换物和丢失的两端；不扫描任意文件来猜历史身份，不用复制／删除覆盖恢复。
叶已改而外层未改、外层已改而意图未清理时，读取只按该准确意图定位。
FILE 与外层布局完成后才清理已验证的意图；未知结果继续原 operation，不制造新操作。
本节的资源 reservation 与实际故障尾验仍需由准确 GitHub 测试证明，不能以静态审查代替。

## 新格式，不做旧格式兼容

本批在 path-facts 格式之上统一使用 `container-current-name-*` 的 managed-directory、
membership-generation、membership-pending、frozen-control-metadata、
retained-exit-quorum-control-metadata、radix-map-roots 标记；preparation 使用
`zxs-container-current-name-backup-native-control`，FILE 为 `file-local-container-current-name`。
内层 FileVersion 必须有 storage_id 且拒绝旧 storage_name。外层标记同时换代，
避免没有版本的旧空池绕过字段准入。旧池／旧备份拒绝后原字节保留，不自动迁移、
删除、重置、重算摘要或覆盖。标记适配不是恢复备份开发或放宽原 900 秒验证。

## 验证记录

本地仅代码编辑、静态审阅与 git diffcheck，不运行编译、测试、formatter 或选择器。
定向范围包含身份严格编码与碰撞、所有 alias 当前名称、切旧节点、HTTP／Web名称绑定、
冷重开和原操作重试、叶／外层中断、inode 替换及目标碰撞，以及相关特性编译。
需取得最终准确 head 的已执行、失败和未执行清单才形成运行结论。

原 canonical_publication 和实际历史捕获保留原字节，用于原证据或明确旧格式拒绝。
更新后的正向样例标明为合成数据，不冒充新 GitHub 捕获；原标签／图证据不证明新名称合同。
备份业务与原超时优化继续放在其他工作之后。
