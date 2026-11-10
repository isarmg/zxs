# 终态链准确 generation 的冷选择与独立读回

[原生 generation 日志](terminal-chain-generation-log.md) · [实际 epoch 决定](terminal-chain-io-decision.md) · [辅助工具](../components/helper.md) · [任务清单](../../TASKS.md)

本片基于 `c1f254fb`，续接已发布的 `34e0bffb` 原生日志安装；旧未提交 selector 候选另存保全后在新工作树继续。只有原请求的完整原生日志、普通不可变对象、真实 epoch choice 和安装后冻结 Current 全部成立，才持久选择这一准确 generation。**这是冷恢复选择，不是活动 Engine 的选择器，不更改 `metadata/pool-membership/generation.json`、locator/head、seal、数据库或 Stop，不启用 writer、不完成 readmission 或 H03。**

按要求未审查、编译、测试、静态检查、CI 或浏览器验收，未运行辅助工具、ZXS 或操作真实系统/磁盘。六项 native 和一项 HTTP 输入回归及路由边界扩展仅为未运行测试源码；Rust 格式化是源码编辑，不是验证结果。

## 唯一原请求及实际字节

`TerminalManagement::select_chain_generation` 先用原 actor、operation、proposal digest 和目标盘目录打开已有 `TerminalChainPublishedCurrent`，保留 SYSTEM 管理 owner 和原目标锁。新 descriptor 绑定完整原生 manifest、原日志 manifest 摘要、实际 epoch decision、完整已选 epoch/Stop 后继计划以及原 Publication/seal。目标路径由已验证原 core 唯一推导，不接受客户端提供 authority、generation、epoch 或路径。

同一 descriptor 以无覆盖形式写在三处：

1. SYSTEM 原操作内 `chain-generation-selector-request.json`，先保留原选择；
2. 原目标准确 generation 内 `chain-authority.json`，不是通用 `authority.json`，不让普通恢复误选为活动代际；
3. SYSTEM 原操作内 `chain-generation-selector.json`，仅在目标实物完整读回后保存完成证据。

每个边界重新核对原来源、目标实际 native choice、generation 目录身份、日志三帧及普通不可变对象。descriptor 字节必须精确一致，不替换已有异内容，不建立另一份权威配方。读取时采用 strict 当前格式，既无旧格式双读，也不修补旧材料。

前一阶段的日志安装器也只有在 SYSTEM 原请求、目标 descriptor 和原 native manifest 精确相符时才承认新增的 `chain-authority.json` 名称；无主的同名文件不被宽松白名单忽略，也不能借“重试日志安装”接管。

## 原操作中断与缺失

- 原请求已保存、目标 descriptor 尚未写入时，只能明确续作同一原请求；必须重新验证全部已安装原生材料。
- 目标 descriptor 完整、SYSTEM 完成证据未写入时，可以在实际读回后补同一完成记录；不重写日志、正文或历史。
- 完成记录存在而目标 descriptor 缺失，或目标 descriptor/完成记录存在而原请求缺失，均拒绝补造。
- 日志安装已完成后，任何日志、普通对象、owner 或完成材料缺失/损坏都拒绝，不从尚存的隔离副本自动恢复。
- `open_chain_selected_generation` 仅要求并读取三份原证据，不安装、续作或修复选择记录。显式选择与只验读是不同入口。

SYSTEM-only 状态保留 `generation-selection-pending` / `generation-selected`，不打开目标。历史报告的 `target_reverified_this_call` 与 `installed_current_reverified_this_call` 为 false，`installed_current` 为 null；这不证明目标现在仍在线或完整。

## 真正的安装后消费者

`TerminalChainSelectedGeneration` 是不可序列化且无 writer 提取能力的持有对象，借用原 SYSTEM owner，并保持原目标锁与实际 generation 目录。它从普通对象路径和实际 generation 日志重建 `CurrentNamespaces`，不是接收 DTO 或把 SYSTEM 快照直接冒充已安装 Current。

`report()` 返回实际安装后的 registry、sequence、snapshot、拓扑代数及继承 mask 后的文件/目录分支条目统计。只有本次实际读回才填充 `installed_current`。分支及目录读取同样使用这份安装后投影与准确 HEAD，且每次访问重验真实目标；持有之后删失普通对象也不能回退到缓存、SYSTEM 或隔离副本。

输出仍明确区分冻结命名空间与内容/运行态：不读取文件正文，不证明全池数据库一致性，不授予活动 Current、SMB 或写入权限。原 `TerminalChainPublishedCurrent` 报告与新消费者复用同一统计实现，差别是新消费者必须提供从实际普通对象重建的投影。

## 独立 Web 与成本

新增 `POST /api/disks/terminal-chain-generation-select` 和 `POST /api/disks/terminal-chain-generation-read`。选择需要原计划、停服声明和单独“冷选择不启用 writer”确认；读回不接受上传 selector 或 Stop 覆盖。控制台按真实阶段显示入口，成功或错误后清除旧表单，重新查询原操作才能继续。HTTP 保留 Host/Origin/CSRF、单任务和退出排空，服务 UID worker 保留真实凭据 revision、host/服务 owner 和固定布局校验。

保持手动 root、`0.0.0.0`、无认证和无 helper 自启。安全确认不等于网络认证，仍只能在可信隔离网络临时使用；不改防火墙，不停止/启动实际服务。

完整输入沿用最多 8,192 对象/128 MiB、三帧 16 KiB 和单 metadata 编码上限。新增最多三个 descriptor 持久名称，SYSTEM 在未完成时预留四倍编码加 1 MiB，目标缺 descriptor 时预留两倍编码加 1 MiB；无正文副本。安装后报告按分支/可见条目扫描，原源链和普通对象的完整核验会重复执行，不宣称增量开销或性能提升。每次无覆盖发布沿用文件和父目录同步，不削减持久化屏障。

## 尚未交付

本 descriptor 不可转换为通用 `PortableAuthority`，普通 Engine 冷启动仍不接纳它。完整 serving/数据库闭包、可信活动 generation 准入、新 epoch/Stop/locator 与活动 Current/writer 的共同切换，以及正式 readmission/外来完整低级链接纳仍未完成。不能因为 `generation-selected` 或某个嵌入首笔结果是 Published 就启动服务、释放来源围栏或宣称 H03 已完成。
