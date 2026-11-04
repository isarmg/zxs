# 可恢复删除登记：P1 纯影响规划

本阶段实现 `Engine::preview_disk_deregistration`，不实施磁盘删除。删除的目标是登记以及目标独占内容的当前可见性，不是原业务文件；备份、旧版本、目录组合和永久历史不能按磁盘名称批量删除。后续注销必须先完成目标根恢复材料的完整持久化与验证，本预览不证明这些条件。

这里的“盘”仅指 ZXS 已登记的数据存储目录；metadata_member 是该目录的角色。预览不发现或操作 Debian/OS 设备，不要求独立物理磁盘或不同文件系统，也不排除用户指定在系统文件系统内的目录。Engine 的 root 是 ZXS 控制状态目录，不是操作系统盘。

## 已实现的来源与类型边界

- 唯一生产构造入口借用当前 Engine 的 `ValidatedConfig` 和已准入 `State`。`CurrentDiskRegistry` 是私有类型，没有公开的任意 Registered 集合、反序列化构造器或增删入口。结构校验通过的外来 Config 本身不是当前池权威。
- 当前实现仍以 Engine 已准入 Config 的全部 disks 作为当前登记。只支持现有身份保持的代际假设：已有 slot/disk/binding 不被偷偷替换。未来持久 registry、注销、rebind 必须提供新的经过认证的来源；不能直接删除 Config 项后调用本模块。
- 请求准确比较 `(pool_id, disk_id, slot_id, binding_generation)`。对每个版本解析真实 Placement 路由和 binding_generations，拒绝槽位缺失、绑定缺失/不符、重复路由、错误主盘和错误版本身份。旧 Placement 不包含 disk_id，disk_id 来自当前已准入、身份保持的配置链；这不是对任意历史重绑定的通用证明。
- `FrozenDeregistrationBoundary` 只引用实际 Engine sequence、business/topology generation、可选已发布 snapshot hash 和已准入 membership generation hash。没有候选哈希、认证 registry root 或 impact root 时不合成替代值。该边界不是新发布证书，不能交给删除执行器作为授权。
- Config 尚无持久 Disabled 字段。本模块不读取 health、在线状态、held-root、目录、容量或观察结果，也不创建 Enabled/Disabled 状态。保留在登记中的离线/Disabled 盘仍应提供登记可见性；本阶段不宣称已经实现持久禁用 I/O。

## 版本与 occurrence 结果

`versions()` 扫描每个版本，返回 Unaffected、TargetExclusive 或 SharedWithRegistered，并保留原 ContentState、target_copies、primary slot。`remaining_holders` 是只读惰性迭代器，包含准确 pool/disk/slot/binding 及现有 level/upstream/voter 角色。不因为没有 metadata vote 而丢弃数据盘，也不把逻辑登记持有关系称为正文已实读验证。

- 目标是唯一持有者：退出当前内容投影；旧版本、Commit、备注、目录树和原字节不改。
- 目标是主盘但仍有备份：返回 RetainedContentOnly，不能在被移除的主盘根制造入口，不能授予备份写入资格。
- 目标只在备份路由：保留现有主命名空间，其他副本仍可见。
- HEAD 仅在目标、旧版本在其他盘：当前 HEAD occurrence 隐藏；显式历史 scope 保留旧版本，不将旧版本提升 HEAD，不按 file_id 隐藏整段历史。
- Purged/Migrated 的 ContentState 原样返回，不凭登记或残留字节将其变为 Available。普通内容读取仍必须遵守该状态。

`namespace()` 接受明确的 BranchHead、DirectoryHead、Commit 或 DirectoryTree。返回的冻结 scope/owner/head 只能读，调用方不能修改内部 owner 后重新计算影响。每条 occurrence 的身份是 `(scope, entry_id)`；共享文件在不同目录分支中的出现分别保留，读取每个 scope 的准确 entries，不追踪后来可变 HEAD，也不重写 directory_refs 或选择。

纯目录由原 namespace owner 及同一冻结 namespace 的保留后代决定。owner 保留时，空目录仍在；owner 删除且没有保留内容时隐藏。父路径判断使用规范 lookup key 的 `/` 边界，不能把 `dir-other/file` 当作 `dir` 的后代。RetainedContentOnly 只表示逻辑内容仍有登记归属，不是已经生成真实 backup listing。整个 occurrence 结果只描述登记影响；分支生命周期、当前选择、管理员会话及其他现有 listing 准入仍由原调用方判断，不由此结果重新授权。

## 冻结、分页与资源预算

预览借用 Engine；Rust 生命周期禁止扫描期间取得同一 Engine 的可变访问。因此 scan 不会跨 HEAD/登记更新拼页，也不需要伪造一个可重用网络 cursor。此接口不适合作为跨请求游标：将来解锁后分页必须另做认证冻结根和 ListingChanged 检查。

- 构造只借用所用业务表与当前配置，额外空间 O(1)。不克隆 State、整池 Snapshot、control_auth、永久 operations/principals 或根文件内容。输出类型没有序列化完整 State/Config 的入口。
- `ImpactPageSize` 限定 1..=256。版本与 occurrence scan 各持一个惰性迭代器，每次最多分配该数量的结果行；ID/路径/持有者借用原数据，持有者不会扩展为无界 Vec。
- 每个版本检查 R 个路由和 D 个登记的成本为 O(R×D + R²)，另有表查找。顺序版本扫描不从第一页重扫。受影响和不受影响版本都可组成后续计划；错误是明确结果行，调用方必须展示，不能当不存在略过。
- 对一个含 E 条目的 namespace，无主 owner 的目录 occurrence 最坏扫描 E 个后代，每个后代按上述版本成本解析。因此完整目录投影最坏 O(E²×版本解析成本)。它只界定输出/额外内存，不宣称总 CPU 或持有 Engine 读取借用的时间有界。没有后台索引、增量缓存或取消协议。
- 无文件读写、网络、SQL、fsync、权限或部署行为；不会返回可执行删除动作。

## 本批测试与未覆盖

新增 `test/engine/unit/engine/disk_deregistration_plan_tests.rs`，通过 `disk_registry` 的私有测试模块接线。纯内存 fixture 覆盖独占 a、共享 b、backup-only、主删备留、独占 HEAD/共享旧版、目录分支共享 occurrence、空目录、非投票登记、无需根 I/O、pool/disk/slot/binding spoof、损坏 Placement、Purged/Migrated、分页完整性及 Config/完整 State/Placement 规范字节和哈希不变。编译失败示例描述借用期间不能重新取得 writer。

本地实际只执行直接 rustfmt 和 git diff 检查；没有编译或运行上述测试，结果需由集成 CI 确认。没有执行真实磁盘/故障注入/持久禁用 I/O/恢复测试，未将源码断言称为物理耐久验收。

后续仍需：持久认证 registry 和历史拓扑分离、old-only 联合成员、根恢复 capsule/独立验证与恢复冲突、所有普通/维护 I/O 停止、工作区闭包、真正的注销/恢复状态机及永久收据、web/API/SMB/listing/search/SQL 和 fallback 的一致投影、跨请求分页失效。当前普通产品视图没有接入此假设结果，原操作行为完全不变。
