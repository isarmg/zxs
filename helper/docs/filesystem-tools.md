# 文件系统检查工具：先看能检查什么

`helper fs-tools` 列出七种格式的检查方式，并在本机固定安装位置查找对应的工具。指定格式只观察一项。例如：

```sh
helper fs-tools
helper fs-tools xfs
helper fs-tools ext4 --sha256 <来自独立可信发行材料的64位十六进制摘要>
```

**这不是设备检查或修复命令。** 不接受设备名、镜像路径、池名、自定义程序或追加工具参数，也不运行检查器（包括版本查询）。`preview` 是展示给人看的模板，不是可执行任务。每行分别显示固定位置、安装元数据、可选摘要、版本与签名的证据。不存在仅指所列固定位置，不搜索 PATH 或别的目录，不安装软件。

FAT32/exFAT/ext4/XFS/NTFS 从[随 helper 私有打包的固定位置](../packaging/debian13-amd64/README.md)查，五种格式各有独立受限的镜像检查命令；其余格式的宿主观察位置按 Linux Debian 13 常用的 `/usr/bin`、`/usr/sbin` 定义。仅认可当前命名空间内从根到该文件所在的目录都由 root 持有、组及其他用户不可写、不经符号链接，文件本身是至少有一个执行许可位、非 setuid/setgid 的非空 root 所有普通文件。内容比对前后重核原实体、挂载、名字和属性；这是当次元数据观察，不证明介质、发行来源、动态库或调用时的权限安全。其他安装布局、符号链接均不冒称合格。

只有显式给可信预期 SHA-256 才读取这一个程序文件。单文件最多 64 MiB，沿用普通文件比较的本地文件系统、真实 procfs 与 `O_NOATIME` 门槛。当前身份无权时不提权、不去掉标志重试。匹配只说明所读完整文件与给定摘要相符，**不验签、不判版本**；期望值不能取自受检文件自身。清单和已确认的缺失可返回 0；指定摘要的完整读回匹配为 0、确实不同为 1，无法查证、输入错误或所选原件在观察中改变为 2。以上都不代表磁盘健康。

| 格式 | 只读检查的对象和限制（除 FAT32/exFAT/ext4/XFS/NTFS 外尚未接执行） | 结果特别注意 |
| --- | --- | --- |
| [ext4 / e2fsck](https://manpages.debian.org/trixie/e2fsprogs/e2fsck.8.en.html) | [独立、未挂载、单卷的本地 ext4 副本](ext4-copy-check.md)；原只读租约，调用 `-f -n`，不接外部日志或待恢复原件。 | 退出为位掩码，未修复、运行错误、取消可以与其他标志并存；跳过日志恢复不能当全部正常。 |
| [NTFS / ntfsfix](https://manpages.debian.org/trixie/ntfs-3g/ntfsfix.8.en.html) | [独立、未挂载的单卷副本](ntfs-copy-check.md)，固定 `-n` 只观察部分 MFT、镜像及备用启动扇区，不清 dirty、不重放或重置日志。 | 不是 Windows `chkdsk`；日志清洁、Windows 休眠及全卷完整性均未验证。 |
| [ZFS / zpool status](https://openzfs.github.io/openzfs-docs/man/master/8/zpool-status.8.html) | 仅明确指定、已经导入的一个池，观察池及 vdev 现状，不按分区检查、不代导入。 | 命令成功仍可显示降级。现有检查进度可观察；[scrub](https://openzfs.github.io/openzfs-docs/man/master/8/zpool-scrub.8.html) 会利用冗余修复，不是只读操作。 |
| [XFS / xfs_repair](https://manpages.debian.org/trixie/xfsprogs/xfs_repair.8.en.html) | [独立未挂载的 XFS 单卷副本](xfs-copy-check.md)，固定 `-n -f`；不提供外部日志或实时设备。 | 1 可表示发现问题或运行失败。脏日志有时警告后仍继续；无完整证据均未确认、不清日志或挂载。 |
| [Btrfs / btrfs check](https://btrfs.readthedocs.io/en/latest/btrfs-check.html) | 即使 `--readonly`，给单个成员仍可能查找相同 FSID 的别的设备。必须隔离并验证完整未挂载成员集，不对宿主上任意单张镜像启动。 | 不提供强制检查或修复；一个正确镜像副本能让检查器继续，并不说明每块介质都无错。 |
| [exFAT / fsck.exfat](https://manpages.debian.org/trixie/exfatprogs/fsck.exfat.8.en.html) | 已确认为 exFAT 的完整独立未挂载镜像，`-n`。 | 用 exfatprogs 对应版本的退出语义，不用 FAT32 的解释；[包内版本 1.2.9 的实现](https://github.com/exfatprogs/exfatprogs/blob/1.2.9/fsck/fsck.c)区分未修复、执行失败与用法错误。 |
| [FAT32 / fsck.fat](https://manpages.debian.org/trixie/dosfstools/fsck.fat.8.en.html) | 已独立证实 FAT32，不把 FAT12/16 或 exFAT 当 FAT32；完整独立未挂载镜像，`-n`。 | 退出 1 还可能包含程序内部不一致，不报告“确认已修”。 |

[FAT32](fat32-copy-check.md)、[exFAT](exfat-copy-check.md)、[ext4](ext4-copy-check.md)、[XFS](xfs-copy-check.md) 和 [NTFS](ntfs-copy-check.md) 已各有独立副本只读检查命令，不是本清单命令的隐式行为，尚未实跑。其余格式后续首先只考虑对**独立完整的工作副本**做有界诊断。须锁定可信发行程序与版本、隔离程序可读的所有资源和进程参数、禁访问原件；取得和解释完整退出证据，超时、输出超限、外部日志/成员不可核、工具缺失时报告未完成。修复另需明确授权、方案确认和适用工具；FAT32 的交互式模式不得用自动回答代替。不提供自动全修、清日志、强制策略、格式化、挂载或通用 shell。原盘的未挂载/无交换、没有跨命名空间/其他持有者、执行中不被再用都要另有可信证据，当前没有原盘修复入口。这些工具是可选宿主维护，不定义存储池格式。
