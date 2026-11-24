# 只读宿主观察

面向只读宿主诊断。先按下文明确给出目录或设备号；命令不会接管池、写入磁盘或启动长期监控。
需要离线维护时，另看[离线指南](headless-offline.md)。

## 目录和文件比较

```sh
helper inspect-dirs /home/alice/downloads /home/alice/downloads/week-01 /mnt/incoming
helper compare-files /home/alice/reference.bin /mnt/incoming/candidate.bin
```

**目录观察。** `inspect-dirs` 接受 1–16 个现存绝对目录，包括 `/`。每行用输入序号标识目录；每对位置分别报告是否同一实体、内核返回的设备号是否相同，以及从原始路径实际看见的两个祖先方向。同一目录可以选两次，正常的父子关系不是错误。内容不被枚举，名称不回显。最多输出 120 对关系、32 KiB；同时保留的目录操作描述符不超过 18 个，不计进程原有描述符。不同设备号不证明独立物理盘，路径中看不见共同祖先也不是跨 bind 挂载的全局不相交证明。

**文件比较。** `compare-files` 要求本次选择的是两个不同、各只有一个硬链接且不超过 256 MiB 的普通文件。两端先以不读取内容的描述符核验身份和实际文件系统，再尝试打开任何一端读取。只受理 ext2/3/4、XFS、Btrfs 和 F2FS；未知文件系统、procfs/sysfs、tmpfs、网络、FUSE、overlay 等不支持，被拒绝的输入不会被探读。需要真实的本进程 procfs 和使用 `O_NOATIME` 的权限（通常为文件所有者）；不能满足即失败，不按名称回退重开。

比较按两块 64 KiB 缓冲区流式读取，即使首字节或长度不同，也要确认两端的完整长度和终点。输出前重核原位置及关键文件属性。返回码 `0` 表示文件相同（或目录观察完成），`1` 表示两份合格文件的内容确实不同，`2` 表示参数、权限、支持范围、变化或 I/O 错误使观察不能完成。

结果只覆盖本次非原子读取；参照文件来源须独立核验。

两个通用命令都要求未经缩写的绝对路径，最多 4096 字节、128 个分量；拒绝符号链接和空、点、父级分量。没有默认扫描、修复、写入、覆盖或上传行为。更多边界及未来范围见[架构](../../ARCHITECTURE.md)。

## 在线查看磁盘、CPU 与内存

ZXS 运行时可另开终端，用 `helper disks --rates` 看宿主磁盘 I/O、用 `helper storage-status …` 看预选目录所在文件系统的容量。观察按当前用户权限读取宿主，不取 SYSTEM 离线独占锁，不读池数据库、卷标或正文。

`helper hardware` 不带参数。CPU 只取约一秒内同一内核汇总计数的两次差值，显示执行、I/O 等待估计及虚拟机 CPU 时间被占用的比例；内存显示内核可用 RAM 总量和 `MemAvailable` 估计（CLI 为字节）。

这不是 ZXS 进程用量、容器限额、磁盘忙碌率或硬件健康。缺失、异样或无法比较的读数标为未知，其他有效项仍显示；CLI 两项都得出结果返回 0，否则 2。只在显式命令时采样，不运行基准、写入设备、修改性能参数或启用后台采集。

