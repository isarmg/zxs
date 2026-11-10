# 磁盘向导核心边界与验证（2026-11-05）

本记录覆盖新池默认拓扑、本盘只读角色识别和管理面类型化接纳接口。发现列表、普通目录转换、外来历史导入和逐盘独立权威是不同边界，不能以本记录代替其验收。

## 已实现的边界

### 新池默认拓扑

`Config::independent_roots` 为每个所选根生成独立的一级盘位 `D01`、`D02` 等，不根据输入顺序推断备份边。管理账户后的 `activate` 和本机 bootstrap 的 `initialize` 共用该构造器。明确提供的本机配置 preset 保留其已审核拓扑；`Config::development` 仍用于明确需要有序备份链的开发配置和测试。

独立根只指备份图。元数据成员、固定多数、全成员发布及写者锁仍按现有池协议执行，没有创建逐盘独立事务域，没有降低成功确认条件。激活仍在写盘之前持久化原始意图，恢复沿同一配置、盘身份和本机授权继续。未知非空根、变化的选择证明和缺席的原根不会被重新初始化。

### 本盘只读角色识别

`recognize_disk` 使用调用方持有的目录 FD，校验本盘 seal、池和盘身份、盘位、绑定代数、设备和 inode，再读取同一根下的配置锚。只读结果 `RecognizedDisk` 的 `RecordedDiskRole` 显式携带证据强度：

- `InitialConfiguration`：初始 seal 没有绑定完整拓扑哈希，只能证明所读配置格式有效且与 seal 的身份一致。这是本盘记录的角色，不能作为经过认证的全历史导入许可。
- `MembershipGeneration`：验证 seal 指定的 authority hash、完整代际配置链、初始成员锚，以及该数据成员所需的本盘 enrollment 证明。它仍不证明业务历史完整、全局最新 HEAD 或跨系统迁移已经安全。

识别结束重读 seal 并复验命名路径与持有根的绑定。初始 seal 下出现未提升的 generation 材料时明确拒绝。不会打开配置中其他磁盘的旧路径，不读取正文，不修复、建目录或改权限。原有 `verify_local_membership` 复用提取的代际校验，保留 Candidate、归档包含证明与元数据 epoch 失效语义。

现有物理 seal 仍绑定设备号和 inode。跨主机移动导致这些值变化时识别拒绝；本改动没有通过忽略身份核验来宣称磁盘可移植。

### 类型化接纳

管理面在 `management` 范围提供 `preview_storage_addition`、`commit_storage_addition` 和 `resume_storage_addition`。HTTP 分派只在真实 wire 边界解析/编码 JSON，向导编排直接调用类型化方法。

三个方法共用原生命周期锁、当前管理身份核验、停止/恢复门禁、固定服务端发现父目录和原始操作意图。提交仍重算并比对完整预览，保留原始 continuation、永久幂等收据、terminal resume、核心代际证明和失败恢复状态。内部调用不获得覆写发现根或伪造管理员的能力。状态查询及显式取消仍沿原入口；没有后台自动写盘任务。

## 验证方法与已通过结果

使用 Linux 临时目录及固定 Rust 1.99.0；Cargo 使用 `--locked --offline`。需要持久文件系统的管理面选择测试设置仓库外、私有的 `TMPDIR`，没有改变生产文件系统准入规则。

