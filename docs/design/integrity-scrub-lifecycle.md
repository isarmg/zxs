# 完整巡检生命周期：离线与锁外后台

状态：离线入口和在线后台已有源码；隔离早测结果见[冻结验证记录](../architecture/online-integrity-scrub-verification.md)，组合矩阵仍须独立验证。它们不等于普通发布旧依赖免读门禁、吞吐实测或物理掉电验证通过。目标责任见 [ADR 0005](../adr/0005-incremental-publication-integrity.md)。现有 `/api/admin/check` 的目录诊断范围、计数和授权语义保持不变。

## 两个真实入口

`zxs-rescue --root SYSTEM_ROOT scrub` 调用 `Engine::scrub`，适用于显式离线维护。一个命令只有一个专属 native worker；Engine 及 writer lock 保持到真实 I/O 结束。SIGINT / SIGTERM、调用 future 销毁均请求取消并 join，不分离 worker。信号安装、线程创建、panic 和核心错误具有失败出口；完整无故障才返回 0。打开选择 PreservePending，但仍执行现有恢复维护；扫描自身不修复。详见[救援工具](../components/rescue.md#integrity-scrub)。

在线 `ManagementBackend` 拥有具体 `IntegrityScrubService`，不是离线整程 Engine 锁的定时包装。daemon 启动 scheduler 后，在存储真正准入时自动开始首轮；管理员也可通过独立 HTTP start/status/cancel 操作。嵌入方的 `ManagementOptions::default()` 只有手动调度，需显式提供 periodic policy 才自动扫描；裸 `EngineBackend` 没有 scheduler owner，明确返回 Unsupported。无 core 构建不编译这些路由。

## 锁内准入、锁外真实读取

| 接口 | 锁内职责 | 锁外职责 |
| --- | --- | --- |
| `Engine::prepare_integrity_scrub` | 拒绝已有 detached preparation；捕获 `Arc<State>`、配置、snapshot/sequence、各成员准确 admitted journal tail 和原 Engine 身份 | 不遍历所有版本，不读正文，不签发发布许可 |
| `IntegrityScrubPlan::estimated_payload_bytes(&cancellation)` | 无 | 遍历固定基线，逐版本响应取消，计算 required payload 的期望字节数；取消不返回部分总数，这不是总 I/O 估计 |
| `IntegrityScrubPlan::run` | 通过调用方回调逐版本准入 | 验证固定基线的完整恢复元数据闭包、journal prefix、必需正文副本；真实读取计费，普通回调每次最多 64 KiB；准入路径 marker 的累计实际字节在解锁后补计 |
| `Engine::admit_integrity_scrub_batch` | 比较当前 owner/config、版本与放置/内容状态；取得一个版本 ReadLease，有限读取路径身份 marker 并打开正文 fd | checksum、proof、正文 hash 均不在 Engine 锁内；一个版本最多 64 个正文 fd，超出明确 ResourceLimit |
| `Engine::finish_integrity_scrub` | 检验结果仍属于原 Engine/config | 输出 Complete、Cancelled、Superseded 或 Failed，不把过期基线当健康 |

实际 job 强持 `Arc<EngineBackend>`，因此 Engine 及它拥有的真实 writer lock 始终保留；不额外复制每个 lock fd。公共 core plan 不单独提供写者锁生命周期保证，最终接纳仍须回到原 Engine；生产 adapter 的 owner 链覆盖全部 I/O、lease/fd 关闭与最终结果交接。

估计在遍历前、每个版本及结束时检查同一单向取消 token。取消发生在估计阶段时，
adapter 不发布未完成的 expected payload bytes，继续由原 plan/token 生成零存储
读取的 `Cancelled` 报告，再交回原 live Engine 完成接纳；不会改成准入拒绝。
身份重新核验和 plan 准入仍先于估计；正常完成才发布完整估计及 Running 基线。

journal 扫描在捕获的准确尾 hash 停止，并实际探测之后的 1 byte；finish 核对当前 writer 尾与不确定状态，区分合法追加和静态未解释尾字节，不用 stat 代替读取。运行期元数据对象没有 GC 删除入口；将来新增元数据 GC 必须接入基线保留约束。正文 batch 以现有 ReadLease 保护准入后的 purge/cleanup；准入前 descriptor/路由/内容变化返回 Superseded，而非完整成功。普通不相关新发布可以和旧基线扫描交错，不会刷新这次基线。

`Arc<State>` 捕获为 O(1)，保留旧 State 仍可能增加写时复制成本；这里不宣称端到端增量或无额外驻留。磁盘内容仍逐项实际验证，路径、Arc、mtime 和锁都不是磁盘完整性的替代证明。

## 具体 owner 与停止排空

服务拥有一个 native JoinHandle、一个短控制锁、条件变量和当前 token。worker 仅持共享控制块及 `Weak<Inner>`；入槽后持本次 backend，空闲时不强持 EngineBackend，认证只保留弱 AuthReader，不增加 AuthStore 强计数。服务字段先于 Inner 销毁，Drop 可取消并 join，避免最后一个 Inner 引用在 worker 内触发 self-join。

- 同时最多一个 queued/running job；重复手动启动返回 409，周期不积压任务。入槽和 stop 使用同一个短锁。
- 手动 identity 在真正取得 Engine 锁、捕获基线前重新核验。排队期间注销/改密拒绝扫描。自动任务使用私有主机策略能力，不伪造管理员会话。
- status/cancel 核验当前 AuthReader，只取短控制/存储状态锁，不等 Engine，也不占长期业务阻塞槽。cancel 仅请求取消；它不是“工作已结束”的确认。
- stop 关闭新准入、取消并唤醒当前等待；drain 在业务锁外 native join。并发 drain 通过 worker mutex 串行等待同一次真实 join，不能第二次提前返回。
- `ManagementShutdown` 同时排空 scrub 和 recovery，任一失败不跳过另一 owner。native spawn/panic 有具名错误和不可用状态；不静默重启、不保留虚假 Running。
- 活动 backend 强引用使恢复 strong-count 检查等待实际工作结束；空闲 scheduler 和历史报告不阻碍恢复。新 backend 安装后旧成功只保留历史时间，不重新变绿。

## 周期、预算与可见状态

默认 daemon policy 为 **64 MiB/s、24 小时周期、从扫描开始起 48 小时新鲜度**。三个 CLI 数值均为正整数，在主机输入边界构造一次有效 policy，最大年龄不得小于调度间隔。它们实际参与调度、每次 read 的等待和 status 判定，不是占位配置。

周期调度使用单调时钟锚点。存储真正准入时立即开始首轮；存储未准入时按 `min(interval, 5 秒)` 重试就绪，真正就绪准入时重新锚定，不创建空任务。健康扫描完成后保留锚点，跳过忙碌期间已错过或恰在完成时到达的周期点，选择严格晚于完成的下一周期点。以 24 小时周期、零时刻锚点为例：6 小时完成时下一轮在第 24 小时；第 24 小时恰好完成或第 30 小时完成时，下一轮均在第 48 小时。不在完成后固定再等 24 小时，也不追赶已跳过的任务。

手动任务实际开始执行时重新锚定周期，并与自动任务共用唯一 queued/running 槽；真实扫描可承担同一连续 owner 的巡检义务。不健康报告、取消、Superseded 或准入拒绝后，从完成起等待一个完整 interval，避免失败忙循环。stop 继续关闭准入、撤销 owner 并唤醒等待；重开仍需新 lease 与真实扫描。

计费覆盖核心实际 metadata、journal、proof/checksum 和 payload 读取；sys/procfs 身份探测、目录枚举和打开操作不计字节。每次实际 read 后按字节成本等待，普通读取最多一个 64 KiB 先读突发。逐版本准入的路径 marker 读取累计跨过 64 KiB 或达到 4,096 个检查点后不再继续；包含最后一读的实际字节少于 128 KiB，解锁后补计，已观察故障先于节流通知。等待期间不持 Engine mutex，cancel/stop 可唤醒。一个扫描、一个版本 lease、有限 fd 和有界 issue 列表共同限制后台资源。这不是设备级 IOPS 隔离、操作系统优先级或前台延迟 SLA；仍需真实压力测量。

HTTP 状态独立保存运行 phase、最近终态、最近完整成功时间及 freshness。Running 显示基线 sequence/hash、开始时间、实际读取字节和 expected payload bytes。实际字节包含元数据及额外验证，因此不能用后两者直接计算总进度百分比。错误列表沿 core 有界截断，不复制所有历史状态。

终态将同一活动任务的读取计数、已有 Running 单调时钟的耗时、完整正文估计与
当轮策略保存为有界值快照，未进入相应阶段的估计/耗时保留未知。原 live Engine
finish 后另取一次短锁，只调用现有资格检查采样共享元数据覆盖；此锁可能等待
并发 Engine 操作，status/cancel 仍只取控制状态锁。该观察不参与任何资格判定。
完整健康报告、HTTP freshness 和收尾时覆盖观察分别保留；局部 proof/body 故障
仍可与有效共享图覆盖并存。终态字段及下界可行性状态见[服务端合同](../components/server.md#integrity-scrub)。

仅 Complete 且 `report.is_healthy()` 更新完整成功时间。写成功、Ready、取消、部分失败、Superseded、线程失败都不能刷新。新鲜度使用单调时间，从**扫描开始**计算；一轮耗时超过 48 小时即使最终完整，也显示 expired。UTC 仅用于展示；重启不恢复旧绿色状态。已观察到故障立即令 freshness 不可用，不等全池扫描结束。

24 小时是无重叠调度周期，不是完成或故障发现 SLA。核心覆盖仍不晚于 `min(扫描开始 + max_age, 接纳完成 + interval)` 到期，周期锚定没有重置期限。它减少完成后固定等待整个间隔造成的无谓空窗，但相邻扫描耗时差异、过期、故障和准入等待仍可产生空窗；频繁维护使基线过期时应展示未完成事实，不能无限重试掩饰。原始盘标称容量不能代替实际 required 正文、历史和元数据读取规模，无法在预算内完成的大扫描仍无有效覆盖。状态不是写入许可证，也没有改变普通发布的现有验证。

## 验证边界

源内 fixture 覆盖真实周期、单槽、排队撤权、锁外限速与前台访问、取消、停止和双 drain、panic、空闲强计数/重开、新鲜度与写成功分离，以及 HTTP 认证、状态、冲突和原 `/admin/check`。core fixture 另覆盖固定 journal frontier、准入前过期和 ReadLease 交错。编译通过、测试通过、规模测量及物理设备故障分别报告，不互相替代。

专属 fixture 还以真实微型扫描验证完整健康但开始时间过久的结果不能获得
coverage/capture；复用原到期边界测试。终态测试核对真实读取总账、与 UTC 独立的
单调耗时、部分读取取消、拒绝/panic 的未知字段、局部 proof 故障与覆盖的区分、
下界恰好达到期限及宽整数编码。这些是源码覆盖，实际执行结果由唯一验证记录给出。
