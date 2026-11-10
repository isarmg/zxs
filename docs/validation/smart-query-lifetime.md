# SMART 查询生命周期边界

[平台组件](../components/platform.md) · [健康观察契约](../contracts/README.md)

## 目标与实现边界

本轮把目标设备的真实健康查询从外部 `smartctl` 子进程移回服务进程内，避免服务退出后
孤儿子进程继续向目标设备提交 I/O。公开健康字段不变；ATA 仍使用官方解码器的型号／固件
数据库，不以通用属性编号猜测厂商格式。同步命令的实际返回是释放准入的条件，期限或信号
不是完成证明。进程退出与新 Engine 的重新准入仍须和引擎的进程所有权围栏共同验证。

可选原生采集仅接受直接 NVMe 命名空间或 sysfs 标为 ATA 的 SAT 设备。
真实设备访问全部由服务内唯一工作线程执行同步只读 ioctl：NVMe 只读命名空间标识、
Identify Controller 与 SMART/Health Log；SAT 只允许 CHECK POWER MODE、IDENTIFY DEVICE、
SMART READ DATA、READ THRESHOLDS 与 RETURN STATUS。没有通用透传入口，不启用 SMART、
启动自检、更改设置或发送电源状态更改命令。ATA 待机和 ACTIVE_NV_DOWN 返回 `low_power`；
电源检查失败、返回未知值或不完整寄存器时停止后续命令，保留错误，不把任意失败猜成休眠。
复杂多设备映射不猜测单盘。SMART 是设备自报观察，不是物理可靠性保证；NVMe 数据可能由
控制器统一报告，不证明不同命名空间是独立硬盘。

ATA 属性的型号／固件数据库解释仍由已安装的 `/usr/sbin/smartctl` 提供，但它只执行
`--json --health --attributes -` 的官方 stdin replay 模式：固定匿名命令记录和最多三个
512-byte 只读回复编码为小于 8 KiB 的输入，经匿名 memfd 交给解码器。目标设备路径、
`/proc/<pid>/fd/<fd>` 和设备 FD 都不交给子进程；服务持有的设备和 sysfs FD 均为 CLOEXEC。
官方 `smartctl` 对最后参数 `-` 直接选择 `parsed_ata_device`，仅从 stdin 构建内存回复表；
缺记录或解析失败不回退到设备查询。保留安装版本的属性单位、温度、通电时间、当前／历史
阈值告警及退出位解释，不以固定属性编号猜测厂商格式。NVMe 标准字段由原生代码读取，
不依赖外部工具；128-bit 通电小时数超出接口 u64 范围时拒绝，不截断。

采集持有只读、不可跟随符号链接的块设备 FD，同时持有 sysfs 设备 inode，
在查询前后核对 inode、设备号、挂载 ID、目录到整盘的关系和依赖数。
结果返回前再次核对当前管理员授权、登记身份、seal 和目录 inode。每条原生命令前检查
目录是否仍运行及剩余期限。采集与解码共用 2 秒预算，stdout 上限 64 KiB，HTTP 侧
等候上限 3 秒；驱动超时只是提交给内核的期限，不能当作实际 I/O 已结束的证明。
若同步 ioctl 尚未返回，唯一线程继续持有目录准入和采集槽，后续请求返回 `busy`，
不积累线程。离线解码超限／超时仍终止并回收子进程，但该子进程不承担目标设备 I/O。
这消除了服务崩溃后孤儿 SMART 子进程继续访问设备的路径；重启准入仍依赖引擎的进程
所有权围栏，不能以 HTTP 超时或发出终止信号代替实际排空。每次刷新重新观察，
无持久健康缓存或后台轮询。

默认 systemd 示例的 `DevicePolicy=closed` 和空 capabilities 不授予原始设备查询能力，
因此通常显示权限不可用；本功能不修改服务权限，也不会安装 smartmontools。
缺少 ATA 解码器、权限不足、设备不支持、身份变化、超时和无效结果均不可用。
真实设备验收须由操作者另行授权；默认自动测试仅使用合成字节、临时 sysfs 夹具及不接触
设备的假解码命令。独立的已安装 smartctl replay 回归明确标记为忽略，须显式运行，
同样只消费合成输入，不以其成功代替物理硬盘验收。