- `storage-contract` 的 `model::config_tests`：3/3，含两种模式、六个无备份边的独立根。
- `storage-server` 的 `setup_recovery::activation::tests`：25/25；另 2 项明确要求隔离观察器或 fsync 故障注入，普通 libtest 未执行。包含真实并发首次激活、失败后的冷完成、固定 quorum、原身份与 seal 不变、未获胜根保持空目录。
- `storage-server` 的 `setup_recovery::tests`：42/42。新增 bootstrap 的 Development/Production、乱序选择和冷重开独立根回归；既有本机 preset、恢复、原身份和授权边界回归同时通过。
- `storage-engine` 的 `disk::discovery::tests`：6/6。覆盖另一原根缺席、当前根同文件系统改名、角色不从名称推断、七类配置身份/格式攻击、未提升 generation、真实数据成员代际证明及篡改拒绝。
- 现有 `membership_activation_data_only` 和 `nonmetadata_payload_disk_offline_proofs_require_saved_membership`：合计 3/3。
- `storage-server` 的 `management::storage_addition::tests`：11/11。含 typed/wire 同收据与精确恢复、撤销身份、固定目录范围、共用锁和真实取消/冷恢复。
- `management::terminal_resume::tests`：14/14。
- `management::storage_lifecycle::tests`：12/12。
- `management::storage_readmission::tests`：12/12，包含重哈希来源攻击前拒绝无关根探测、冷重开、撤销授权和跨后续生命周期操作的原收据重放。
- `python3 .github/scripts/check-rust-diagnostics.py -- cargo clippy --locked --offline -p storage-engine -p storage-server --all-targets --message-format=json -- -D warnings`：最终通过，零 Rust warning/error，包含库、单元与集成测试编译目标。

以上定向测试合计 128 项通过、2 项明确未执行。构建使用 `cargo test --locked --offline -p storage-server --lib --no-run` 生成 libtest，再以所列模块过滤器运行。Clippy 不代替集成测试运行。

相邻测试的首次失败包括 `/tmp` 为不支持的 tmpfs，以及三个陈旧夹具假设：枚举发现子目录被新固定父目录策略提前拒绝；创建第二候选根后第一个选择的父目录证明已过期；在没有任何已发布业务边界时执行 exit。修正测试环境与真实初始化步骤，没有弱化生产门禁。首次新增 bootstrap 测试的 `Mode` 所有权编译错误也已修正后重编。最终组合 Clippy 的转换模块错误类型大小与目录测试 lint 由其所有者修复，未添加 lint 例外。

## 外来历史导入仍需独立实现

后续只读验证与目标映射已作为独立步骤实现，实际范围和验证结果见[外来盘历史预览](foreign-history-preview-20261105.md)。它不放开本文所述的迁入、剪裁或逐盘独立写入。

外来磁盘的数据库剪裁不能改写原哈希证明。需要先验证来源发布与完整元数据图，再在新目标生成明确的来源到目标映射、闭包和新证明；原盘及来源证明保留。失去其他磁盘或缺少正文不是历史失效，已验证分支记录应保留并标记“内容暂不可用”，不能改为 `Purged` 或 `Migrated` 来通过现有恢复器。

多一级来源的二级盘必须按可证明的来源池、原一级身份及代际、历史域分别重建独立目录结构。现配置的单个 `upstream_slot_id` 只描述当前边，不足以枚举历史来源。显示名称和盘位名称也不能替代稳定来源身份。目标名称规范化冲突、元数据闭包缺失或来源证明不足均须明确报告，不能静默只导入当前视图。

逐盘独立协议须把来源逻辑身份与本机目录 FD 绑定分开。新目标证明应绑定来源发布摘要、实际保留图、来源到目标映射、暂缺正文清单及实际复制并校验的字节；缺少元数据证明必须拒绝，不能用“暂不可用”掩盖未知历史。系统盘数据库只能作为可重建投影，先完成盘内权威发布再推进其缓存序号，不能用无顺序的双写假定一致。本批保留现有协议，未提前引入这些新证明格式。

现有历史包校验可作为发布绑定和图闭包验证的基础，但现 `restore_history_package` 要求所有 `Available` 正文存在，并且只有单历史域到一级盘的目标约束；不能直接满足暂不可用内容、多来源空目录重建或跨主机物理身份迁移。本批不宣称这些流程已经实现。

## 成本与未覆盖项

默认拓扑构造为所选根数的线性工作。只读识别读取有界配置、seal 和所需代际元数据，不遍历正文或其他根，但代际证明大小仍影响成本。类型化重构没有增加磁盘同步阶段或改变锁顺序；接纳继续承担原有固定成员、持久意图与发布成本。

本记录不是全仓验收，也不包含真实用户磁盘、实际部署、跨主机搬盘、物理掉电或逐盘独立权威协议验收。
