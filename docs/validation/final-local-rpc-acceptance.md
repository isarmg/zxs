# 最终本地 RPC 成对性能与 SMB 组合语义验收

本验收只覆盖固定源码、GitHub Linux runner 上的开发池和真实 Unix RPC 实现。冻结 candidate（当时 main）为 `d3eefd289e53a2803703870fe4250d08cadff9b5`，对照为 `e2f33d9882cf7263412d9370b6a703e0b216357d`。控制器固定 SHA 与 tree，不随 main 漂移，不采用被拒绝的 `2af128b5` 架构。

已有完整 CI [41154826502](https://github.com/isarmg/zxs/actions/runs/41154826502) 的 17 个任务，以及边界验收 [41158008645](https://github.com/isarmg/zxs/actions/runs/41158008645) 的 7 样本／21 阶段是本轮前置证据，不重复执行。规模后续由原增长验收作者负责；这里的 22／220 历史夹具仅复用原 RPC 工作负载。

## 当前交付政策（2026-10-31 08:28 UTC 生效）

用户要求先完成全部任务，再逐步提高性能和效率；启动时间相对对照 ≤1.1 倍、
VmHWM 降低 20%、保存 P95 增量 ≤50ms 三项助手工程目标改为**非阻断实测指标**。
仅这三项不再阻止交付或触发追加优化；读／目录、保存批次、Flush／Close／write-through
等其余现有门槛不变。正确性、认证、原操作 ID／永久历史／FULL、独立 oracle、
64MiB／128MiB 产品额度、45 分钟／单阶段 1200 秒等实验资源预算均不改变。

新报告保留原八项／十一项比较的原始数值、阈值与 `passed`，并单列
`delivery_policy`、每项 `blocking` 和 `delivery_gate`。旧门槛可仍为 FAIL，
同时当前策略允许交付；这不是把旧结果改为 PASS，也不证明性能目标已达到。
缺失 pair、身份不符、oracle 失败或连续容量样本不足仍不通过，未测能力不写为通过。
现跑冻结任务按原合同收取终态，不改其 seal／预算，不为上述旧目标重复实验。
后文旧“原门槛”“FAIL／非零”等文字描述对应历史版本及证据，不覆盖本节生效政策。

## 不可改写的历史结果

原工程 screen [41062775195](https://github.com/isarmg/zxs/actions/runs/41062775195) 在 220 历史 BA 的第二次保存出现 1320.77 ms；总体 save P95 比对照增加 384.6 ms，超过 +50 ms，因此采用仍为 **FAIL**。诊断 [41071838794](https://github.com/isarmg/zxs/actions/runs/41071838794) 未复现孤立尖峰，不能解释异常或取消失败。本轮是冻结 candidate 的独立验收，不把另一次通过当作原结果的修复。

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

| 指标 | 冻结 candidate 相对对照的限制 |
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

首轮 [41166634669](https://github.com/isarmg/zxs/actions/runs/41166634669) 的机械检查、受影响严格 lint、102 项 RPC／50 项协议测试和两臂 release 构建、密封全部通过。四个 pair 都在读取不存在的 `/sys/fs/cgroup/cpu.max` 时、创建任何负载子进程前中止，判定为 incomplete／NOT EVALUATED；没有性能样本。修正后把缺失控制文件明确记录为 `not_present`，不推断无限额度，真实权限／I/O 错误仍失败并保存 pair 错误产物。恢复入口 [reuse-workflow.yml](../../tools/local-rpc-acceptance/reuse-workflow.yml) 固定复用该 run 的产物和 manifest `bc2cc8e53b796d8aa6fb2bd9fb493133b15445e5e764aa5cb166d199b8975700`；核验成功 build job、原控制器／源码身份、前后覆盖层、全部编译 Rust 字节及二进制哈希，只允许本次控制器、回归、文档和恢复模板改动。原 90 分钟截止时间和全部资源／性能门槛不变，不重新编译或重复已完成的测量。新增机械回归、格式、严格 lint、RPC 语义及成对负载的实际执行状态以最终交付的 run、SHA／tree 和产物为准。模板或任务启动不代表完成。原采用 FAIL 与尖峰未解释状态始终保留。

### 2026-10-31 有限执行结果

恢复 run [41168852801](https://github.com/isarmg/zxs/actions/runs/41168852801) 已结束，最终 summary 为 **incomplete / NOT EVALUATED**，不是完整性能通过，也不是已经证明的延迟门槛失败。四对均尝试执行，22 历史 AB、BA 完整结束；220 历史两对分别在首臂对照、首臂 candidate 的 `tails` 阶段中止，第二臂没有启动。没有拼接不同 runner 的数据，没有提高限额、清理恢复证据或重跑已有效的测量。

22 历史两对均满足其全部九项条件，共 18 项；下表均为冻结 candidate 相对对照的 P95 增量，单位 ms。读／目录 P95、P99 和 20 次保存总时长也均在原门槛内。

| 顺序 | 完整保存 | 8MiB Flush | 8MiB+64KiB Close | 8MiB write-through |
| --- | ---: | ---: | ---: | ---: |
| AB | +3.40 | +3.33 | +2.28 | +4.98 |
| BA | +20.36 | +5.35 | +5.61 | +20.44 |

220 历史失败均为 `bounded_write_tails` 中打开新的 `tail-through-*` 文件：客户端收到 `WouldBlock / ResourceLimitExceeded / errno 11`，消息为主体工作区证据元数据字节预算耗尽，全部元数据及恢复证据继续保留。默认主体限额是 64MiB，全局是 128MiB；这是核心应用资源准入，不是 runner 的 8GiB 地址空间／磁盘下限，也不是性能数值越线。两个首臂都已完成 seed、完整旧历史 oracle、1000 次读／目录、20 次原始保存及独立重开 RPC oracle；新增 40 组尾样本未齐全，且没有同 runner 的另一臂，不能判定 220 的读写、启动或 VmHWM 相对门槛。原始错误和部分有效数据保存在 [220 AB](https://github.com/isarmg/zxs/actions/runs/41168852801/artifacts/13030213191)、[220 BA](https://github.com/isarmg/zxs/actions/runs/41168852801/artifacts/13031156286)；[summary](https://github.com/isarmg/zxs/actions/runs/41168852801/artifacts/13030599992) 明确拒绝不完整样本。

本轮实际产品源仍为对照 `e2f33d9882cf7263412d9370b6a703e0b216357d` 与 candidate `d3eefd289e53a2803703870fe4250d08cadff9b5`；控制器恢复源码为 `e6729bd934beae2b22de90b48bed3cb4116fc536`，carrier 为 `52bea22bf564ae8054c067e859507af8ae263d7a`。首轮成功完成 102 项 RPC、50 项协议测试；恢复机械检查包含 15 项控制器回归，独立审阅修正后无剩余阻塞。40 个尾样本的 P99 是最大值；这些结果不是物理盘、网络 SMB 或办公客户端证明。

主线后来采用 `1f1904ad35b0cc3214e42e495907c95ce714e367`，本轮没有实测该 SHA。与冻结 candidate 的生产差异只有历史窗口 Nodes 查询的有界批量首发布查找；`engine.rs` 另加测试模块接线。该查询入口来自 HTTP 历史窗口，不在本轮 Unix RPC 的 Open／Read／ReadDirectory／PrepareWrite／Dirty／Flush／Close、提交、恢复及工作区预算路径；这些路径和配置、Cargo 锁／工具链均未改变。因此本轮可作为相同 RPC／资源准入路径的已有证据，无需仅因 head 不同重跑无关负载，但不能宣称量过新 main 二进制，亦不能据此证明新历史窗口查询性能。主线 [41168353374](https://github.com/isarmg/zxs/actions/runs/41168353374) 的功能 CI 成功不能补足这里的 220 性能缺口。

该历史阶段的下一项软件验收缺口是先设计在既定元数据准入内可完成的 220 成对扩展负载，并保留原始保存与新增尾测试的独立完成状态；当前证据不支持修改产品限额或削弱证据保留合同。本轮不继续尝试刷绿，旧 `2af` 采用 FAIL 与孤立尖峰未解释状态不变。

### 220 合同闭环与容量支持边界

`220` 的准确含义是 500 个文件、97 个目录、一个分支下的初始已发布 sequence／历史规模；它不是测量迭代数。原工程合同的五阶段为 build、edit、完整 verify、1000 次读／目录及 20 次完整保存、独立 rpc-verify；结束 sequence 为240。新增尾合同是在同一个池继续40轮 Flush／续写Close／write-through，共120次发布，要求到360。二者的结论必须分别报告。

默认预算下连续240→360有源码可证的支持边界：每个已含500文件的工作区，生命周期预留将所有文件context的三个ID扩至至少64字节，generation/fence扩至u64最大值；另保留至少500个selected_file_branches映射，每个生成ID至少含32位UUID。仅这两张JSON映射就至少211,002字节，不计外层字段名、其他字段、新增文件和ID前缀。排除仓库创建和首次整批上传工作区，原220+20保存至少保留238个这种工作区，贡献至少50,218,476字节；新增120个贡献至少25,320,240字节。合计至少75,538,716字节，比主体64MiB上限67,108,864多至少8,429,852字节。因此原连续扩展合同结为 **FAIL_DEFAULT_BUDGET**；Cold retirement只减resident计数，不减这些metadata字节，Abort和重开也不能释放它们。没有找到保留所有原证据却缩减该计费的公开API。[确定性下界工具](../../tools/local-rpc-acceptance/budget_contract.py)只证明下界，不重写Engine准入算法，也不把磁盘inventory当逻辑charge。

拆分独立尾池可以形成“每池至少220历史”的另一种测量，但会改变连续历史增长与冷首发布次数，不能代替原连续留存压力合同。因此不以拆池PASS覆盖失败。闭环工作只补尚缺的原始220 AB／BA五阶段：仍使用原sealed seed/server、原500文件／初始220、全部原ID/FULL/历史oracle、相同请求、同runner两臂对称和原八项阈值。22历史的完整两组以原pair.json SHA256固定复用，不再执行。新增[续测模板](../../tools/local-rpc-acceptance/original-220-workflow.yml)与旧冻结CI独立；原运行41168852801及其NOT EVALUATED不改写。新授权续测声明新的45分钟pair窗口，保持原每阶段1200秒、两CPU、8GiB地址空间／磁盘下限，不延长已结束的旧run。

输出会同时给出原始220性能PASS／FAIL及连续尾容量FAIL；即使原始性能通过，完整扩展准入仍为FAIL，summary保持非零。此次控制器还改为逐阶段归档已生成seed／RPC操作ID，避免后续尾阶段失败使已有ID文件只留在被清理的临时池。没有修改任何生产源码、默认限额或既有冻结CI。

### 原始 220 五阶段终态（2026-10-31）

续测 [41174023827](https://github.com/isarmg/zxs/actions/runs/41174023827) 的同 runner AB／BA 两臂均完成全部五阶段。原始八项条件每方向六项通过、两项失败，合计 **12 PASS / 4 FAIL**；原始 220 性能判定 **FAIL**，不是 NOT EVALUATED。失败均为打开时间和打开 VmHWM；保存 P95 没有越过 +50ms 门槛。

| 顺序 | 打开时间：对照→candidate（秒） | VmHWM：对照→candidate（KiB） | 保存 P95 增量（ms） |
| --- | ---: | ---: | ---: |
| AB | 88.476854918→133.567525121 | 449912→543156 | -8.950064 |
| BA | 85.754738331→131.735542338 | 449516→543024 | +8.854133 |

打开时间原限制为对照的 1.1 倍；VmHWM 原限制为 0.8 倍，即要求降低 20%，不是允许增长 20%。实际 VmHWM 约为对照的 1.208 倍，既未达到改进目标，也有实际增长。读／目录 P95、P99 和 20 次保存总时长均通过原限制。两臂独立 oracle 分别验证 220 个原操作／历史命名空间、718 个历史正文及 500 个当前文件；RPC 后独立重开核对 20 个新增收据／历史正文／命名空间，sequence 为 240。seed 与 RPC 都使用原 `bench` 主体，未切换主体、重置预算或释放永久证据。

冻结产品源码与 sealed 二进制仍为前述 e2f33d／d3eefd；该结果不声称测量了后来 main。连续尾合同的 **FAIL_DEFAULT_BUDGET** 是另外一项失败，不因五阶段完整而消失；22 历史通过亦不能覆盖 220 失败。旧 NOT EVALUATED 和 2af 的失败仍保留。

原始 ZIP 已下载并逐字节 SHA256 核对：

| 产物 | artifact ID | SHA256 |
| --- | --- | --- |
| 220 AB | [13033401704](https://github.com/isarmg/zxs/actions/runs/41174023827/artifacts/13033401704) | `3260c7a1d4c55306f7d12f0957158e9e6cf02b2e3262a12630a769d6392503d1` |
| 220 BA | [13033565753](https://github.com/isarmg/zxs/actions/runs/41174023827/artifacts/13033565753) | `14f45e7c44d2c7ef89ab07bcfd2a2b171d74ea2ff7cf5dc8e5cc68d45219a5f9` |
| summary | [13033191203](https://github.com/isarmg/zxs/actions/runs/41174023827/artifacts/13033191203) | `ea60d366863367e3895a877a399496e310f9c3054fe7d7cc335ea5c9547c8b1a` |

### 新源码的独立 220 复测入口

[新密封模板](../../tools/local-rpc-acceptance/candidate-220-workflow.yml) 与
[candidate_220.py](../../tools/local-rpc-acceptance/candidate_220.py) 专供已独立审阅的新产品源码，
不把旧 d3 二进制或旧 manifest 当成修复后的测量。必填输入是正常 CI controller 的完整
`validation_sha`、独立冻结产品 `product_sha` 与 `product_tree`；carrier 不进入产品祖先。
对照仍固定 e2f33d 及其原 tree。新产品和对照分别核对源码、仅测试接线、前后 tree、编译器、
特性与二进制摘要后形成新 seal；已有 tracked bench 只在与 controller 字节完全一致时复用，
不允许静默覆盖。全部原机械／实际消费者前置仍执行。

流程为 validation→新两臂 build/seal→selection 冻结 45 分钟计划→原 220 AB/BA→summary。
每阶段 1200 秒、每 pair 45 分钟、两 CPU、8GiB 地址空间／可用空间下限与原八项门槛不变。
SHA、tree、manifest／binary／plan hash 不符、顺序或 oracle／样本不完整均拒绝。
只复测五阶段；旧 22 仅作为历史 d3 证据引用，不计作新产品通过。初版 controller 对当时
64 字节预留版本固定保留连续容量 FAIL 并令 summary 非零，这一旧 run 结论不改写。
当前 controller 不把旧预算失败套到新产品：连续项与总合同均标记
`NOT_EVALUATED_BY_ORIGINAL_220`，`old_continuous` 单独保留原失败与旧预算证明，
`actual_continuous_run` 为 null，必须另取独立连续 run 的实际结果。summary 退出 0 仅表示
两对原八项门槛全部通过；任何原八项失败仍非零，退出成功不代表连续容量或完整合同通过。
模板提交不代表已启动或通过；实际 run、产品 SHA/tree 和新密封摘要必须另行记录。

### 新密封 2198fc 的原始 220 实测结果（2026-10-31）

[41178276427](https://github.com/isarmg/zxs/actions/runs/41178276427) 使用新产品
`2198fc1866ba68b1d7d517ca57a2360f9c66a0a6`、tree
`1b5ba2aa0bf77b080a172207c1fc014cf8aebed5` 与原 e2f 对照，重新构建四个二进制；
seal manifest SHA256 为 `39239baeb64700ef52af364ddedf16b218e1feef8e6add55cf8de8aa95871b22`。
AB／BA 四臂均完成五阶段，但原十六项条件仍为 **12 PASS / 4 FAIL**：两方向的
启动时间和打开 VmHWM 未达到原 1.1 倍／0.8 倍限制，原始 220 判定 **FAIL**。

| 顺序 | 启动时间：e2f→2198fc（秒） | VmHWM：e2f→2198fc（KiB） | 保存 P95 增量（ms） |
| --- | ---: | ---: | ---: |
| AB | 85.910712100→133.152066959 | 449720→441496 | +3.926584 |
| BA | 88.243269944→144.725878655 | 449648→441200 | +2.346905 |

新候选 VmHWM 相对本次原对照降低约 1.8%，不是要求的 20%；相较旧 d3 543156／543024 KiB明显减少，但不同 run 的观测不能代替原门槛。
读／目录 P95、P99、20 次保存总时长与保存 P95 均通过原限制；启动缺口仍在。
每臂原始 JSON 的独立 oracle 均核验 220 个原操作／历史命名空间、718 个历史正文、
500 个当前文件；RPC 后正常独立重开核验 20 个新增收据／历史正文／命名空间和 sequence 240。

| 原始 ZIP | artifact ID | SHA256 |
| --- | --- | --- |
| AB | [13035007774](https://github.com/isarmg/zxs/actions/runs/41178276427/artifacts/13035007774) | `364e864c9909a08f28f882dda0332e8e1fa2ff16eecc86b39e43fb090d0056bf` |
| BA | [13035252490](https://github.com/isarmg/zxs/actions/runs/41178276427/artifacts/13035252490) | `1a9ffaf8bfaf39ba2167077443be42348387e7f626f07aa37914b8336a9de044` |
| summary | [13034843323](https://github.com/isarmg/zxs/actions/runs/41178276427/artifacts/13034843323) | `d60ea4ac5e983689b7d27cf58fa6411ee958a28a45a1117b63199741c32f9c82` |

三个 ZIP 下载后均逐字节核对 SHA256，另核对四臂原始 JSON，不只依赖汇总。
本次产品不含后续 ID-width 修复；summary 的 `FAIL_DEFAULT_BUDGET` 只沿用旧预算边界，
不能评价后续 52b590 新产品。后续共同修复工程对照／连续容量验证是另一合同，不得替换
本次原 e2f 门槛或抹去旧 2af、d3 的失败；本结果也不声称测量后来主线二进制。

### 后续候选：严格 ID 宽度的完整生命周期预留

旧 d3 的 64 字节预留、75,538,716 字节下界及其实际失败均保持原样。
后续源码候选只将 FileContext 新生成 ID 的预留宽度收紧到既有闭合类型：node 是
`ver`／`fnode`／`fmeta` 中最长 38 字节，branch 是 `br` 的 35 字节，inherited origin 是
`dbr` 的 36 字节。`validate_id` 要求前缀、下划线、恰好 32 个小写十六进制字符；
HistoryManifest 对对应表键逐一验证，context 必须引用这些已验证记录，State admission
和外部历史包导入均经过同一验证。未来提交仍只能生成这些 ID，不引入其他格式兼容。

每个字段仍取既有值长度与类型上界的最大值；generation／fence 仍预留 u64 最大值，
未释放 Cold、提交或 Abort 的生命周期预留，未改 64MiB／128MiB 限额。测试保留原
“提交前后 charge 相等、重开后不变”的断言；满额冷记录仍阻止新记录并允许重复 Abort，
跨 principal 转移、损坏镜像和未决写入仍拒绝。全部实际历史、FULL、原操作 ID 和元数据
继续保留并计费。

两次真实提交的 500 文件小诊断中，第二个终态 workspace 实际 canonical 为 108,792
字节，旧生命周期预留 215,811 字节；仅保留终态 fence 的假设上界为 108,811 字节。
该“终态释放”方案未采用。采用的是上述完整生命周期精确宽度；原两映射数学下界由
75,538,716 降为 60,681,716 字节，仅说明旧“不可能容纳”的证明不再适用于新版本。
它不包含全部其他字段／尾文件／首批上传，**不是原连续 240→360 已通过**；必须使用新
源码／新 seal 独立实测后登记，不能用旧 controller 的固定容量 FAIL 作为新版本结论。

## 只需人工／物理环境执行的项目

- 在指定 32GB／8 核／512GB 系统盘和 12×15.6TB 数据盘上记录型号、固件、文件系统、占用量及完整副本拓扑，执行同一有限合同，验证稳态／拥塞时读写尾延迟与资源预算；GitHub 开发盘数据不能外推。
- 在实际 Windows／macOS 与拟采用 Office、照片／视频／文档应用的准确版本，经过真实 Samba 网络路径执行固定工作日场景；逐项核对打开、保存、Flush、Close、write-through、文件替换、重连和历史正文，不以 SDK 编译或 Unix RPC 代替办公客户端兼容。
- 在具备独立恢复条件的物理测试池进行保存中的真实掉电、控制器／盘掉线、网络中断与服务重启；按原 ID 查证未知回复，检查全部 FULL 和恢复证据。模拟断线与正常进程重开不证明电源故障后的硬件持久性。

## 新 ID-width 版本的连续容量与隔离工程对照

`continuous_220.py` 与 `candidate-continuous-220-workflow.yml` 是独立的新版本验证入口，
不替代原 e2f 对照的 original-220 八项验收，不改写原 220、连续容量及 2af 的失败记录。
工程控制臂仅在 e2f 上施加同一字段 ID-width 预算修复：精确 tree
`378f3b063c8aa2099da3d582eaa511937da138e9`，唯一父必须是原 e2f，唯一变更路径为
`src/engine/src/engine/workspace_budget.rs`。控制臂 SHA、候选 SHA/tree、controller 与四个
二进制哈希均重新绑定；不能复用旧 seed seal。产品提交必须包含与控制器逐字节相同的新 seed。

连续合同仍是 500 个初始文件、一个文件分支、同一 principal `bench`：初始 220 条操作
（创建仓库 1、初始上传 1、历史编辑 218），普通 RPC 保存 20，再执行 40 轮 8 MiB
Flush／追加 64 KiB／Close（80 次提交）及 40 次 8 MiB write-through，最终 360 条操作／事件、
359 条工作区记录，而非 360 个并发工作区。正文与全部历史不删除，principal 不更换，预算不重置。
principal 元数据额度仍为 64 MiB、全局 128 MiB，Cold 与完整生命周期预算语义不变。

每臂仍执行七阶段，2 CPU、8 GiB 地址空间及最小可用空间、单阶段 1200 秒、AB/BA 每对
45 分钟窗口均不变。最后独立 reopen oracle 不计入 RPC 性能计时，重新核验全部初始 220
原操作 ID／namespace／718 个历史正文、20 个 RPC 保存、120 个尾提交及 owner、发布状态、
FULL、事件序号和仍保留的 359 个工作区额度。新 seed 在实际运行前须重新构建、封存。

结果分开报告：候选完整连续容量／正确性（要求两个样本）、共同修复后的工程对照十一项门槛、
其中三项尾延迟门槛，以及单独原 e2f 八项验收。若候选已完成而控制臂失败，保留该候选的完整
oracle 样本，并明确 `1/2`，不把未完成成对比较或两个样本要求标为通过。全部阈值不变。
模板、控制器及编译通过只证明验证接线，不能提前宣称新连续容量或延迟通过。

### 缺失候选容量样本的独立恢复

`capacity_recovery.py`／`candidate-capacity-recovery-workflow.yml` 只恢复第二个候选正确性样本：
固定复用 run `41180163138` 的 artifact `13034728252`、原 manifest、原 carrier 及
`52b5901ac716f67432fe01e9515c1d7158a480e9`／tree
`ee672528c69b4b00428f04d015451037aed8511b`，不重编译、不重复控制臂。
原 BA 候选七阶段／360 操作／359 工作区 oracle 按原 pair.json 的精确 SHA256 重新核验；
原 AB 候选超时与原 ABBA 未完整完成的结论保持不变。

新 observer/controller 的 SHA、tree、run 与旧产品／二进制 seal 分开记录。新单候选仍完整执行
七阶段、500 初始文件、同一 `bench`、相同 120 个尾提交及全部历史 oracle；只在第二样本也完整
通过后合计容量正确性 `2/2`。单 job 45 分钟（含准备），新计划最多 45 分钟、单阶段 1200 秒、
2 CPU、8 GiB 地址空间／可用空间、64 MiB／128 MiB 元数据预算不变。该恢复不是原 ABBA
成对比较通过，也不构成新增性能门槛；任何 seal、旧样本、身份、预算或完整性不符均拒绝。
