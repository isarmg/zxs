# helper

Helper 是 Linux x86_64 的本机诊断、首次部署和离线维护工具。无参数只显示用法，无后台服务。

业务 Web 与嵌入的 Rust SMB 由同一个 ZXS 可执行文件直接提供。Helper 不负责启动、代理、启用或安装运行服务，不保存可供以后代用的路径、用户偏好或授权状态。

```sh
helper inspect-dirs /home/alice/downloads /home/alice/downloads/week-01 /mnt/incoming
helper compare-files /home/alice/reference.bin /mnt/incoming/candidate.bin
```

**目录观察。** `inspect-dirs` 接受 1–16 个现存绝对目录，包括 `/`。每行用输入序号标识目录；每对位置分别报告是否同一实体、内核返回的设备号是否相同，以及从原始路径实际看见的两个祖先方向。同一目录可以选两次，正常的父子关系不是错误。内容不被枚举，名称不回显。最多输出 120 对关系、32 KiB；同时保留的目录操作描述符不超过 18 个，不计进程原有描述符。不同设备号不证明独立物理盘，路径中看不见共同祖先也不是跨 bind 挂载的全局不相交证明。

**文件比较。** `compare-files` 要求本次选择的是两个不同、各只有一个硬链接且不超过 256 MiB 的普通文件。两端先以不读取内容的描述符核验身份和实际文件系统，再尝试打开任何一端读取。只受理 ext2/3/4、XFS、Btrfs 和 F2FS；未知文件系统、procfs/sysfs、tmpfs、网络、FUSE、overlay 等不支持，被拒绝的输入不会被探读。需要真实的本进程 procfs 和使用 `O_NOATIME` 的权限（通常为文件所有者）；不能满足即失败，不按名称回退重开。

比较按两块 64 KiB 缓冲区流式读取，即使首字节或长度不同，也要确认两端的完整长度和终点。输出前重核原位置及关键文件属性。返回码 `0` 表示文件相同（或目录观察完成），`1` 表示两份合格文件的内容确实不同，`2` 表示参数、权限、支持范围、变化或 I/O 错误使观察不能完成。结果只覆盖本次非原子读取；参照文件来源须独立核验。

两个通用命令都要求未经缩写的绝对路径，最多 4096 字节、128 个分量；拒绝符号链接和空、点、父级分量。没有默认扫描、修复、写入、覆盖或上传行为。更多边界及未来范围见[架构](ARCHITECTURE.md)。

## 本机磁盘面板

`helper disk-web` 监听本机临时回环端口。用终端的一次性配对码查看设备与挂载、被动 I/O 速率、CPU/内存及七类工具的固定安装位置。普通启动不会执行检查。终端用 `--fat32-copy`、`--exfat-copy`、`--ext4-copy`、`--xfs-copy`、`--ntfs-copy` 或 `--btrfs-copy` 加 `IMAGE --sha256 HEX` 只选定一份卷镜像，配对后预览并确认一次。两种 FAT 在启动时封存，ext4/XFS/NTFS/Btrfs 只在确认后持只读租约并比对整份摘要，不复制。网页不收路径、上传、工具参数或修复请求。见[用法和访问边界](docs/disk-web.md)。

## 在线查看磁盘、CPU 与内存

ZXS 运行时可另开终端，用 `helper disks --rates` 看宿主磁盘 I/O、用 `helper storage-status …` 看预选目录所在文件系统的容量，或在 `helper disk-web` 里手动点击对应按钮。观察按当前用户权限读取宿主，不取 SYSTEM 离线独占锁，不读池数据库、卷标或正文。面板要求本机前台的受限普通用户，**不要用 sudo 运行面板**。

