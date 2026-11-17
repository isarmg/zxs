# worktree 新文件的原生出生与保存

## 本片决定

新增文件第一次进入 worktree 时，在同一磁盘的真实 `deleted-data` 下独占创建容器。
分配稳定 `FileId` 与实例内 `EntryId`，真实 FILE 数据库初始只有 Unborn/Empty 工作稿，
不创建历史 Version、FILE 分支或目录节点。同源的两个实例接收同名文件，仍分别分配身份。

`ContainerNamespace.parent` 使用明确的 `Worktree` 分支，保存原实例、逻辑路径和留存名称。
普通 `Root`、`Directory` 的原编码和已有原收据保持不变；这不是旧盘迁移或兼容读取层。
逻辑名称仍是用户文件名，不能用删除区物理前缀反推实例或权限。

删除区容器和 FILE 管理数据分别使用原登记根的 `deleted-data` 与 `zxs-data/deleted-data`。
所有目录访问仍经持有的磁盘根、原 seal、登记代数及实际 inode 核验。
FILE 的空容器例外、EXCL 准备、真实物理反馈、SYSTEM/DISK/FILE 发布与冷恢复共用原生路径，
没有创建普通 main/common-data 容器后挪过去的中间状态，也不写占位 inode。

## 原操作与完成界线

1. 原 `BirthFile` 请求冻结实例 revision、路径、稳定身份、永久盘号/名称分配及独立完成号。
2. 同一原操作执行实际 native FILE 出生。原决定可见但 `CompleteFile` 未完成时，实例整体为 Pending。
3. 持有原 ledger checkpoint，核验 native 容器与工作稿，追加确切 Added frame。
4. 独立 `CompleteFile` 绑定原请求 commitment、工作稿、checkpoint 和 revision，之后才返回成功。

保存采用相同顺序。帧写入使用现有 `WorkingWriteSession` 和实际 `PreparedWorkingSave`，
固定原 State Arc、Config、磁盘资源、slot 与 held extent，不能从 manifest 重新铸造准备权能。
普通写入只发布不可变 extent 和长度，不合成全量 Version，不计算虚构的整个文件 hash。
Flush 后可继续下一代保存，Close 不创建历史。

每个原操作的准确重试先查询永久请求与主体，已完成返回原事实；未完成保持原 Pending，
不再分配另一份容器或新操作。冷开只接受独立完成指定的 ledger checkpoint，且逐项对照
原收据导出的 Added 集合，不接受本地最新 frame 自证。未决尾部的自动续作不是本片承诺。

删除前缀的盘号为整个池唯一的 `DiskId → number` 事实，首次实际 BirthFile 原决定惰性登记。
后续工作树和未来普通删除必须消费同一原收据索引，Pending 也占用；不按 slot、配置顺序、
名称、UUID 前缀或各实例单独计数。完整时间、随机串和 resolved 名称均保留在原决定中。

## 当前边界

- 生产新增只接已登记 root-only 实例的根级文件，不声称子目录内新增已完成。
- 工作稿枚举和读取使用明确 `Working` 类型及 NativeRanges，不伪造 VersionId 或 whole hash。
- 含 Added 的显式目录提交在任何 FILE/目录发布前明确拒绝。下一片须闭合真实首 FILE 版本、
  目录节点及 MOVE，不能把本片当作新增文件已能提交。
- 原 snapshot 文件的既有覆盖路径不改为增量算法。该来源的 reopen 仍可能扫描完整正文。
- 本片不包括移动、改名、跨盘、备份升级、旧盘迁移或额外恢复协议。

## 定向验证

真实 Engine 夹具覆盖 deleted-data 实体和 FILE 数据库、两个实例同名独立身份、Empty 读取、
Native 双 Flush、旧 extent 读者不可变、准确重试零写、拒绝提交零写及冷重开。
默认共享的 Session/dispatcher 生产调用由同批 SMB 消费者夹具验证。
外部 smbd/smbclient 网络会话未运行，进程内 dispatcher 不替代真实 Samba 验收。