平台 ABI 和同步完成边界依据
[Linux NVMe UAPI](https://github.com/torvalds/linux/blob/v6.12/include/uapi/linux/nvme_ioctl.h)、
[Linux NVMe ioctl](https://github.com/torvalds/linux/blob/v6.12/drivers/nvme/host/ioctl.c) 和
[Linux block-device SG_IO](https://github.com/torvalds/linux/blob/v6.12/drivers/scsi/scsi_ioctl.c)，
二者经 [blk_execute_rq](https://github.com/torvalds/linux/blob/v6.12/block/blk-mq.c#L1285-L1318) 等待完成。
ATA sense 格式参考 [T10 SAT-2 提案](https://www.t10.org/ftp/t10/document.08/08-344r0.pdf)。
离线解码契约依据 [smartmontools 7.5 官方手册](https://github.com/smartmontools/smartmontools/blob/RELEASE_7_5/smartmontools/smartctl.8.in)、
[入口分派](https://github.com/smartmontools/smartmontools/blob/RELEASE_7_5/smartmontools/smartctl.cpp#L1550-L1563) 和
[parsed_ata_device 实现](https://github.com/smartmontools/smartmontools/blob/RELEASE_7_5/smartmontools/atacmds.cpp#L2437-L2636)。
本实现不自动扫描设备，也不执行写设置或自检命令。

## 类型与错误边界

平台 `BlockDevice` 只从 `fstat` 验证过的块设备 FD 构造；不接受 `/dev/sg` 字符设备，
因为其请求中断与保留语义不同。服务端原本就只绑定块设备，这不是新增设备类型限制。
只借用持有的设备 FD，不打开设备路径，不转移目录准入；固定命令及缓冲区始终存活到
同步 ioctl 返回。`CommandTimeout` 拒绝零和 SG_IO 的无限期限哨兵，逐条使用同一查询
预算的剩余时间，而不是给每条命令重新分配两秒。

NVMe、SCSI host/driver、sense 和 ATA 状态保留其具体错误来源。SAT 电源检查只在明确
运行／空闲时继续；失败、未知状态和缺少寄存器均不冒充休眠。完整 SMART 状态签名
决定通过／失败；只有确实缺少状态寄存器时才按官方 ENOSYS replay 让解码器按既有属性
回退，畸形或部分状态签名直接拒绝，不能作为“通过”。完整 sense 长度与格式都必须有效，
不模拟某些 smartctl 设备兼容分支对损坏回复的宽容解释。禁用或无法确认已启用的 SMART
不自动启用；保持官方默认非 permissive 查询的不可用含义。

## 验证范围与后续检查

本轮只完成源码审阅、官方 `rustfmt --check` 与 `git diff --check`。编译、单元测试、
Clippy、CI、已安装解码器运行和真实设备验收均未执行。以下是待执行入口，不是通过报告：

- `cargo test -p platform-io --lib disk_smart`：固定命令编码、ABI 大小／偏移、超时边界、
  非块设备拒绝、标准 NVMe 字段、SAT sense 和电源／健康状态。
- `cargo test -p storage-server --lib disk_health`：原身份校验、取消、单槽／子进程回收、
  有界 replay、默认解码参数和健康／错误投影。
- 有可信已安装 smartctl 的环境可显式运行
  `cargo test -p storage-server --lib installed_smartctl_offline_replay_preserves_real_health_and_metrics -- --ignored`。
  此忽略用例只提供合成 IDENTIFY、属性及阈值扇区，核对通过／失败、温度和通电小时数，
  不打开任何真实设备。解码器版本变化须复核上述官方入口链，解析错误不得改为在线回退。
- 整合后的平台／服务构建、严格 lint、契约与来源治理检查，以及进程崩溃和 Engine
  重新准入测试由最终验证批次记录。测试源码存在、纯合成 replay 成功或格式检查通过
  均不能证明真实控制器、驱动超时、设备权限或物理设备兼容性。

本轮没有实际设备测试授权，不执行设备命令、挂载、格式化、权限变更或部署。
