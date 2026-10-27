# 0007：同代磁盘资源所有权与 FD 相对 I/O

状态：当前源码设计；不代表测试、性能或真实设备验收已经通过。

## 背景与选择

配置中的路径是打开磁盘的地址，不是每次读写都要重新查找的资源身份。逐操作重新扫描 mountinfo、sysfs 和根路径，再用绝对路径打开正文或元数据，既重复准入，也不能把前一次身份检查与后一次实际 I/O 绑定起来。

`Engine` 持有一代 `Arc<DiskResources>`；每个已准入成员保存根目录 FD、实际设备身份和 seal。初始化、重开及停服重配重新执行真实设备、seal、物理独立性准入。冷启动允许缺失成员的既有规则不变；该代缺失成员不会因为同名目录后来出现而自动接入。部署不提供在线改变底层 Btrfs 设备集合的管理协议，不能用每次提交轮询拓扑替代停服切代。

盘内路径只作为该根下的地址。`RelativePath` / `EntryName` 保留路径形状边界，`openat2` 保留无符号链接与同挂载约束；创建、改名、删除、遍历和目录同步使用已持有父 FD，容量读取使用根 FD 的 `fstatvfs`。闭包包含容器遍历和 marker、正文与清单、本地证明末端的 `purged.json`、对象图、journal、工作区镜像、修复、checkpoint 和 scrub，不仅是正文末端的 open。

根目录改名、命名路径被替换或 lazy unmount 不使仍有效的 FD 转向新盘。后续操作继续对原资源产生真实 I/O；设备、文件或同步实际失败照常传播。`check`、`MetadataMemberProbe`、`DiskObservationPlan`、操作回执的 `current_health`、`version_health` 和公开容量诊断仍采样当前命名路径，既不借用旧观察结果作为准入，也不以资源 owner 冒充当前路径在线。

冷生产准入从持有根 FD 取得 `statx(AT_EMPTY_PATH, STATX_MNT_ID)`，以该 mount ID 选择 mountinfo 行，再按原规则检查设备、Btrfs UUID 与物理独立性；不再用稍后对命名根的 canonicalize 结果给旧 FD 配身份。部署内核必须返回 `STATX_MNT_ID`，且准入时该 mount ID 必须仍在当前命名空间的 mountinfo 中。缺少返回字段或对应行分别返回 `DISK_IDENTITY_MISMATCH`（`kernel did not report the held disk root mount identity` / `held disk root mount is absent from mountinfo`）；系统调用失败保留原 I/O 错误，不猜测路径或降级回退。这不改变已经完成准入后 lazy unmount 的普通 FD I/O 语义。

## 生命周期与证明

- Planning、detached publication、内容读计划和 scrub 共享同一资源代际。决定点拒绝其他资源代际的候选；配置和候选成员绑定仍独立校验。
- writer lock 仍由 `Engine::_locks` 及写准备的 `retain_writer_locks` 持有。磁盘 owner 不包含这些锁；读计划可延长根 FD 与读租约，但不能延长 writer lock。
- 根 FD 仅证明资源定位，不证明 bytes 健康。实际 seal 字节、成员证明、对象摘要、正文分块/完整摘要、预算、取消、租约及元数据损坏撤销继续在原责任边界执行。
- 普通决定在全部 PREPARE 后、epoch fence 和首个 ACCEPT 前，对元数据成员及 required payload 成员各从 held root 实读并校验一次 seal。恢复 journal 准入及恢复 preview/commit、工作区证据、导出/维护证明来源也保留各自的 seal 准入；后续内容和证明读取不是 owner 身份的替代品。
- 清理的证明来源准入与破坏性目标准入是两个边界：每个本次实际处理的 distinct slot 在首次 tombstone/unlink 前实读一次 seal，其后的内部步骤复用本次准入，不逐版本轮询。它不等价于旧实现每个版本重新观察 seal 的窗口；source 准入后、target 准入前的损坏仍阻止该目标写入和删除。tombstone 持久化、unlink、父目录同步顺序不变。
- 固定 N/Q、PREPARE/ACCEPT/FINALIZE/PUBLICATION、原有文件与目录同步顺序不变。不存在 TTL、校验成功缓存、降级门槛或第二套业务读写实现。

## 代价与边界

常驻根 FD 和资源记录为 O(已准入磁盘数)，各计划共享它们；枚举仍随实际条目数，内容校验仍随实际 bytes，对象图仍随原证明范围。已存在目录的相对操作随路径深度解析，不扫描全机拓扑；缺失目录创建保留逐层创建与文件系统同步。该变化不减少发布阶段和新材料所需的持久化屏障，也不消除已有图/历史全量工作。

系统盘、调用方输入/导出目标、外部历史包和独立离线工具仍是各自的路径准入边界；它们不借用 Engine 数据盘身份。显式诊断仍承担实际拓扑采样成本。性能结论必须另附绑定源码与环境的测量，不能从减少轮询推导端到端加速。

内部复制使用显式 `ReadSource`：调用方/历史包输入按命名路径准入，盘内来源由生产者携带 `Arc<DiskHandle>`。公开 `stage_file` / upload 的 `source: &Path` 始终是外部输入，即使字符串位于某个配置盘根之下，也不猜成旧盘资源。持久化 `StagedFile` 仅在其特定 workspace namespace 的解码边界恢复 owner，并在 journal 恢复后另核 authoritative primary；重新打开来源时仍校验原有大小/内容摘要，不能靠保留旧 File 跳过来源变更检查。

immutable 相同字节快路径也在 held parent 上以 `openat2` 的 `NO_XDEV` 打开 leaf，并对同一 File 读字节、执行同步，不能跨入启动前已 bind mount 的普通 leaf。隔离 user/mount namespace 的两个回归需 runner 提供 util-linux 且允许 namespace 内挂载；默认忽略，必须显式运行，权限或命令失败不当作成功。源码交付不代表这些回归或真实 Btrfs 硬件已经运行通过。

## 替代方案

拒绝缓存 `validate_seal` 结果：缓存没有取得实际资源所有权，不能防止后续绝对路径转向其他目录。拒绝每次检查命名根是否仍指向原 FD：支持的普通 I/O 应跟随原资源，显式诊断才回答路径当前状态。也不新增通用文件系统 trait 或兼容双轨；平台只提供已有安全类型所需的相对目录原语，业务算法共用同一实际闭包。

关联：[Engine](../components/engine.md)、[增量发布完整性](0005-incremental-publication-integrity.md)、[工程生命周期规则](../20-engineering.md#s20-07)。
