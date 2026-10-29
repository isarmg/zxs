# 实际 Backend 竞争测量（仅验证载体）

本夹具不是生产功能，也不是已经执行的结果。源基线：e2f33d9882cf7263412d9370b6a703e0b216357d；候选：13b664e9fd41a356ffb7ed982d21e97abb123626。

## 测量对象与边界

- 500／1500／3000 个同目录文件，单成员临时池。初始化和64 MiB工作区暂存在测量区间外。
- 真正的 EngineBackend::commit_workspace 与真正的 read_directory、file_history、operation 并发执行，每类32次。保留全部调用开始／结束时间，统计与写操作实际重叠的样本，不把未重叠样本称为竞争延迟。
- 独立观察者每次等待5 ms后对 **同一个 EngineBackend.engine Mutex** 进行一次取得后立即释放，记录尝试至取得的时间。此数字包含调度延迟，观察者也会改变竞争；它不是原业务持锁时长，也不等于纯futex睡眠时间。最多12,000次尝试，耗尽标记未完成；锁取得本身可能阻塞，因此这不是60秒墙钟上限。
- 固定短名称的首页必须恰好256项、逐项路径/文件类型正确且more=true；历史归属、原操作结果、同ID重放、完整正文、全量命名空间路径与重开后结果均核验。后来健康观察时间允许变化，不与永久结果混淆。
- 网络路由、HTTP调度、SMB RPC、真实独立设备不在此夹具覆盖内。名称为Backend，而不是整条网络请求。

### 锁占用的保守上界，不冒充精确持锁计时

当前同步Backend函数的所有engine guard都在调用返回前释放。因此，一次实际调用的开始至结束区间是该调用累计持锁时间的保守上界（包含等待、锁外I/O、调度和断言时间）。观察者在acquired_ns时刻实际持有同一把锁，写调用不可能在该时刻持有它。用写调用开始、写区间内全部观察者取得时刻、写调用结束分割区间，其最大间隙是该次写调用任意**连续**持锁区间的保守上界；它不是累计持锁时间，更不是精确guard计时。

runner分别保存writer_total_hold_upper_bound_ns、writer_continuous_hold_gap_upper_bound_ns及各读调用上界，并要求至少一次观察者取得发生在写调用内部、每类读至少一次与写调用重叠。只有写后取得样本或没有真实重叠时标incomplete。上界可能很松，不能由短样本推导所有请求的SLA或把等待时间归因于某个owner。是否足以满足既有采用预算须结合实际三档配对结果评审；不新增“必须精确到每次guard”的ADR条件，也不修改ADR降低门槛。

这些是合法私有测试对实际锁的观测。没有复制Backend业务函数、替换另一把锁、插入临时生产计数器或人为延长持锁区间。

### 实际编译配置

这是release **单元测试目标**，不是普通守护进程二进制。既有cfg(test)的空hook Mutex检查仍在；基线engine dev特性为fault-injection，候选为fault-injection、test-support，本夹具不设置任何故障或hook。workflow保留实际Cargo compiler-artifact特性和rustc命令并断言这个图，不删dev-dependencies以伪装生产构建。两个产品的真实Backend路径和准入锁仍被执行，但本配置的观测上界不能直接冒充无测试特性的生产时延。

## GitHub执行

只在验证载体给两个独立源码目录应用相同夹具文件与末尾cfg(test)/path接线，不修改其生产函数，不把验证载体祖先合入产品。基线和候选Rust均固定1.99.0。每个目录执行：

    cargo test --locked --offline --release -p storage-server --lib --no-run --message-format=json -vv > server-test-build.jsonl 2> server-test-build.log

从Cargo的compiler-artifact记录选择package storage-server、target.kind含lib、profile.test为true且executable非null的唯一产物。不要通过target目录glob猜测二进制。两个构建使用不同CARGO_TARGET_DIR，禁用增量，限制build jobs，保留rustc -Vv、精确源码SHA/tree、实际编译命令、源码应用前后diff与二进制摘要。

    python test/server/bench/contention/run.py --baseline BASELINE_BIN --candidate CANDIDATE_BIN --baseline-sha e2f33d9882cf7263412d9370b6a703e0b216357d --candidate-sha 13b664e9fd41a356ffb7ed982d21e97abb123626 --out EVIDENCE_DIR --pool-parent OWNED_TEMP_PARENT --deadline-unix ABSOLUTE_DEADLINE --sizes 500

同目录verification.yml.template是完整的独立载体workflow；载体分支verify/backend-contention-20261029-r82，三个规模各一个job，每档三对独立进程，顺序AB/BA/AB，共18个执行。产品分别checkout精确SHA/tree，只追加相同私有测试模块接线与夹具；编译后重新核对允许变化的精确字节。载体workflow不得进入产品祖先。

每次900秒进程上限、8 GiB虚拟地址空间上限；500/1500/3000档启动分别要求11/15/24 GiB临时盘空闲，运行每0.5秒检查8 GiB剩余量。编译步骤最多60分钟，驱动安装时设置包含编译的150分钟总截止，job最多180分钟，为清理和上传留出余量。stdout/stderr直接写文件，进程状态与summary递增落盘；截止/磁盘不足/覆盖不足为incomplete，断言或进程失败为failed，均非通过。只终止自己创建的进程组，退出后再清理自己创建的临时父目录；不读取或清理真实用户池。

磁盘预检只是容量规划，不是声称已证明峰值。依据[既有三档20次编辑记录](../../../../docs/validation/paged-namespace-task1-benchmarks.md)中两实现最大保留池333.4/2893.7/11467.0 MiB，沿用命名空间配对载体2/6/16 GiB池余量；小两档再加1 GiB规划余量覆盖64 MiB暂存/发布正文及重放相关临时材料，得到3/7 GiB，分别加独立8 GiB剩余量。最大档仍保留16 GiB池余量（相对旧约11.2 GiB保留池超过4 GiB余量），不降低原24 GiB预检。这里不假定正文只有固定份数，也不把旧Rust1.97供方数字说成已复现的Rust1.99结果；新负载是同目录文件、额外认证发布和一次大正文写入，实际占用可能不同。超过规划时仍由持续8 GiB检查停止并保留incomplete及实际字节，不以规划值强制通过。

标准ubuntu-24.04的实际可用空间可能达不到该档11/15/24 GiB需求；保留需求与观测字节后明确incomplete，不降低持续保护余量或默默跳过大档。实际runner故障/外部取消仍可能阻止最终上传，不承诺始终有完整日志。只有全部被请求案例成功才输出completed_measurements；它不是采用批准。

各次进程的临时池由TempDir生命周期释放；崩溃留下的runner临时目录不作为用户数据。不要修改/删除任何真实池。p50/p95/p99可从原始elapsed数组计算，但每类32个样本的尾部粒度很粗，必须保留原始分布和三次波动，不能以单次百分位宣称稳定提升。
