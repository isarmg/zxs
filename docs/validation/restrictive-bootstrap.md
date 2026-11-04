# 限制启动发现与停盘恢复

本批源码把保护在 ZXS 系统状态目录 `bootstrap-discovery/` 中的 ledger 接入
`Engine::open`、held recovery 和显式 activation resume。首次新池初始化从已准入
held directories 建立 locator。普通启动必须先读完整 ledger，随后才允许任何数据
根 stat/open；不存在从缺失、损坏、部分写入或不确定 locator 回退到旧 Config
逐根扫描的路径。系统目录或配置丢失也只返回明确管理恢复错误。

## 发现限制和真实权威

locator 的根、目录 identity、stop operation/intent 摘要只限制地址，不构成登记、
代际或发布权威。发现先建立固定探测列表，再核对已打开目录的实际 device/inode，
之后读取 seal、marker、joint 和完整本地 archive。候选 marker 中的配置、旧 fence
和归档路径均不得扩大此列表。恢复原配置仍按真实保存的 genesis 全部成员及固定 Q；
恢复后继仍按完整 portable lineage 的实际 N/Q、每个根的真实控制证据及冻结归档。
任意编辑 hint 的 voter 标志不会减小权威分母或选择另一个池。

损坏的 `InvalidJson`/`ObjectCorrupt` marker 可以隔离，但不跟随该根的 fence；
余下根必须达到所选后继的固定 Q，且全部执行实际本地证据检查。ConfigConflict、
LogFork 仍明确失败，全部 marker 损坏也不能回退 genesis。

真实 stop 逐项匹配已认证 Exit lineage。控制准备先在保留根保存
`metadata/membership-exits/<intent_hash>/authority.json`；stop 后、active marker 前
崩溃时，受限制发现可读取这份完整 portable authority。普通业务准入仍要求当前 seal
和日志；显式 activation resume 才可补完允许的 promotion。已选择当前 Exit/Abort
marker 或 seal 却丢失日志时，不能从完好 archive 重建空日志。

`ValidatedConfig::io_disks()` 仅约束本次实际资源，不序列化、不修改完整历史登记、
旧 Candidate、原始证明或元数据 N/Q。`DiskResources` 的每个打开入口必须在第一个
syscall 前使用此集合。全局 logical I/O domain 的关闭检查在 replay 之前进行；旧
Engine 的 reader/gate 尚未关闭时明确拒绝。重启排除证明不是 live closed proof，
必须与独立的全进程 domain guard 组合，后者维持到 Engine 销毁。

## 添加、移动和显式恢复

纯添加从已认证 successor 的 held resources 在首个 marker 前扩展 ledger。
扩展不能删除旧 identity 或已有 stop，不能借配置文本授予新根权威。没有任何 stop
且完整配置覆盖 ledger 的阶段，支持同一已观察 directory identity 的路径移动；
实际 identity 在内容读取前重新核对。首次 stop 前从已认证 held resources 把移动
后的地址持久化。出现 stop 后地址变化不能自动生效，须明确 retained-root recovery。

`restore_from_disks` 只读取调用方明确提供的保留元数据根。单根 anchor 解析只能从
该根读取完整 archive，不能通过保存的 Config 读取其它根。通过真实固定多数和
本地证据后，新系统 ledger 只记录明确提供的根；未列地址保持禁止 I/O，历史成员
身份和分母不变。丢失系统 ledger 后的停止证明来自认证 Exit 和持久的地址排除，
不会为了补写目标 observation 而打开目标，也不在目标上生成恢复文件或备份。

需要恢复非投票数据根时，使用独立的
`restore_from_disks_and_data_roots`（及 workspace-limits 变体），同时传入保留元数据根
与显式 `RecoveryDataRoot`。活动 Engine 的 `recovery_data_roots()` 只从已经持有的
非投票资源导出准确登记和 device/inode；调用方须在 SYSTEM 丢失前可信保存这些绑定。
不能从恢复目标自己的 `disk.json` 重新推导“预期”身份。尤其 Genesis 数据盘没有
独立的后续 Add 身份证书，单凭复制后可重写的 seal 无法证明未发生设备身份替换。

新入口先完成真实 metadata 固定多数选择，再访问显式数据根。数据根永不计票，且须
匹配已认证当前登记、可信的原物理身份、当前 seal 与本地配置/代际锚；新增数据盘还须
通过 portable data dependency 和本地 readiness 验证。stop、detach、错误角色或
身份替换均拒绝，取得实际 Engine handles 后再次复验，随后才记录到新系统 locator。
没有显式传入的数据路径仍禁止探测；该入口不提供身份重绑定或隐式恢复权限。

ledger 保留不可变历史和完整 hash chain。root 扩展、已有 identity 不变的无 stop
移动和新增 stop 均使用原有 file/rename/directory 屏障；不删除旧记录。对象仍有
128 KiB 边界，根数组分页及最后投票成员的独立终止权威尚未实现，不能形成零 quorum
的服务池。整个控制目录被外部完整回滚仍不在这个本地 ledger 的信任保证内。

## 源码测试与验证边界

新增/扩展源码测试覆盖：生产启动的第一 stat/open 边界、旧配置仍列出且物理仍在线
的目标、缺失/损坏 locator 与系统目录、合法重新编码但任意编辑的 hint 不能缩小 Q、
仅指定保留根的恢复及再次重开、添加后保留 stop 和拒绝 identity 重绑定。原有测试
继续覆盖 canonical 编码、完整链、孤儿记录、未知原子文件、错误 hash、写屏障中断、
目录替换和同文件系统不同目录。

本批只执行直接 rustfmt 和 git diff 检查，没有编译、测试、Clippy、模型、CI 或真实
目录生命周期操作。完成集成仍需组合退出权威/D3、当前命名空间消费者以及进程级
I/O closure 的准确源码，并验证所有新增调用入口。这里不是物理掉电验收、最后成员
终止、真实部署或下线/删除完整交付声明。
