# 离线救援工具

`storage-rescue` 生成 `zxs-rescue`，它是明确命名的命令行救援工具，不是日常管理客户端。正常入口是直接运行的 `zxs` 守护进程，首次初始化、唯一管理员改密及仓库、策略、计划等操作在 Web 管理面进行；不提供多账户或仓库授权管理。SMB 网络服务仍由独立 Samba/smbd 桥接提供。

所有需要打开池的救援操作应先停止该池的服务。`--root` 指向系统状态目录，`--principal` 指定审计与操作所属主体，默认 `admin`，不是角色或 ACL 提权入口；工具没有管理员重置能力。帮助、版本与参数错误不启动服务或工作线程。失败返回非零退出码。

需要打开或恢复池的命令支持[四项保留证据运行期上限](../19-capacity-and-performance.md#runtime-retained-workspace-limits)，作为全局参数同时提供；不带时使用原配置基线。`offline-check` / `offline-export` 直接核验单盘，不借这些参数改变证明或载荷规则。恢复前应配置足够额度，不能通过删除已提交清单释放名额。

## 恢复系统盘

```sh
zxs-rescue --root /srv/zxs/new-system restore-system \
  --disks /srv/zxs/old-metadata-1 /srv/zxs/old-metadata-2
```

上述为多成员示例。实际至少需要固定配置的 Q = floor(N/2)+1 个可信原元数据成员，N ≥ 1；单成员池可只传唯一旧盘。不能拿空盘、替换盘或任意目录替代，不能按在线数量重算成员集合。所有新初始化盘保存池配置锚；缺锚旧盘不兼容，不自动补锚或迁移。核心校验发布证据、成员与池身份并保留冲突拒绝规则。工具不格式化磁盘、不自动重置、不在多成员池中根据不足 Q 个旧成员推断整个池的最终状态。单成员盘永久丢失不具备原池恢复保证。

输出中的 `credential_recovery_snapshot_available` 只表示恢复后的池中存在凭据灾备快照，不表示已创建或替换管理面权威凭据。工具不写旧 `auth.json` 缓存，也不覆盖独立系统目录中的 `management-auth.json`。随后启动守护进程或重试存储发现都不会自动导入该快照，无论是否已有权威标记。有效的本机管理凭据继续作为唯一权威；凭据缺失或损坏时保持认证关闭。需要恢复管理凭据时，保留此次恢复目录作为核验依据，停止相关服务，并在另行准备的干净系统状态目录中走受本机 bootstrap 授权保护、明确接受旧凭据风险的恢复流程。不要删除或覆盖现有管理凭据、权威标记或部分恢复目录来绕过保护；工具不提供凭据重置命令。

## 受限降级恢复

```sh
zxs-rescue --root /srv/zxs/system recovery-preview OPERATION_ID --reason '经审查的原因'
zxs-rescue --root /srv/zxs/system recovery-commit RECOVERY_ID \
  --digest REVIEWED_DIGEST --accept-permanent-loss
```

仅支持已经获得 ACCEPT 但尚未发布、所有原始数据盘都可访问且正文确已不可修复的操作。执行时必须持有池的独占锁及适当本机权限。先审查持久预览和损失范围，再提交完全匹配的摘要并明确接受永久数据损失。缺盘替换、自动重试发布、自动 reset 和任意撤销已发布历史均不属于此能力。

## 显式完整巡检（第一阶段）

```sh
zxs-rescue --root /srv/zxs/system scrub
```

先停止该池服务。此命令是完整巡检执行器的真实离线调用方，不是定期后台调度，也不替换 HTTP `/api/admin/check` 或 `offline-check` 的单盘目录诊断范围。当前 `Engine::scrub` 在整次扫描中持有 Engine 独占借用；不得把本命令改为默认周期在线任务，或据此宣布增量发布免读门禁已经满足。

命令先安装 SIGINT / SIGTERM 接收器，再在一个专属本机线程中打开池；系统目录和可用磁盘的既有非阻塞写者锁拒绝同池服务或第二个巡检进程，不排队积压。打开使用 `PreservePending` 恢复入口，不自动提交未发布决定。**打开阶段仍会执行既有恢复、日志尾修复、已清除正文清理和可重建索引维护，不是整个命令只读。** 只有随后的 `scrub` 扫描是无隐式修复的观测；恢复可能先修复可恢复的元数据副本，因此报告反映打开之后的实际字节，不能当作打开之前损坏清单。无法安全打开时不运行巡检、不输出健康报告，错误写到 stderr，退出码为 1。

SIGINT / SIGTERM 只请求协作取消，不终止或分离 I/O 线程。核心在对象、日志帧和正文块之间检查取消；已在执行的系统调用及打开阶段必须实际结束。命令等待线程 join 后才返回；调用 future 被取消或销毁也先取消并真实 drain。OS 强制终止、设备永久阻塞及物理断电不属于协作取消的完成保证。线程创建失败、线程 panic、信号安装失败和核心准入错误都明确失败，不伪装成完整报告。

完成后 stdout 只输出一份 JSON：`state` 为 `complete_success`、`partial_failure`、`cancelled` 或 `failed`，并包含核心的发布 `sequence` / `snapshot_hash`、实际覆盖计数、至多 128 条 issue、截断数量及起止时间。时间使用 `{ "secs_since_epoch": …, "nanos_since_epoch": … }`。只有完整覆盖且无故障的 `complete_success` 返回退出码 0，并令 `last_complete_success_at` 等于本次完成时间；其它扫描结果仍输出部分覆盖报告并返回 1。输出或编码本身失败同样返回 1。

`last_complete_success_at` **只属于本次调用**：命令不保存跨进程健康缓存，不读取普通写入成功时间，不宣称先前结果仍新鲜；每次启动从未检查状态重新准入。报告是扫描区间观测，不能用作未来读写许可、当前健康凭证或自动修复授权。离线阶段没有 HTTP 任务队列、在线 status/cancel 路由及自动周期策略；最终在线方案必须先完成有界锁外执行、具体后台 owner、恢复 drain 与前台资源公平性的独立设计和验证。

## 单盘核验与隔离导出

```sh
zxs-rescue offline-check /srv/zxs/old-disk
zxs-rescue offline-export /srv/zxs/old-disk VERSION_ID /srv/rescue/new-file
```

在停止写入的可信原成员或一致的只读副本上操作。核验或导出只证明被检查的发布证据、单个版本与其数据完整性，不证明整个池可恢复、当前在线认证、最新历史或所有副本健康。导出目标不得已存在；不要把隔离导出结果直接当作可重新加入池的磁盘。

## 历史恢复包

```sh
zxs-rescue --root /srv/zxs/system package export HISTORY_ID /srv/private-rescue/new-package
zxs-rescue package check /srv/private-rescue/new-package
zxs-rescue --root /srv/zxs/new-system package restore /srv/private-rescue/new-package \
  --primary-slot P01 --copies 3
```

保留原有完整历史包导出、独立核验和新池恢复能力，不是 Web 文件传输。导出目标不得已存在，父目录必须为当前用户拥有的私有 0700 目录；包的路径、发布证明、摘要、对象与历史结构仍由原核心实现核验。没有外部可信成员配置时，独立包检查仅证明包内证据与内容自洽，不赋予来源池成员关系权威。恢复要求目标池已安全初始化，保留目标身份、副本和新池恢复约束，导入形成目标池的新本地发布，不沿用外来发布作为本池权威，不能覆盖现有目标或绕过损坏证明；可用 `--operation-id` 指定经审查的幂等操作标识。

<a id="uninitialized-configuration-drafts"></a>

## 未初始化拓扑配置草稿

```sh
zxs-rescue configuration template --production \
  --disks /srv/zxs/disk1 /srv/zxs/disk2 /srv/zxs/disk3 --output /srv/private-layout/custom.json
zxs-rescue configuration register-disk --config /srv/private-layout/custom.json \
  --slot P02 --disk-id REVIEWED_DISK_ID --path /srv/zxs/disk4 --level 1
```

此子域只生成或编辑未初始化的配置草稿，不打开池、不初始化设备、不注册在线磁盘，也不提供账号初始化或远程客户端。`template` 生成随机池/盘 ID；必须显式指定 `--production` 才是生产草稿。`register-disk` 保留原草稿到完整文件名加 `.before-registration` 的备份，再以描述符锚定的原子写入发布 0600 文件。再次编辑保留紧前一份草稿。从读取、校验到备份和发布，整个事务持有父目录的排他建议锁，而不是会被原子替换的草稿 inode；同目录并发编辑（包括父目录别名）在读取前明确失败，须待前次编辑结束后重新运行，不会静默覆盖成功编辑或保存过时备份。锁不创建边车文件；标记检查及两次发布始终使用同一已打开的父目录描述符。此锁只协调救援草稿编辑，不能替代停止守护进程及排除其他写入者。已有池或管理凭据标记、已入池盘、复制出去的在线配置、链接、重叠根目录、非法拓扑及不可信文件均被拒绝；不能用不相关的 `--root` 绕过这些检查。

自定义草稿通过守护进程本机参数 `--setup-config ID=/absolute/private-config.json` 预置，再由受保护 Web 首次初始化选择该 ID。文件须由服务账号拥有、为单链接普通文件、权限 0400/0600；浏览器不能提交任意路径或任意 Config JSON。所有配置磁盘仍必须在 `--setup-disk` 白名单中，沿用生产挂载、设备身份、空池和引导授权检查。缺失或损坏的预置文件不能阻止管理 HTTP 服务启动。Web 向导最少选择一个存储盘，多盘时按选择顺序建立可选备份链；单盘没有冗余。预置方案保留经验证的高级配置参数。

## 副本与发布约束

除系统盘外最低只需一块存储盘，后续备份层均可不配置；单盘没有冗余或故障盘容错。`configuration template --disks /absolute/primary` 可生成单盘草稿，增加路径时按顺序形成备份链；草稿本身不访问或初始化设备，实际使用仍须校验拓扑和设备。

正常元数据发布要求全部已配置元数据成员持久化成功；严格多数仅用于相应恢复/准入流程，不能自动把失联成员移出配置。`package restore` 默认一个副本；需要更多副本时显式设置 `--copies N` 并预置足够的备份链。

## 检查

运行 `cargo test --locked --offline -p storage-rescue`。`test/rescue/startup.rs` 覆盖命令边界、损失确认、元数据恢复、历史包字节一致性及拒绝覆盖/损坏证明；`test/rescue/unit` 覆盖草稿拓扑、权限、锁和原子备份，以及巡检的真实信号、取消、future 销毁、线程失败和 Engine 锁释放；巡检集成用例实际运行命令并检查健康、缺盘和准入失败结果。测试不替代真实设备及生产恢复演练。
