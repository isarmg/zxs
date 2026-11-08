# 源围栏后的目标实体安装与原任务续作

本片以 `766d8a52` 为准确父基线，在 `aa6c3824` 的[原生源写围栏](backup-source-fence.md)之后增加真实目标实体复制。**仍没有最终目标激活／权威切换**，不解除原代全池停写，不返回迁移成功。按当前要求未审查、编译、测试或运行 CI；没有操作真实数据盘。

## 核心入口与权限

`Engine::start_backup_target_installation(operation_id, principal)` 只接受已经完整写入所有原成员日志的同一 `BackupCutover` 意图。宿主仍须认证 actor；字符串不是身份凭据。只有当前已准入原生围栏能提供内部能力：原 actor/op、Config、sequence/snapshot、完整 State 摘要、所有原成员 receipt、持有根和原成员 seal 都必须匹配。SYSTEM 的任务 JSON 或目标完成 marker 不能单独授权。

该步骤明确只允许在原代已经停写时向原任务的隔离目标写入。没有放宽普通 `require_write`、Add/Joint 或目标 I/O 准入。需要补齐的部分源围栏先由原操作的既有重试完成；本入口不猜测缺失 receipt。目标替换、原状态变化、认证取消后的新代际、缺失成员或损坏材料均拒绝，原数据保留。

## 实际安装内容

对每个原任务已明确选择的目标，从**该目标自身**重新准入的完整 State、原生前缀、历史包／正文和原范围数据库出发，生成并复制 `activation-image/`：

- `tree/<逐跳直接上游盘名前缀>/histories/<history>/directory-repositories/<根仓库ID>/…` 使用既有纯目录森林规划器的稳定 main／工作快照、嵌套仓库与空目录。父子仓库和文件实体在同一目标，未按版本分盘。
- 每个文件实体内部保存 `.zxs-file.sqlite` 和 `.zxs-versions/<version>/`，后者包含准确版本描述、内容状态，以及每个 AVAILABLE 版本独立写入的正文和 checksums。不是只复制当前 HEAD。非 AVAILABLE 版本保留描述，不伪造正文。
- 目录仓库实体内部保存自己的 `.zxs-directory.sqlite`。未经初始化的普通 namespace 目录另外按外仓库身份和当前路径保留；它不是目录仓库初始化。未归入目录森林的稳定文件实体按 file ID 和已保存 owner path 放在 `file-containers`，不猜测新的归属。
- 原 `histories` 包、完整 State／native-control、SYSTEM-current／原盘范围 SQLite 以及旧 completion 全部留在目标原有准备域，不搬走、不覆盖、不改旧收据。新实体复制额外占空间，不采用硬链接、reflink 或去重。

这里的物理树是**明确版本化的隔离实体映像，不是活动盘格式或挂载点**。尚无 live locator/新 registry/placement 将它开放为正常业务数据；不能把目录存在、SQLite 可读或映像完成当成目标已纳入系统。实体 manifest 绑定原 fence 和全量路径／字节承诺，不是对外恢复权威。

## 持久顺序与恢复

1. 在任何目标路径重开前核原生围栏能力；随后按原选择顺序获得每个目标 held root 和 writer lock，重核 device/inode、来源隔离及完整原低级链。所有新祖先仍必须属于原同一批拓扑，无占位补盘。
2. SYSTEM 无覆盖持久化 `backup-preparation/target-entities.json`，其中绑定完整原围栏证据与哈希，才允许第一笔目标安装。
3. worker 在锁外持久化进度；目标均独立复核原切换材料及完整映像容量，再逐目标安装不可变意图、manifest、目录、SQLite 和全部正文。
4. 每份正文独立流式复制、全长／完整摘要验证与同步；已存在文件必须精确匹配，不把“存在”当完成。未知名称、不同字节、链接、身份变化和容量不足保留拒绝。
5. 每个目标只有完整映像读回后才能保存自身 acceptance；所有目标再次读回核对后，才分别保存同一个完整集合 marker。`TargetEntitiesPrepared` 仅表示这一步的终态。
6. 冷开在旧准备 worker 之前检查本阶段任务。只在同一原生围栏、State 和全目标绑定仍有效时重启 worker；已同步准确文件复用，缺失的准确文件补齐，不重新选择或重采源。失败留下任务和诊断，原源继续停写。

进度保留 worker 的 Copying/Verifying/Blocked/TargetEntitiesPrepared 以及具体错误；存在源围栏时另带 RecoveryRequired 和真实成员 receipt 数，防止材料进展掩盖业务停写。任意底层 I/O 仍可能阻塞，停止检查点／join 不承诺硬关机时限。

**恢复边界**：当前只能续作准确缺失或完整文件。若硬中止留下不能归属的临时文件，闭合 inventory 会保留并阻塞，不据随机文件名前缀擅自删除；这不是所有写入字节边界的自动修复保证。源取消、新代际、缺失 SYSTEM 安装意图或格式变更不能凭旧进度自动重开这份任务。认证取消本身仍由原成员协议处理。

## 预算与仍缺闭包

每份映像最多 100,000 个路径项、96 MiB 元数据／路径累计、16 MiB manifest；另沿用原生材料、完整 State 和 SQLite 的既有预算。每个目标独立预留缺失正文／元数据、目录预算和 16 MiB 余量，不从另一目标借空间。多轮材料／正文读回成本与全量保留数据量成正比，未测性能，不接普通热路径。

本片仍限制原准备支持的 fixed-genesis、单主盘完整单位、单旧终端边替换；多明确目标和其中的完整多级树仍可准备，但活动 Config 的多后继语义没有放宽。跨旧目标父子合流、完整系统历史和盘根全部目录／标签审计、目标新身份库投影、真实共同最终决定、活动多下级 writer/reader、权威切换冷重放、旧副本退役及 helper 外来完整链正式接纳均未完成。普通设置不会自动启动源围栏或最终迁移。

SYSTEM 派生编码后继增加业务族时，原准备字节按现行严格编码复验可能拒绝；保留原字节，不增加旧格式兼容／双读或静默覆盖。隔离映像的目标身份描述同样不把旧源 scoped 数据库重新标记为新盘权威。
