# 21 配置规范与默认值

[文档目录](README.md) · [上一章：工程结构与依赖约束](20-engineering.md) · [下一章：验收标准与证明材料](22-acceptance.md)

<a id="s21"></a>

目标配置、逐级来源示例及关键参数的约束。

## 本章目录

- [21.1 配置示例](#s21-01)
- [21.2 默认值的边界](#s21-02)
- [21.3 配置分类与权威顺序](#s21-03)
- [21.4 启动校验与变更](#s21-04)

<a id="s21-01"></a>

## 21.1 配置示例

以下 TOML 定义完整目标配置，不是现有进程直接读取的配置或可直接部署的 smb.conf。当前引擎读取严格 JSON `Config`；产品首次初始化通过 Web 受保护引导，操作员先在守护进程 `--setup-disk ID=/absolute/path` 白名单中配置本机可信盘位。此流程不挂载、分区或格式化设备。下述目标配置字段不表示 Web 已提供任意 Config 编辑器。实际接受的字段和限制见[服务端说明](components/server.md)、[救援配置草稿](components/rescue.md#uninitialized-configuration-drafts)及[当前限制](23-delivery-and-decisions.md#s23-03)。

Samba 配置由经版本验收的部署模板另行生成。设备清单通过登记工具产生，不在示例中填写真实设备路径。

除系统盘外最低只需一块存储盘，二级及后续备份层均为可选。单盘配置只有一级盘、一个元数据成员和一份业务内容；没有磁盘冗余，也不承诺唯一盘丢失后的恢复。下列多层、三元数据成员示例展示可选扩展，不是最小部署要求。

示例展示目标配置的组织方式，包含协议常量断言、当前运行额度的文档投影和尚未实现的设计目标；通过文档契约检查不表示整份 TOML 已实现或可交给进程。只有下文明确列出的 JSON 字段或 CLI 参数是当前配置入口。分类及冲突处理见[第 21.3 节](#s21-03)。

```toml
[server]
mode = "single-writer"
local_socket = "/run/zxs/service.sock"
remote_transport = "https"
http2 = true
http3 = false
require_verified_tls = true

[smb]
enabled = true
server_backend = "samba"
business_bridge = "vfs-to-core"
supported_protocol_family = "SMB3"
allow_smb1 = false
allow_anonymous = false
allow_guest = false
require_signing = true
require_encryption = true
share_target = "history-main-branch"
basic_file_operations_only = true
allow_branch_switch = false
expose_history_payloads = false
save_boundary = "flush-write-through-or-last-writable-close"
retain_failed_server_workspaces = true

[[smb.shares]]
share_id = "share-example"
name = "personal"
history_tree_id = "history-example"
main_branch_id = "br-main"
share_generation = "1"

[storage]
filesystem = "btrfs-single-device"
disk_inventory = "registered-physical-disks"
minimum_disks_for_protected_write = 1
allow_explicit_disk_enrollment = true
content_layout = "full-file"
writable_disk_format = 2
allow_cross_version_reflink = false
allow_content_hardlinks = false
allow_implicit_content_deduplication = false
allow_cross_tree_dependencies = false

[metadata]
replica_members = 3
decision_members = 2
recovery_evidence_members = 2
publish_after_all_members = true
acknowledge_after_all_members = true
max_inflight_decision_slots = 1
require_fenced_membership_change = true
allow_automatic_degraded_writes = false
sqlite_role = "rebuildable-index"
application_log = true
member_slot_ids = ["P01", "B01", "C01"]
member_selection = "user-specified"

[versions]
immutable_after_seal = true
historical_save_creates_branch = true
default_save_mode = "full"
supported_save_modes = ["full"]
automatic_topology_expiration = false

[branches]
creation_mode = "record-until-write"
content_mode = "inherit-parent-until-first-write"
materialize_all_files_on_create = false
main_branch_required_for_smb = true
merge_enabled = true
ordinary_commit_parent_count = 1
# 以下 merge_* 字段仅描述兼容的 MergeLink 关系标记，不限制文件分支合并。
merge_behavior = "link-only"
merge_target = "selected-existing-version"
merge_creates_content_version = false
merge_updates_branch_head = false
allow_history_rewrite = false
concurrent_authenticated_sessions = true
file_write_ownership = "server-arbitrated"
serialize_branch_publication = true
exclusive_maintenance_plans = true
auto_create_conflict_branch = false
switch_requires_clean_state = true

[history]
preserve_topology_after_purge = true
explicit_content_purge = true
automatic_content_purge = false
protect_active_heads = true
check_live_references_before_purge = true

[tree_split]
enabled = true
selection = "cut-commit-and-descendants"
new_root_mode = "full"
independent_metadata_and_payloads = true
copy_full_files_without_reflink = true
preserve_purged_nodes = true
atomic_publish_both_trees = true

[naming]
policy = "PortableName"
path_policy = "PortablePath"
unicode_data_version = "16.0.0"
canonical_form = "NFC"
case_sensitive = false
preserve_display_case = true
reject_noncanonical_new_names = true
auto_rename_invalid_names = false
component_utf8_bytes = 240
component_utf16_units = 240
relative_path_utf8_bytes = 768
relative_path_utf16_units = 200
max_directory_depth = 32

[replication]
default_target = 1
preset_targets = [1, 2, 3]
minimum_target = 1
maximum_target = "primary-route-depth"
freeze_policy_per_version = true
inherit_for_retained_history = true
strict_on_commit = true
count_client_cache_as_replica = false
placement_mode = "user-tiered-backup-topology"
allow_random_placement = false
allow_unmapped_fallback = false
allow_automatic_role_promotion = false
allow_automatic_cross_route_rebalance = false
require_all_route_levels = true
count_off_route_copies = false

[backup_topology]
id = "topology-example"
generation = "1"
require_adjacent_levels = true
one_next_target_per_source = true
require_distinct_physical_disks = true

[[backup_topology.slots]]
slot_id = "P01"
copy_level = 1
binding_generation = "1"
disk_id = "disk-example-p01"
source_slot_ids = []

[[backup_topology.slots]]
slot_id = "P02"
copy_level = 1
binding_generation = "1"
disk_id = "disk-example-p02"
source_slot_ids = []

[[backup_topology.slots]]
slot_id = "P03"
copy_level = 1
binding_generation = "1"
disk_id = "disk-example-p03"
source_slot_ids = []

[[backup_topology.slots]]
slot_id = "P04"
copy_level = 1
binding_generation = "1"
disk_id = "disk-example-p04"
source_slot_ids = []

[[backup_topology.slots]]
slot_id = "B01"
copy_level = 2
binding_generation = "1"
disk_id = "disk-example-b01"
source_slot_ids = ["P01", "P02"]

[[backup_topology.slots]]
slot_id = "B02"
copy_level = 2
binding_generation = "1"
disk_id = "disk-example-b02"
source_slot_ids = ["P03", "P04"]

[[backup_topology.slots]]
slot_id = "C01"
copy_level = 3
binding_generation = "1"
disk_id = "disk-example-c01"
source_slot_ids = ["B01", "B02"]

[[backup_topology.slots]]
slot_id = "D01"
copy_level = 4
binding_generation = "1"
disk_id = "disk-example-d01"
source_slot_ids = ["C01"]

[primary_placement]
require_user_rule = true
allow_automatic_primary_selection = false

[[primary_placement.rules]]
history_tree_id = "history-example"
path = "/"
primary_slot_id = "P01"

[[primary_placement.rules]]
history_tree_id = "history-example"
path = "/media"
primary_slot_id = "P03"

[reading]
require_full_download_before_open = false
pin_version_for_committed_read_handle = true
range_reads = true
checksum_chunk_bytes = 262144
readable_checksum_chunk_bytes = [65536, 262144, 1048576, 4194304]
verification_mode = "server-verified-range"

[writing]
workspace_model = "session-overlay"
automatic_commit_scope = "file-and-required-namespace"
serialize_commits_per_session = true
freeze_file_writes_during_commit = true
require_expected_head_for_explicit_batch = true
require_file_and_namespace_preconditions = true
allow_verified_unrelated_head_advance = true
require_idempotency_key = true
writable_shared_mmap = false
offline_auto_commit = false

[capacity]
warning_used_percent = 80
critical_used_percent = 90
growth_sample_days = 7
warning_days_remaining = 30
per_disk_reserve_percent = 10
metadata_reserve_policy = "checkpoint-plus-recovery"
reserve_space_per_target_disk = true

[garbage_collection]
enabled_for_verified_unreachable_objects = true
orphan_grace_days = 7
pause_when_recovery_incomplete = true
pause_when_transaction_in_doubt = true

[resource_limits]
# 当前入口的文档投影，不是新增 TOML 配置接口。
# max_workspaces 来自引擎 JSON Config；其余当前字段对应同名连字符 CLI 参数。
max_workspaces = 128
http_transfer_slots = 2
http_spool_bytes_per_account = "107374182400"
http_spool_bytes_total = "536870912000"
http_spool_recovery_reserve_bytes = "1073741824"
smb_spool_bytes_per_account = "107374182400"
smb_spool_bytes_total = "536870912000"
smb_spool_recovery_reserve_bytes = "1073741824"
# 以下分页、事件重放和提醒字段仅为设计目标，当前没有对应运行配置入口。
default_page_items = 200
maximum_page_items = 1000
page_snapshot_ttl_seconds = 900
event_retention_days = 7
event_retention_bytes = "1073741824"
workspace_reminder_days = 7

[observability]
# 诊断目标，不表示当前已有相应告警器或应用等待队列。
decision_warning_seconds = 30
decision_critical_seconds = 120
queue_warning_seconds = 30
```

<a id="s21-02"></a>

## 21.2 默认值的边界

`component_utf16_units=240` 不代表任意 240 字符名称在任何位置都可创建；全相对路径还有 200 单元的保守限制。两个限制分别验证，错误应准确说明是哪一项。

`orphan_grace_days=7` 不授权删除历史拓扑、有效内容pin、受保护版本或未决计划。历史目录仍引用的payload可以在显式清除事务完成后回收；仅凭提交可达不能禁止已授权清除，也不能授权绕过墓碑直接删除。时间条件不替代内容状态与引用检查。

`case_sensitive=false` 只影响约定的逻辑命名空间，不要求把底层 Btrfs 变成另一种文件系统。内部对象通过 ID 定位，名称比较由核心负责。

本例配置三个元数据成员，所以恢复/准入多数为2；任意非空配置的多数固定为 floor(N/2)+1，N 为已配置成员数。`decision_members=2` 不能解释为本例两成员即可正常写入。正常保护、发布和确认始终要求全部已配置成员完成；不按可访问数量缩减门槛。单成员池要求该成员，双成员池要求两个成员。`maximum_target="primary-route-depth"`按具体一级来源的连续链深计算；总盘数只给出全局上界。离线、满盘或映射缺失不能擅自降低目标或使用别的来源盘。

`supported_save_modes=["full"]` 约束实际内容保存；`creation_mode="record-until-write"`、`content_mode="inherit-parent-until-first-write"` 与 `materialize_all_files_on_create=false` 约束分支只登记冻结来源、首写前继承父内容。目录分支直接引用父子树，不提前创建自己的子树或后代文件分支。历史另存建立新分支，但只为实际 staged 文件生成独立 FULL，其余文件继续继承冻结来源。这些是目标配置的受检断言，不是当前 JSON `Config` 的运行开关；实际能力声明见[API 能力发现](15-api-and-errors.md#s15-05)。

示例中 `merge_behavior="link-only"`、`merge_updates_branch_head=false` 等 `merge_*` 字段只描述旧 `MergeLink` 关系标记：端点固定、正文与HEAD不变，不是所有合并操作的全局约束。文件分支合并 `FileBranchMerge` 是另一个已实现能力，接口声明 `merge_behavior:"explicit-file-node-adoption"`，会推进归属与参与目录引用；目录分支合并待用户确认，暂不实现。两者见[补充规则](design/file-merge-tags-backup.md)，不能通过修改旧字段开启或关闭新语义。`preserve_topology_after_purge=true`保留图节点，不永久锁住payload。`tree_split`要求新域对象和恢复包独立，不只是分支引用改名。

`smb.shares` 固定历史树与主分支；Web 切换工作分支不修改该配置。普通会话不独占整个分支，按文件核对并串行发布；显式批量计划仍锁定 HEAD，维护计划处理相关工作区后才取得排他权。`auto_create_conflict_branch=false` 禁止静默处理冲突，不影响用户从历史保存时按规则登记新分支并只保存实际写入的 FULL 版本。

上述示例有4个一级、2个二级、1个三级和1个四级盘位。primary_placement按规范目录的最长匹配规则解析，`/media`落在P03，其余由根规则落在P01；不同目录规则冲突必须拒绝，不能用规则声明顺序猜测。disk_id仅为占位，实际必须通过设备登记与物理身份校验。

`member_slot_ids`在本例是用户另外指定的三个控制元数据成员；一般配置可指定任意非空成员集合，不等于每个业务版本都保存三份。盘位配置、业务路由和控制成员发生变更时分别校验并按需要组合迁移，普通启动不重新随机选成员。

`verification_mode` 是默认服务端校验模式，保留的受控 HTTP 客户端库支持完整性验证，产品 CLI 不提供正文传输或验证协商入口；SMB 客户端不需要实现本系统校验协议，由服务端在返回字节前校验。段参数仅对新版本生效；读取旧版本时以其描述为准。容量阈值与校验段默认值都是待实测的初始配置，不能宣称为适合所有设备的最优参数。

`resource_limits` 中当前额度投影的实际来源如下；字节值以十进制字符串展示，CLI 则接受十进制整数参数。`max_workspaces` 是全池 open/sealed 工作区数，不是每账户额度；128 是当前初始化草稿的值，不是不可调协议常量。唯一管理员的所有会话共同占用额度。

| 文档投影字段 | 当前入口及作用域 |
| --- | --- |
| `max_workspaces` | 引擎 JSON `Config.max_workspaces`；初始化前的受审查草稿，见[救援配置草稿](components/rescue.md#uninitialized-configuration-drafts) |
| `http_transfer_slots` | `--http-transfer-slots` → `ServerConfig.http_transfer_limits.slots`；上传、读取各自一池，每池默认 2，不是两池合计 2 |
| `http_spool_bytes_per_account` / `http_spool_bytes_total` / `http_spool_recovery_reserve_bytes` | 对应 `--http-spool-*` 参数 → `ServerConfig.http_spool_limits`；只约束 HTTP spool |
| `smb_spool_bytes_per_account` / `smb_spool_bytes_total` / `smb_spool_recovery_reserve_bytes` | 对应 `--smb-spool-*` 参数 → `SmbConfig.spool_limits`；只约束 SMB spool |

两种协议的 spool 各自要求账户额至少 65536 字节、总额不低于账户额、恢复余量大于零。`per_account` 是现有入口与账本的所有者维度名称，不表示存在多账户管理；未知归属字节也不能绕过唯一主体准入。HTTP 与 SMB 的默认 100 GiB / 500 GiB 分别生效，不能解释为跨协议共享的 500 GiB 池。核心 `max_workspace_bytes` 和持久工作区证据四维额度仍独立，见[当前资源边界](19-capacity-and-performance.md#s19-10)。

普通 HTTP 后端的 8 槽是 `HTTP_BACKEND_SLOTS` 内部常量；SMB 的 64 条连接、8 条已认证会话和每会话 1024 个句柄分别由 `MAX_CONNECTIONS`、`MAX_AUTHENTICATED_SESSIONS`、`MAX_HANDLES` 限制。这些不是本 TOML 或 CLI 开关，也没有另一个 4096 全池句柄上限。HTTP 传输及后端准入池满时立即拒绝，不存在旧示例所写的 128 项应用等待队列。

`default_page_items` 至 `workspace_reminder_days` 以及 `observability` 只保留设计目标约束，不声明当前分页接口、事件重放、定时提醒或这些告警器已经实现。实际 SMB 目录页固定最多 256 项，不能用这里的 200/1000 目标覆盖它。修改文档投影不会修改任何运行额度；schema 的范围和关系检查也不意味着进程已接受整份目标配置。

只实现当前唯一存储格式，不提供可配置的旧格式读取列表或兼容分支。格式修改必须同步更新写入、恢复、校验和测试向量，不自动迁移真实数据。resource_limits 与 observability 的数值、计费口径和超限动作见[第 19.9 节](19-capacity-and-performance.md#s19-09)及[第 19.10 节](19-capacity-and-performance.md#s19-10)。告警秒数不是提交取消期限。

<a id="s21-03"></a>

## 21.3 配置分类与权威顺序

| 类别 | 示例 | 修改与校验规则 |
| --- | --- | --- |
| 协议常量或构建能力断言 | metadata 的非空配置成员集合及固定多数 floor(N/2)+1、max_inflight_decision_slots=1、全部成员发布/确认、禁止自动降级、full-only、旧MergeLink的link-only语义、禁历史改写与跨树依赖 | schema 使用 const 或固定枚举；冲突返回 CONFIG_INVARIANT_VIOLATION，不能当作可调布尔开关；文件分支合并能力单独声明 |
| 运行参数及待实现目标 | 当前已支持的网络监听和资源额度入口；分页、observability、维护并发等目标另行标注 | 实际入口进行类型、范围和交叉检查；目标字段不是可用开关。当前资源额度不由此 TOML 热更新，运行行为以组件文档及实现为准 |
| 受事务管理的持久状态 | 成员集合、盘位/物理绑定、拓扑代际、共享归属、命名策略、一级规则和保护策略 | 配置只提出初始化目标或变更计划；有效检查点和日志决定当前值，必须经对应业务事务修改 |

首版未明确列为运行参数或已实现可选能力的字段，均不得通过配置扩展新语义。命名策略中的长度、Unicode 版本和比较规则是同一策略身份的一部分，不能只热改数值而保留原策略版本。协议常量文件可用于核对和能力展示，不是绕过规范的入口。

已有存储池的权威顺序为：可信成员配置链与检查点/日志 → 已发布业务状态 → 可重建索引与配置展示。启动配置与已发布状态冲突时不自动覆盖任一侧；报告具体字段及双方代际，进入 CONFIG_CONFLICT 维护状态。无盘上身份时，只有显式初始化流程才能采用初始化目标，普通启动仍禁止建空池。

<a id="s21-04"></a>

## 21.4 启动校验与变更

目标启动校验顺序为：严格解析并拒绝重复键/未知字段 → 常量及范围检查 → 各独立预算的所有者/总额关系检查 → 物理身份和拓扑检查 → 权威代际比较 → 事务恢复 → 发布就绪状态。此处不声明当前进程解析上文 TOML；当前 JSON 和 CLI 分别执行其已实现的校验。不得把校验失败自动修正成默认值后继续写入。

当前引擎在公开初始化入口、启动/恢复读取和预置方案读取边界，将 `Config` 转为私有拥有原始配置的 `ValidatedConfig`。盘位/磁盘标识格式、连续拓扑、元数据成员和资源范围只在转换时检查；引擎与脱离写锁的读取/规划快照持有该类型，不再重复全量校验。初始化规范化路径、恢复重绑路径及救援草稿解析目录别名后，只检查根路径数量、绝对路径/父级穿越及根之间的重叠。运行期保留证据额度覆盖在发布初始化意图前与持久基线比较，随已验证配置携带且不写入 JSON；现有配置文件及线格式不变。物理身份与 I/O 检查仍按各自的磁盘边界执行。

`ValidatedConfig` 以单个 `Arc` 共享私有准入记录：原始 `Config`、一次构造的元数据成员索引及重启期额度覆盖。内容读取、批量操作健康检查、磁盘观察和脱离引擎的规划直接持有该类型；捕获配置只增加一次引用计数，不再为每份内容读计划分配外层 `Arc<ValidatedConfig>`。根路径和额度覆盖仍在公开边界校验，以 copy-on-write 保留旧计划的 roots/limits；共享时修改会复制配置和成员索引，普通读取不会。计划可独立于 `Engine` 存活，但实际读取仍检查当前磁盘 seal、发布证明和正文完整性；元数据探测不扫描历史。这里说明的是类型与调用图确定的分配消除，不是吞吐或延迟实测。

减少运行额度只限制新增准入，已有占用超限时显示超额并停止增长，不删除工作数据。修改告警值不改变决定状态；修改份数、成员或绑定也不能通过热更新生效。动态配置验证失败保留上一份有效运行配置，持久业务状态不变。

[配置约束附件](contracts/config-constraints.schema.json)校验最关键的协议断言、资源上限与告警参数；它是完整 TOML 的受检投影，不覆盖全部设备和业务状态。该投影不能替代当前 JSON 配置的核心校验、拓扑交叉校验或实际启动检查，不能把投影校验当作部署成功。


### HTTP 暂存的可执行配置入口

当前直接运行的 `zxs` 接受下列独立参数（十进制字节数）：

- `--http-spool-bytes-per-account`：默认 `107374182400`，即 100 GiB。
- `--http-spool-bytes-total`：默认 `536870912000`，即 500 GiB。
- `--http-spool-recovery-reserve-bytes`：默认 `1073741824`，即 1 GiB。

它们映射到 `ServerConfig.http_spool_limits`，只约束 HTTP `api-spool`。`--max-upload-bytes` 仍限制单次上传；核心工作区及 SMB 的预算独立。上文完整设计配置中的跨入口全池参数不能仅因存在于示例中就视为全部已接入运行时。启动会盘点留存数据并持有独占目录锁；不得让多个服务实例同时使用同一 spool。详见[HTTP 暂存预算与恢复](components/server.md#http-spool-budget)。

HTTP 传输准入及期限参数（上传/读取各自独立一池）：

- `--http-transfer-slots`：默认 `2`，必须大于零；唯一管理员的全部会话共享各池额度。
- `--http-transfer-idle-seconds`：默认 `60`，无非空正文进度的期限。
- `--http-transfer-total-seconds`：默认 `3600`，必须不低于 idle，且最多 `604800`（7 天）。

映射到 `ServerConfig.http_transfer_limits`；旧 `--http-transfer-slots-total` 和 `--http-transfer-slots-per-account` 参数明确拒绝，不保留别名。额度耗尽立即 429；响应头前超时为 408 `TRANSFER_TIMEOUT`，已开始的正文以流错误终止。超时不删除留存数据，也不提前归还仍被阻塞 I/O 使用的许可/租约。详见[HTTP 并发准入与传输期限](components/server.md#http-transfer-limits)。

## 当前配置应用入口

`zxs-rescue configuration` 只生成或编辑未初始化草稿，不直接应用配置或在线登记磁盘。部署者可将受保护的本机 Config 通过 `zxs --setup-config ID=PATH` 登记为预置方案，并为每个盘登记 `--setup-disk`。Web 只提交方案 ID，授权预览后按配置摘要确认初始化；提交时重读并核对摘要，文件变化不能静默替换已审查方案。缺失或损坏的预置文件不阻止 HTTP 启动，不能通过浏览器上传任意 Config 或服务器路径。详见[受保护初始化](management/setup-recovery.md)。

---

[文档目录](README.md) · [上一章：工程结构与依赖约束](20-engineering.md) · [下一章：验收标准与证明材料](22-acceptance.md)