`helper hardware` 不带参数，或在面板点 **读取 CPU 与内存**。CPU 只取约一秒内同一内核汇总计数的两次差值，显示执行、I/O 等待估计及虚拟机 CPU 时间被占用的比例；内存显示内核可用 RAM 总量和 `MemAvailable` 估计（CLI 为字节）。这不是 ZXS 进程用量、容器限额、磁盘忙碌率或硬件健康。缺失、异样或无法比较的读数标为未知，其他有效项仍显示；CLI 两项都得出结果返回 0，否则 2。只在显式命令或点按时采样，不运行基准、写入设备、修改性能参数或启用后台采集。内核数据来源见 [Linux procfs 文档](https://docs.kernel.org/filesystems/proc.html)。

池登记、恢复和账户操作须按各自流程核原件并独占 SYSTEM；镜像检查和复修各用独立选件、授权和隔离。

## 查看选定的存储根

```sh
helper storage-status --system /zxs/disks/system --recycle /zxs/disks/recycle --data /zxs/disks/data/photos --data /zxs/disks/data/video
```

明确给一个 SYSTEM 候选、一个回收站候选和 1–62 个普通数据候选；顺序不限。当前存储根参数的原始路径边界为：最多 4096 字节、1–256 个分量，不接受 `/`。不做 UTF‑8 转换；与通用命令一样不跟随链接，不允许空、点或父级分量。报告开头是采样的 Unix 毫秒时间窗，随后按角色和数据盘序号列出结果，不回显路径。**角色来自终端选择；登记身份和布局未核。** 此命令不枚举设备。

每个目录以自身持有的描述符取 Linux 挂载 ID、内核是否将它标为该挂载的根（可能不报告），再用该目录的文件系统统计取只读标志、总字节、全部空闲字节和非特权进程可用字节。挂载 ID 只在本机当次命名空间与该挂载的生命周期内使用，不是设备/文件系统 UUID。只读标志未设置不证明当前账户有写权限；可用量不是可保证的预留空间，也不能跨行相加当作独立磁盘容量。

只观察已挂载目录，不触发自动挂载。报告前重新解析原路径，核对全部可见祖先、目录实体与挂载 ID；身份不匹配不发布那一行的旧数据。若路径不存在、权限不足、挂载 ID 不可取或容量统计不可用，将保留其他行和仍可核的字段，返回 `2`。各行挂载与容量都采到时返回 `0`。结果按行采样，不是全机快照。

## 查看块设备、分区与 I/O 速率

```sh
helper disks
helper disks --device 259:1 --rates
```

块设备清单按 `主设备号:次设备号` 逐行列出，给出内核名称、整盘或分区、块层容量。分区另列实际父设备、分区号和起始扇区。名称不是固定的盘身份。“整盘”也可能是 loop、dm 或其他虚拟块设备。不会将父盘、分区和底层设备加总。对超过 256 个条目的系统，改用 `--device` 精确选择一项。

`direct_mount_ids` 只列本线程挂载表中超级块设备号直接匹配的挂载 ID；同一文件系统的 bind 挂载可能有多个。无直接匹配不表示未挂载或未被 swap、阵列、其他命名空间使用。`fs_uuid_udev_alias`、`partuuid_udev_alias` 来自受保护 devtmpfs 上当前分别指向该设备号的 udev 别名；它们是可陈旧的宿主缓存，不是现读介质、硬件序列号、登记 ID 或挂载 ID。无法安全归属的别名和无已知缓存型号的设备显示未知，不启动识别程序。只对设备节点取路径元数据，不打开设备读取扇区。

`ancestor_subsystem_sysfs` 是同一盘世代、在同一 sysfs 挂载中核对的最近有报告的非块层祖先子系统，最多处理 24 个设备路径分量。当前识别 `scsi`、`nvme`、`nvme-subsystem`、`mmc`、`virtio`。最近子系统不可识别时标为不支持，不改报更远祖先。整盘标为当次祖先观察；分区只从已核实的父盘推断，`subsystem_evidence` 会注明。SCSI 不表示一定是 SATA、SAS 或 USB；不按设备名猜测，也不解析叠加卷的底层盘。只有核到单个 NVMe 控制器时才读内核的 `nvme_transport_sysfs`（如 `pcie`、`tcp`）；多路径命名空间不挑任意一条路径。该字段仅记录控制器报告的传输类型。未报告、不支持、权限不足及观察间原件变化分别显示为未知。来源：[内核 sysfs 规则](https://docs.kernel.org/admin-guide/sysfs-rules.html)、[NVMe 控制器属性 ABI](https://docs.kernel.org/admin-guide/abi-stable.html)、[Linux v6.12 属性实现](https://github.com/torvalds/linux/blob/v6.12/drivers/nvme/host/sysfs.c)。

`disks` 只用已挂载的 `/sys`；可选 UUID 别名另用已核实的 `/dev`。只固定这两个顶层名字，不用目录打开标志或尾部斜杠触发其自动挂载。报告前再次核对同一 `/sys` 挂载和块索引，无法核实就不把原描述符的样本报告为当前结果；`/dev` 无法核实时只将别名标为未知。此核对不把设备、挂载与计数变成同一个原子快照。参见 [Linux 的 `O_PATH` 行为](https://man7.org/linux/man-pages/man2/open.2.html)和[目录查找触发条件](https://github.com/torvalds/linux/blob/v6.12/fs/namei.c#L1289-L1310)。

`--rates` 多取一帧内核累计计数，中间等约一秒；按**每个设备两次读计数完成时的单调时间间隔**输出读、写 B/s。Linux 在这里的一个扇区恒为 512 B，即使设备采用 4 KiB 扇区。设备生命周期、分区原件或几何变化、可见计数回退、缺少同一盘世代时不能给速率；只有该方向的计数增量为零才报告 0；有实际传输但平均不足 1 B/s 显示为 `<1`。不是写入测试、性能上限、文件系统可用量或健康证明。基本资料/挂载或所请求速率不完整时保留其余行、退出 2；仅缺型号、UUID 或子系统/传输证据不影响基本清单。`disks` 不做 SMART、自检或修复。来源：[Linux 块层计数](https://docs.kernel.org/block/stat.html)、[内核设备世代](https://docs.kernel.org/admin-guide/abi-stable.html#sys-block-disk-diskseq)、[挂载命名空间字段](https://man7.org/linux/man-pages/man5/proc_pid_mountinfo.5.html)。

## 查看检查工具是否在位

`helper fs-tools` 显示 ext4、NTFS、ZFS、XFS、Btrfs、exFAT、FAT32 各自的只读检查方案、对象和固定程序位置：FAT32/exFAT/ext4/XFS/NTFS/Btrfs 查私有发行工具槽，其他仅观察宿主槽，不能据此调用。可用 `helper fs-tools xfs` 只看一种，或对单个工具加 `--sha256` 与独立发行材料的摘要比较。**不会打开设备、调用检查器或修盘**，版本与签名仍为未核。详情见[七格式能力表、检测限制和返回码](docs/filesystem-tools.md)。

## 检查一份 FAT32 或 exFAT 镜像（只读）

`helper check-fat32-copy /绝对路径/volume.img --sha256 完整镜像的可信SHA256`。只接受已保存、最多 128 MiB 的普通 FAT32 卷镜像，不接受设备、分区表或任意工具参数。检查器只看到已核摘要的独立副本，在独立的 Linux 命名空间中运行 `fsck.fat -n`；不会修复或改写原镜像。[Debian 13 amd64 的发行配方](packaging/debian13-amd64/README.md)将上游工具与其运行库放在 helper 私有目录；不借用宿主同名程序，不自动安装。缺件或隔离不成立就拒绝。详见[配置、结果和限制](docs/fat32-copy-check.md)。

[`repair-fat32-copy`](docs/fat32-copy-check.md#只修新建的工作副本) 是另一项本机确认操作。固定 `fsck.fat -p` 仅处理新工作文件，可能截短或删除工作副本内的受损项，独立只读复查通过后无覆盖发布；不改原输入、磁盘或现有输出。可加 `web` 先从[独立配对页](docs/disk-web.md#可选由同机浏览器发起工作副本的终端确认)请求原终端确认；网页不执行。

exFAT 使用 `helper check-exfat-copy /绝对路径/volume.img --sha256 完整镜像的可信SHA256`。它只对 1–128 MiB、符合单卷边界的封存副本运行包内 `fsck.exfat -n`。返回 0 只表示此次检查未报告问题；1 表示明确报告问题或卷状态位须注意；2 表示未能确认。不会修盘，也不检查普通文件正文。[范围、结果与上游依据](docs/exfat-copy-check.md)。也可在终端按上述方式预选 exFAT，由配对页面确认同一项检查。

另有本机确认的 [`repair-exfat-copy`](docs/exfat-copy-check.md#只修新建的工作副本)：仅对新工作文件固定 `fsck.exfat -p`，再次只读检查后无覆盖发布。自动修复可能截短或删除工作副本里的损坏项，不改变输入、原盘或现有输出。可加 `web` 先请求同一终端确认，网页不执行。

## 检查一份 ext4 保存副本（只读）

`helper check-ext4-copy /绝对路径/volume.img --sha256 完整副本的独立SHA256`。从用户明确选择的本地普通镜像检查，不打开原盘，也不把镜像拷入内存或限制为 128 MiB。需保持实际只读租约及原件；用隔离的 Debian 原版 `e2fsck -fn`。外部日志、需要恢复、截短或未完整检查都不报正常。也可用上面的配对面板确认同次只读检查；更长任务用终端命令。[完整边界与返回码](docs/ext4-copy-check.md)见专页。

另有分开的本机前台 [`repair-ext4-copy`](docs/ext4-copy-check.md#只修新建的工作副本)：可用 `web` 先请求原终端确认，但网页不运行修复、也不选输入或输出。只对新的工作副本固定用 `e2fsck -fp`，独立 `-fn` 复查后才发布新文件，不改输入或磁盘。

## 检查一份 XFS 保存副本（只读）

`helper check-xfs-copy /绝对路径/volume.img --sha256 可信SHA256`。只对独立单卷副本运行固定 `xfs_repair -n -f`，可在本机配对网页确认一次。没有外部日志、实时卷或修复选项；脏日志、额外诊断和工具不完整均不报正常。[实际边界](docs/xfs-copy-check.md)。

## 查看一份 NTFS 保存副本（有限只读）

`helper check-ntfs-copy /绝对路径/volume.img --sha256 可信SHA256`。包内 `ntfsfix -n` 仅观察独立单卷副本的有限 MFT/镜像记录和备用启动扇区；配对网页也可确认一次。未验证日志/Windows 休眠或正文完整性，不是 chkdsk，不修复、不打开原盘。[用法与结果](docs/ntfs-copy-check.md)。

## 检查一份 Btrfs 保存副本（只读、单设备）

`helper check-btrfs-copy /绝对路径/volume.img --sha256 可信SHA256`。固定上游 `btrfs check --readonly` 只对自包含的单设备完整原始副本做结构检查；也可通过配对面板确认。不会查宿主其他设备，不支持多设备、恢复或修复，也不证明普通文件或每份镜像无误。[用法与边界](docs/btrfs-copy-check.md)。

## 本机部署与离线维护

部署默认从 `/zxs` 开始：独立可执行文件在 `/zxs/bin/zxs`，ZXS 权属状态只在 `/zxs/state`（`deployment.v1` 与 `<instance-hex>/`），SYSTEM 是 `/zxs/disks/system/SYSTEM.sqlite3`，回收卷是 `/zxs/disks/recycle`，普通卷按明确选择用 `/zxs/disks/data/<标签>`。可在相关命令中显式指定合格的可执行文件或媒体路径；复查和重置只认已登记的原路径，不找旧部署目录。目录与挂载由 Linux 管理员事先准备；Helper 不配置挂载或日志、不安装服务、也不格式化或迁移任何卷。

[共用账户离线重置与只读回执](docs/shared-account-reset.md)只认确切原 v19 站点与同一原前序。Web 与 SMB 使用一个用户名和口令；两者只能切到同一已确认代次。该写路径默认禁用，不接管未知暂存。

- [无状态发现](docs/discovery.md)只根据本次实际挂载和固定的 SYSTEM 元数据，或本会话给出的绝对目录，报告候选。候选不是授权，不自动选盘或初始化。
- [新池、同一原 INIT 续作与查询](docs/sqlite-first-install.md)要求当前有前台授权、明确提供原 INIT 主体和数值 UID/GID；不设置 Helper 偏好或保存授权。继续原单需要原件及原标记，查询不自动修改。
- [新 v19 私有首设](docs/fresh-deployment.md)可从原始 INIT 依据、受保护单 ELF 路径（默认 `/zxs/bin/zxs`）、独立预期 SHA-256 和实际前台确认，接原低权限的 SQLite 与新凭据发行。此处没有迁移、服务启停或自动接管。
- [可选内嵌精确 ZXS 文件](docs/embedded-image.md)另用独立批准的完整 v19 ELF；只允许一次显式、不覆盖的文件落位，不发账户、不启动 ZXS。默认 Helper 无负载；本源码没有交付已核验的内嵌制品。
- [复查原 ZXS 部署](docs/reattach-check.md)使用 ZXS 自己的受保护持久依据，只有只读结论，不采纳、不重装、不授权维护。
- [普通卷退登记预检](docs/retire-ordinary-preflight.md)只核本次真实原件与已知阻挡，不是真的退登记，也不表示可移除、擦除或卸载。

此版本不开放空盘追加或非空导入。

## 按需使用同一个 Helper 文件

直接从本机受保护位置执行该 ELF，例如 `./helper --help`，不需要先安装到固定路径。特权离线操作仍必须从真实宿主的 root 前台进行，使用 root:root 0755 的唯一链接本机文件；只读非特权的副本检查可使用当前用户所有的受保护文件。内部工作进程只传递已经运行的原 ELF 描述符，不从 PATH 或文件名另选程序。需要发行凭据的首设仍要给出独立预期的 Helper 全文件 SHA-256；不会把从自身读出的值当作批准。已删除、替换、额外链接、扩权或来源不能确认时停止。通用盘工具仍按各自受保护的上游材料配置核对，不是 ZXS 的运行时依赖。

## 独立开发

本目录有独立 Cargo 工程、Rust 工具链和[依赖来源记录](DEPENDENCIES.md)。允许验证时，入口为 `cargo build --locked` 和 `cargo test --locked`。后者的正向文件用例须在上述受支持文件系统上的专用临时位置创建自己的测试文件；如系统临时目录不满足，可由操作者设置 `HELPER_TEST_DATA_PARENT` 指向合格的专用目录。该变量仅供测试使用；先决条件缺失是测试失败，不算验证通过。

尚未安装、运行现场服务或操作实际介质；冷启动和 Web/SMB 登录待现场验收。块设备 CLI 验收须显式提供稳定的试验设备号 `HELPER_TEST_BLOCK_DEV`。独立构建和发行需要哪些输入与证据，见[交付核对表](RELEASE.md)；待办见[项目任务清单](TASKS.md)。
