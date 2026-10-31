# 最终本地 RPC 成对性能与 SMB 组合语义验收

本验收只覆盖固定源码、GitHub Linux runner 上的开发池和真实 Unix RPC 实现。主线基线为 `d3eefd289e53a2803703870fe4250d08cadff9b5`，对照为 `e2f33d9882cf7263412d9370b6a703e0b216357d`。控制器固定 SHA 与 tree，不随 main 漂移，不采用被拒绝的 `2af128b5` 架构。

已有完整 CI [41154826502](https://github.com/isarmg/zxs/actions/runs/41154826502) 的 17 个任务，以及边界验收 [41158008645](https://github.com/isarmg/zxs/actions/runs/41158008645) 的 7 样本／21 阶段是本轮前置证据，不重复执行。规模后续由原增长验收作者负责；这里的 22／220 历史夹具仅复用原 RPC 工作负载。

## 不可改写的历史结果

原工程 screen [41062775195](https://github.com/isarmg/zxs/actions/runs/41062775195) 在 220 历史 BA 的第二次保存出现 1320.77 ms；总体 save P95 比对照增加 384.6 ms，超过 +50 ms，因此采用仍为 **FAIL**。诊断 [41071838794](https://github.com/isarmg/zxs/actions/runs/41071838794) 未复现孤立尖峰，不能解释异常或取消失败。本轮是当前 main 的独立验收，不把另一次通过当作原结果的修复。

## 有限且可复现的执行合同

入口是 [screen.py](../../tools/local-rpc-acceptance/screen.py)，只允许在 GitHub Actions 执行负载。仓库保存 [workflow 模板](../../tools/local-rpc-acceptance/workflow.yml)，由独立验证 carrier 分支将其接到已登记的 `ci.yml` 并手动 dispatch。必填 `validation_sha` 指向保留正常 CI 的精确候选提交；所有验证和控制器 checkout 都使用该提交，记录其 SHA／tree，carrier 只提供启动工作流，避免替换后的 CI 破坏正常静态合同。候选分支保留正常 CI，不写 main；整合负责人只整合源码、测试、文档和模板，不整合 carrier 对 CI 的替换。

机械检查、受影响格式／严格诊断、正常 SMB RPC 和协议消费者测试先行，之后才构建并运行负载。两个产品 checkout 均从固定 tree 起步，仅加根 `test/` 的 seed、共同 RPC 测试、明确版本的读适配和 Cargo／模块接线；构建前后记录基础 tree、有效测试 tree、所有覆盖文件 SHA256、实际 features、编译产物哈希和控制器身份。没有生产源码、FSYNC、FULL、副本、收据、门槛或缓存改动。

使用仓库固定 Rust 1.99.0、release、`x86_64-unknown-linux-gnu`、通用 x86-64 CPU、两个构建作业、空 Cargo home、锁定离线依赖。两臂都使用既有开发测试图的 `fault-injection` 特性，不触发故障点。每对在同一 runner 顺序执行 AB 或 BA，子进程绑定相同两个允许 CPU，显式设置 Rayon／Tokio 两线程，并记录 CPU、内存、cgroup、亲和性和磁盘环境。runner 不是目标 8 核／32GB／12 盘机器。

只执行四对：22 历史 AB／BA、220 历史 AB／BA。每臂 500 文件、97 目录、一个文件分支，初始夹具和独立完整 oracle 复用诊断控制器 `3ff0b11d`；原非嵌套 RPC 总计时复用 `08b0e6e`。1000 次完整读和目录循环、20 次真实提交 Close，保存计时包含 Open、写入准入／Dirty、正文写入、关闭本地描述符和真实 Close。这里没有函数级归因计时。

两版本的实际只读协议有意保持各自产品实现：对照 AuthorizeRead 后从不可变 FD pread，当前 main 使用有完整性证明的 Read RPC；两者均校验实际正文并 Close。这是两份产品的端到端比较，不用 FD 捷径绕过当前安全合同，也不向产品添加旧接口兼容。

原 20 次保存和独立重开 oracle 完成后，另一次进程在该臂的同一夹具上执行 40 轮尾延迟样本；开始时历史是 42／240。每轮新文件 8MiB，以 64KiB 块正常写入，单独计 Flush；续写 64KiB，再单独计 Close；第二个新文件以一次 8MiB write-through mutation 计完整准入、正文写入和同步发布。之后干净 Close 不产生新提交。每臂共 120 个新增发布、80 个文件。独立进程重开后逐一验证原操作 ID、顺序、永久发布收据、TargetMet、历史正文和当前版本，不把正确性检查放入计时。性能夹具仍为原单开发盘／一个 FULL，三副本合同由下述组合语义覆盖。

每阶段最多 1200 秒，8GiB 地址空间上限，至少 8GiB 可用空间；整个负载截止时间为构建前 90 分钟，pair job 最多 45 分钟。超时、磁盘不足、子进程失败或缺少样本明确中止，产物尽力保留。无自动重复，不能因噪声或失败重跑到绿色。

## 判定

四对分别判定，任何一对失败即最终 FAIL；缺臂、缺阶段、错误 manifest、错误顺序或样本不全不能通过。原阈值保持：

| 指标 | 当前 main 相对对照的限制 |
| --- | --- |
| 完整读／目录 P95 | 增量 ≤ 2ms |
| 完整读／目录 P99 | 增量 ≤ 5ms |
| 20 次保存总时长 | 增量 ≤ 1000ms |
| 原保存 P95 | 增量 ≤ 50ms |
| 220 历史打开 VmHWM | ≤ 对照的 80% |
| 220 历史打开耗时 | ≤ 对照的 110% |
| 新增 Flush／Close／write-through P95 | 各自增量 ≤ 50ms |

保留全部原始纳秒样本，以及 P50／P95／P99／最大值。nearest-rank 算法下，20 次保存的 P99 和 40 次尾延迟的 P99 都是最大值；它们不证明总体高分位或用户 SLO。检查前的完整读取和 inventory 可能预热 OS cache；不声称冷设备性能。summary 发现失败会以非零退出，不将 GitHub 执行成功偷换成门槛通过。

## 有界 SMB 组合语义

常规活动测试 [bounded_lifecycle_tests.rs](../../test/server/unit/smb_rpc/bounded_lifecycle_tests.rs) 通过 `FrameStream` 和生产 `run_connection`、真实 Engine、三个独立完整开发副本执行 60 轮、六次完整引擎重开，最后再重开核验。只使用两个文件、一个分支。

每四轮覆盖 Flush 后续写再 Close、write-through 后干净 Close、Close 的真实线回复丢失、Flush 的真实线回复丢失后断开。丢回复在发送完整请求前关闭接收方向，保留发送方向直到完整帧写出；join 确认服务器已处理，明确匹配 BrokenPipe／ConnectionReset，不吞掉业务错误。测试读取工作记录中原操作 ID，Flush 后按描述符身份找到新代工作记录，不使用过期 workspace ID。

每次按原 ID 查询已发布结果，不重发提交；检查锁已释放。每次重开前后，75 个发布最终全部保留原 ID、主体、单调顺序、TargetMet、历史正文和当前正文；当前健康检查实际扫描独立 FULL 及发布证明。断连保留的工作证据交给既有恢复预算准入，不不安全删除。没有任意 sleep、常驻缓存或生产测试 hook。

## 实际结果登记

新增机械回归、格式、严格 lint、RPC 语义及成对负载的实际执行状态以最终交付的 run、SHA／tree 和产物为准。模板或任务启动不代表完成。原采用 FAIL 与尖峰未解释状态始终保留。

## 只需人工／物理环境执行的项目

- 在指定 32GB／8 核／512GB 系统盘和 12×15.6TB 数据盘上记录型号、固件、文件系统、占用量及完整副本拓扑，执行同一有限合同，验证稳态／拥塞时读写尾延迟与资源预算；GitHub 开发盘数据不能外推。
- 在实际 Windows／macOS 与拟采用 Office、照片／视频／文档应用的准确版本，经过真实 Samba 网络路径执行固定工作日场景；逐项核对打开、保存、Flush、Close、write-through、文件替换、重连和历史正文，不以 SDK 编译或 Unix RPC 代替办公客户端兼容。
- 在具备独立恢复条件的物理测试池进行保存中的真实掉电、控制器／盘掉线、网络中断与服务重启；按原 ID 查证未知回复，检查全部 FULL 和恢复证据。模拟断线与正常进程重开不证明电源故障后的硬件持久性。
