# 永久收据与事件增长基准

本工具比较固定对照 `e2f33d9882cf7263412d9370b6a703e0b216357d` 与固定候选
`13b664e9fd41a356ffb7ed982d21e97abb123626` 的真实元数据发布、查询与恢复成本。
候选对应验证分支 `verify/task-a-completion-20261029-r82`，不是已采用的产品基线。
工具不修改生产算法、磁盘格式、同步、永久保留规则或公开 API。

## 工作负载与正确性

单个逻辑数据盘、新建系统根、一个仓库及其初始空分支。初始化后，重复调用真实
`Engine::set_smb_binding`，交替切换同一绑定的 `enabled`。每次发布推进原有代际，
生成一个永久操作收据和一个事件，活动文件、目录和分支数量保持不变。
本工作负载不包含内容上传和真实 SMB 客户端，不代表完整文件保存性能。

`--operations N` 表示**最终累计收据／事件数量**，包括仓库创建记录。
256 与 1024 的样本按这个总数定位；不能把第 256 次负载调用当作第 256 条总收据。
两版各自创建临时池，不共享或相互打开磁盘格式；请求语义相同，随机实体 ID 可以不同。

驱动使用磁盘上的流式核验材料，不在额外的常驻数组中保存全部历史 oracle。
追加计时只包围引擎操作；收据核验、输出和基准自己的记录另行执行。
核验包括原 operation_id 与原结果／请求绑定、代际、事件身份和顺序、固定活动集、
原请求重试不产生第二次效果，以及关闭后由新进程重开和从原盘恢复到新系统根后的结果。
恢复保留原系统根和原数据盘，不清空或替换原材料。

规范依据：

- [事务与恢复](../../docs/10-transactions-and-recovery.md)：真实发布、固定结果及原 ID 重试。
- [ADR 0011](../../docs/adr/0011-paged-namespace-roots-and-segmented-logs.md)：回执封段 256 条或达到 1 MiB，事件封段 1024 条；恢复仍全量读验。
- [盘格式](../../docs/08-full-storage-and-disk-format.md)：对象地址、证明与格式拒绝。
- [核心组件](../../docs/components/engine.md)：Engine 业务入口与故障注入边界。

## 构建与运行边界

正式构建、工具自测和基准只在 GitHub Actions 执行，固定 Rust **1.99.0**。
本地可以阅读、编辑、检查文件格式和使用 Git；不要在本地运行驱动或降级工具链。
`prepare.py` 和编排器具有 GitHub Actions 执行边界；该环境变量是防误用提示，
不是授权认证机制。

`prepare.py --source <checkout>` 只接受两个固定 SHA 的干净 checkout，复制同一份
`driver.rs`，在已有 `perf-check` manifest 中临时注册普通 binary。它不添加工作区、
依赖或锁文件，不在引擎里增加测试计数器。临时接线留在验证 checkout，不作为产品补丁。

普通 binary 避开 `storage-engine` 自身开发依赖开启的 `fault-injection`。
池使用 `Config::development` 的单逻辑盘准入；release 编译不代表真实生产设备配置。
不要改用 example、libtest、`--all-targets` 或 `--all-features` 来测正常构建成本。
旧版只有 `events_after`，候选具有 `events_page`，因此旧版仅启用无依赖的
`perf-check/history-scale-baseline` 适配特性；两边都使用原引擎的真实读取实现。
Actions 保存实际 normal/build feature 图、编译命令、工具链、release profile、
ISA／CPU／内核和文件系统信息；图中出现故障注入或测试支持特性时应停止验证。

构建命令（在各自 checkout 根执行）：

```sh
python3 <tool-checkout>/tools/history-scale-bench/prepare.py --source "$PWD"
cargo tree --locked --offline -p perf-check --edges normal,build,features
# 仅旧版增加 --features history-scale-baseline
cargo build --locked --offline --release -p perf-check --bin history-scale-driver
```

工具自测入口：

```sh
python3 -m unittest discover -s test/history-scale-bench -p 'test_*.py' -v
```

`workflow.yml` 是独立验证工作流模板。把它复制到专用验证载体分支的
`.github/workflows/history-scale.yml` 后推送 `verify/history-scale-*` 分支触发。
工具补丁与工作流载体分开交付；不要把验证载体合入产品祖先，也不要替换或削弱正常 CI。
工作流分别准备两个精确源码 checkout，编译后在同一 runner 上交替版本次序运行，
按边界与 1 千、1 万、10 万累计记录分档。每次重复、每种计数模式均使用独立池。
完整运行参数与预算以该次 artifact 中的 manifest 和命令记录为准。

## 指标与解释

无 strace 的计时与 strace 计数采用独立运行，不能把追踪耗时作为正常延迟。
strace 覆盖每个 build／reopen／recovery 的完整阶段，包括核验及幂等重试；
阶段总同步次数不能直接解释为一次追加的同步次数。
追加逐条记录延迟，早／中／晚 ID 查询及事件翻页按阶段保留原始样本。
新进程的重开／恢复时间与其后的驻留内存分别记录；不要把前一进程的高水位当作恢复后的 RSS。
样本不足以形成分布的阶段保留具体样本和重复范围，不伪造稳定百分位。

| 指标 | 范围与限制 |
| --- | --- |
| 驱动单调时钟 | 引擎调用的端到端时间，受 runner、页缓存和监测开销影响 |
| `/proc/<pid>/io` | 进程级逻辑字符与块 I/O；包含驱动自身 I/O，`rchar` 不是物理读取 |
| strace 返回字节 | 实际 syscall 边界的逻辑字节；按路径区分池数据与观测输出，追踪开销独立 |
| `/proc/<pid>/status` | RSS 与高水位；不等于分配次数，也不等于仅引擎占用 |
| 设备统计 | runner 的共享／虚拟块设备观察；不能归因为独立物理盘性能 |
| 文件／目录同步 | syscall 成功与失败、文件和目录分类；无法解析的项保持 unknown |
| 对象与保留空间 | 池内各类文件数、逻辑字节与已分配空间，包含保留旧证据，不删除历史提速 |

默认测试不清理系统页缓存，也不声称 cold disk；重开是新进程，操作系统缓存仍可能热。
查询顺序和各次重复保存在原始结果中，应先看这些条件再比较延迟。
池目录的空间扫描属于观测开销，结果必须连同实际监测配置解释。

## 预算、失败和交付

每档、每阶段和总 cohort 都有明确时间、RSS 与磁盘预算。达到预算就保留原始日志及
已完成前缀，并标记 `incomplete`；错误接受、核验不符、崩溃或工具错误标记失败。
后续阶段因前置失败没有执行必须明确记录，不能算作通过。
Actions 的最终状态与 JSON 中各档的通过／失败／未完成／未执行应一起读取。

artifact 应包含精确源码 SHA、工具提交、完整命令与环境、build/feature 日志、
各阶段 JSONL、strace 原文与汇总、资源样本、池清点和机器可读汇总。
失败日志与未完成的大档位同样属于交付物。保留期限由 workflow 明确指定。

瓶颈排序只能基于实际完成档位与观测证据。局部编码或对象数量、一次内存改善、
空命名空间下的绑定操作，均不能推出整个保存路径为 O(log N) 或 O(delta)。
本工具不证明无限历史、真实物理掉电、多物理盘独立性或全部产品采用门槛。