内核数据来源见 [Linux procfs 文档](https://docs.kernel.org/filesystems/proc.html)。

池登记、恢复和账户操作须按各自流程核原件并独占 SYSTEM。

## 查看选定的存储根

```sh
helper storage-status --system /zxs/disks/system --recycle /zxs/disks/recycle --data /zxs/disks/data/photos --data /zxs/disks/data/video
```

明确给一个 SYSTEM 候选、一个回收站候选和 1–62 个普通数据候选；顺序不限。当前存储根参数的原始路径边界为：最多 4096 字节、1–256 个分量，不接受 `/`。不做 UTF‑8 转换；与通用命令一样不跟随链接，不允许空、点或父级分量。

报告开头是采样的 Unix 毫秒时间窗，随后按角色和数据盘序号列出结果，不回显路径。**角色来自终端选择；登记身份和布局未核。** 此命令不枚举设备。

每个目录以自身持有的描述符取 Linux 挂载 ID、内核是否将它标为该挂载的根（可能不报告），再用该目录的文件系统统计取只读标志、总字节、全部空闲字节和非特权进程可用字节。挂载 ID 只在本机当次命名空间与该挂载的生命周期内使用，不是设备/文件系统 UUID。

只读标志未设置不证明当前账户有写权限；可用量不是可保证的预留空间，也不能跨行相加当作独立磁盘容量。

只观察已挂载目录，不触发自动挂载。报告前重新解析原路径，核对全部可见祖先、目录实体与挂载 ID；身份不匹配不发布那一行的旧数据。若路径不存在、权限不足、挂载 ID 不可取或容量统计不可用，将保留其他行和仍可核的字段，返回 `2`。各行挂载与容量都采到时返回 `0`。

结果按行采样，不是全机快照。

## 查看块设备、分区与 I/O 速率

```sh
helper disks
helper disks --device 259:1 --rates
```

块设备清单按 `主设备号:次设备号` 逐行列出，给出内核名称、整盘或分区、块层容量。分区另列实际父设备、分区号和起始扇区。名称不是固定的盘身份。“整盘”也可能是 loop、dm 或其他虚拟块设备。不会将父盘、分区和底层设备加总。

对超过 256 个条目的系统，改用 `--device` 精确选择一项。

`direct_mount_ids` 只列本线程挂载表中超级块设备号直接匹配的挂载 ID；同一文件系统的 bind 挂载可能有多个。无直接匹配不表示未挂载或未被 swap、阵列、其他命名空间使用。

`fs_uuid_udev_alias`、`partuuid_udev_alias` 来自受保护 devtmpfs 上当前分别指向该设备号的 udev 别名；它们是可陈旧的宿主缓存，不是现读介质、硬件序列号、登记 ID 或挂载 ID。

无法安全归属的别名和无已知缓存型号的设备显示未知，不启动识别程序。只对设备节点取路径元数据，不打开设备读取扇区。

`ancestor_subsystem_sysfs` 是同一盘世代、在同一 sysfs 挂载中核对的最近有报告的非块层祖先子系统，最多处理 24 个设备路径分量。当前识别 `scsi`、`nvme`、`nvme-subsystem`、`mmc`、`virtio`。

最近子系统不可识别时标为不支持，不改报更远祖先。整盘标为当次祖先观察；分区只从已核实的父盘推断，`subsystem_evidence` 会注明。SCSI 不表示一定是 SATA、SAS 或 USB；不按设备名猜测，也不解析叠加卷的底层盘。

只有核到单个 NVMe 控制器时才读内核的 `nvme_transport_sysfs`（如 `pcie`、`tcp`）；多路径命名空间不挑任意一条路径。该字段仅记录控制器报告的传输类型。未报告、不支持、权限不足及观察间原件变化分别显示为未知。

来源：[内核 sysfs 规则](https://docs.kernel.org/admin-guide/sysfs-rules.html)、[NVMe 控制器属性 ABI](https://docs.kernel.org/admin-guide/abi-stable.html)、[Linux v6.12 属性实现](https://github.com/torvalds/linux/blob/v6.12/drivers/nvme/host/sysfs.c)。

`disks` 只用已挂载的 `/sys`；可选 UUID 别名另用已核实的 `/dev`。只固定这两个顶层名字，不用目录打开标志或尾部斜杠触发其自动挂载。报告前再次核对同一 `/sys` 挂载和块索引，无法核实就不把原描述符的样本报告为当前结果；`/dev` 无法核实时只将别名标为未知。

此核对不把设备、挂载与计数变成同一个原子快照。

参见 [Linux 的 `O_PATH` 行为](https://man7.org/linux/man-pages/man2/open.2.html)和[目录查找触发条件](https://github.com/torvalds/linux/blob/v6.12/fs/namei.c#L1289-L1310)。

`--rates` 多取一帧内核累计计数，中间等约一秒；按**每个设备两次读计数完成时的单调时间间隔**输出读、写 B/s。Linux 在这里的一个扇区恒为 512 B，即使设备采用 4 KiB 扇区。设备生命周期、分区原件或几何变化、可见计数回退、缺少同一盘世代时不能给速率；

只有该方向的计数增量为零才报告 0；有实际传输但平均不足 1 B/s 显示为 `<1`。不是写入测试、性能上限、文件系统可用量或健康证明。基本资料/挂载或所请求速率不完整时保留其余行、退出 2；仅缺型号、UUID 或子系统/传输证据不影响基本清单。

`disks` 不做 SMART、自检或修复。

来源：[Linux 块层计数](https://docs.kernel.org/block/stat.html)、[内核设备世代](https://docs.kernel.org/admin-guide/abi-stable.html#sys-block-disk-diskseq)、[挂载命名空间字段](https://man7.org/linux/man-pages/man5/proc_pid_mountinfo.5.html)。

精确单个直连 PCIe NVMe 命名空间的只读探测见 [NVMe 健康观察](nvme-health.md)。
