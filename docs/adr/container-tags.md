# 文件容器标签：直接采用新模型

状态：用户已明确确认容器标签模型及整个旧池、旧备份不兼容的具体影响。本批实现合同进入生产代码集成；完整准确提交的 GitHub 定向编译和行为验证仍待完成，不能以静态审阅作为通过证明。

最初设计核查基线：`1558d2f3c58ddda0c75a1aa86e4b6b1b9c51284c`；本批最终集成基线：`39bcae218116b2d8f925120d9a8f34cea791df5a`，保留其备份诊断测试与原 900 秒限制。本轮未运行本地编译、测试、格式化或验证脚本。

## 1. 决定与非目标

用户明确要求标签只属于文件容器，不属于节点或分支，并明确不要旧模型兼容。因此本次直接替换模型，不从旧节点选择、合并或迁移标签，不提供旧标签写入口，也不保留空字段占位。

拒绝旧格式不等于删除旧数据。程序必须保留原有池、日志、内容、历史图、永久收据和备份字节，不自动重写旧格式标记、重算旧摘要、重置池或清空标签。需要旧格式才能解释的输入明确失败，不能报告成空标签或成功空历史。

本次不完成全部操作时间线，不改同正文合并的内容节点规则，不开发非容器文件或非容器目录功能。已经接入的删除/恢复事实读取不因此变成完整操作时间线。

## 2. 权威数据

新增必需的 `State.container_tags`，键为类型化 `FileId`，值为：

```text
ContainerTags {
    tags: TagSet,
    generation: Generation,
}
```

每个仍由保留材料引用的文件身份恰有一份记录，包括已软删除的容器。记录不能缺失，不能指向不存在的文件身份；空标签也显式存储。文件归属仍由已准入的历史域和所有者索引证明，标签表不成为绕过归属的第二套身份来源。

`Tag` 是私有字段类型。值必须非空、无首尾空白、无控制字符，最多 128 个 UTF-8 字节；不自动 trim、不做大小写或 Unicode 归一化。`TagSet` 最多 16 项，输入重复项拒绝，不用去重掩盖错误或超过原始数量限制。规范编码按标签值排序。读写边界均调用同一类型构造器，域内不反复解析裸字符串。

新建容器为显式空集合、`Generation::new(0)`。标签实际改变时仅通过 `Generation::next()` 前进一代；耗尽明确拒绝。无变化请求保留原代数，仍产生可重放的操作收据。正文版本、节点、分支和路径变化不改变此记录。

## 3. 写契约

操作种类为 `file.container-tags.replace`，与旧 `file.tags.update` 分离。请求使用完整集合替换，不再接受节点、分支、add/remove 或 expected HEAD。

```text
ReplaceCommand {
    file_id: FileId,
    context: Context { repository_id: RepositoryId, path: PortablePath },
    expected_generation: Generation,
    tags: TagSet,
}
```

HTTP GET/POST 保持 `/api/file-manager/files/{file}/tags` 路径。GET query 必须给出 `repository_id` 和 `path`；POST body 给出这两个字段、`expected_generation`、完整 `tags` 和 `operation_id`。服务端将路径 FileId、body Context 和单独操作号组合成上述领域请求。服务器提供经过认证的 `Principal`，不接受客户端提供主体。本片要求真实、当前且可写的 repository/path 上下文；不以缺少 location 暗示脱离当前命名空间的写权限。已删除容器保留标签，但本片不增加无位置标签编辑入口。

操作结果为 `Replacement { request: ReplaceCommand, tags: TagSet, generation: Generation, changed: bool }`，包含完整原请求而非仅 FileId。直接提交、操作查询及 `/outcome` 使用同一投影；事务框架的 timestamp、membership epoch、version protection 仍留在原永久收据，不作为公开 Replacement 的扩展字段。返回结果不含新节点、推进分支或目录分支列表。

顺序与不变量：

1. 既有 permanent receipt 的 operation/principal/请求摘要精确重放先于新路径和 CAS 判断。同操作号不同请求仍拒绝，成功后路径改变不妨碍查询原结果。新种类收据保存可恢复的完整原请求证明，包括 file、repository、path、expected generation 和标签集合，并绑定主体、操作种类、操作号及摘要；不能从最终标签反推请求。不同路径或 expected generation 即使得到相同结果，也不是同一请求。
2. 新请求在提交线性化点验证当前文件归属、写上下文和 `expected_generation`。CAS 冲突返回既有 `HEAD_CONFLICT`；当前上下文不再对应此容器返回 `FILE_NOT_FOUND`，不能自动覆盖新值。
3. 普通写链和 Current 写链使用同一纯标签计划。注册表存在但 Current 尚未发布的围栏保留；不允许 prepared payload。
4. 标签写允许改变目标容器标签记录、对应操作收据、主体、事件及既有操作序号/业务代数。精确拒绝其他容器标签、正文、HEAD、commit、namespace tree、fmeta、placement 和 payload 的连带变化。
5. 真实标签改变不创建内容节点或标签元数据子节点。每次成功新操作仍按现有事务规则产生一个操作事件和永久收据；`changed=false` 表示替换请求完成，不是标签改变事实，标签代数不变。全局业务代数、操作序号和标签代数不可混用。
6. 冷回放校验与实时写使用同一变化证明，不读取时钟或随机源重新决定标签。

请求摘要及永久收据编码留在编码边界；新业务计划、投影和回放使用类型化请求及结果，不把 `Value` 带入领域逻辑。旧操作结果不被新种类解释。

## 4. 读取与实际界面

现有文件详情提供容器级读取和编辑。读请求绑定文件身份与当前 repository/path，返回可用于 CAS 的同一权威记录。HTTP 路径和 body 分别由 typed `ApiPath`、`ApiJson` 及认证提取器准入，旧字段明确拒绝。

不同分支、历史节点、版本选择均展示这一个容器的当前标签。删除节点上的标签 chips、逐节点编辑按钮、标签专用勾选目标和分支选择；与真实 merge 审阅共用的基础行为不能连带删除。Merge 不继承、选择或接受标签参数。

浏览器捕获 session epoch、导航目标、完整文件上下文及本次读/编辑实例。切换目标、关闭或注销取消读取并隔离晚响应。响应验证 `file_id` 和请求上下文。CAS 失败保留用户草稿并要求重新读取，不把服务器新值自动并入。

结果未知沿既有 operation-id 查询流程解决，持续保存原操作号和完整原请求，不把后续草稿、新读取的 generation 或新路径写入原请求。查询仍受当前会话授权。查不到收据不等于确认未执行；只有既有恢复协议确认可重试后才重发完全相同的请求，不生成新操作号盲重试。

标签读取不依赖历史节点分页，也不为读标签扫描全部历史。界面及结果不声称完成全操作时间线。

## 5. 创建、复制、移动与保留

- 新文件身份：在权威创建路径同时建立空标签记录，不在读取缺失记录时补写。
- 同身份改名、移动、切换版本、分支操作、软删除和恢复：保持集合和代数。
- 独立复制、导入或其他真正生成新 FileId 的流程：复制源已发布集合，新身份从 generation 0 开始；之后双方标签独立。复制计划捕获来源标签及代数，与复制源快照/基线共同验证，发布前来源变化遵循既有复制线性化规则；不得捕获图后另读任意时刻的最新标签。
- native 备份或同身份精确副本：保留集合和代数原值，不能重置 CAS 域。
- 保留历史、删除 freeze 和恢复材料覆盖标签记录。没有已发布来源不能从任意历史节点推断值。

核查消费者包括普通历史复制、object copy、foreign import identity closure、maintenance 身份映射及 file deletion freeze。新模型必须覆盖全部真实入口，不能只改最常见的上传路径。

## 6. 持久格式与拒绝边界

以下内容必须同批闭合，不能发布一个新字段却依靠缺省继续读取旧材料：

- `State`、tracking、typed dirty keys、owner/coverage 索引和 publication shape。
- snapshot radix roots、history manifest/graph、State admission 和历史恢复合并。
- FILE 的文件级行承载标签/代数；移除 version、anchor、metadata 的节点标签编码。
- scoped/native backup、FILE rescue、foreign import、identity remap 和冷恢复。
- 删除 `FileVersion.tags`、`FileMetadataNode.tags`、Tags metadata kind、节点标签读取以及 Merge 的标签参数/继承代码。Merge metadata 的来源证明仍有消费者，必须按职责迁出，不能整删旧标签模块而失去 merge 准入。

snapshot 与 history root 采用直接描述容器标签语义的格式标记，FILE 使用对应的新语义标记，不用发行版本号命名。旧标记不保留可执行兼容分支。

同时替换现有 active `DiskSealFormat` 的 Genesis/Generation 语义标记，不保留旧反序列化变体。该决定不是仅拒绝含标签的旧节点：整个旧池的服务开启、offline check/export、旧备份恢复也会明确拒绝，包括原本没有标签的池。程序不提供自动迁移，也不重写旧 seal/ID/hash。用户在被明确告知“新版本无法打开旧格式的整个数据池和旧备份，旧数据不会被删除或自动修改”后，再次确认不保留一切兼容；本批按该具体范围切换，不推定其授权删除、覆盖或迁移旧数据。

本批标记如下，标记必须与内部权威模型同时发布：

| 材料 | 新语义标记 |
| --- | --- |
| Genesis seal | `container-tags-managed-directory` |
| Generation seal | `container-tags-membership-generation` |
| Pending seal | `container-tags-membership-pending` |
| 普通 serving archive root/binding | `container-tags-frozen-control-metadata` |
| retained-exit serving archive root/binding | `container-tags-retained-exit-quorum-control-metadata` |
| preparation archive | `zxs-container-tags-backup-native-control` |
| snapshot/history root | `container-tags-radix-map-roots` |
| FILE rows | `file-local-container-tags` |

SYSTEM 增加 catalog family 39；既有永久审计主体、事件、状态的 36–38 编号保持不动。

同批格式闭包还包括 `membership_transition/pending.rs::PendingFormat`、`membership_transition/archive/persistent.rs` 中用于 `ArchiveRoot` 与 `ArchiveBinding` 的两种 Format，以及 `backup_preparation/native_control/archive.rs::FORMAT`。后两者是不同的 serving archive 与 preparation archive，不能只改其中一个。同步更新格式分派比较，但不因 `membership-pending` 同名就改变磁盘目录路径。

启动拒绝必须在 authoritative recovery 写入前生效。只修改 snapshot marker 不足以证明此点：现有 journal preflight 在 pending/barrier failure 会早停，而恢复可继续；完整 State 重解码会扩大成本和改变合法恢复行为。因此禁止为本片直接添加“扫描所有历史并重建所有 State”的补丁。

静态调用链核查显示，普通启动的 `DiskResources::open → DiskHandle::open → decode_seal` 在 `prepare_recovery_resources`、引擎锁与 journal writer 之前。非空旧 seal 不能被当作离线空盘；Generation discovery 的 rejected-member 记录也不直接为其自动覆盖授予权限。这不是所有旁路的运行证明。

实施前须进一步核各启动、激活、导入和恢复入口，并证明混合旧/新成员不能导致旧材料自动重写。若证明不成立，格式切换不得发布。系统 `BootstrapStore::hold` 在较早阶段可能创建或同步 control lock；此例外只说明该控制锁本身，不泛化授权其他元数据写入或目录 fsync，也不宣称整个目录绝对零变化。

导入和备份的源 root、FILE、archive 仍各自准入；新外壳不能为旧内层材料授予写权限。原始证据及永久收据在拒绝后保持原字节。禁止重编码旧 rescue rows 然后沿用原 hash。

### 6.1 入口与首写边界清单

下表是静态定位，不是已执行验收。路径均相对于 `src/engine/src`；本批已按这些路径接线，新增拒绝测试与合法恢复回归仍须在 GitHub 对准确完整提交执行。

| 入口 | 已定位的源准入 | 首写边界与仍需闭合的事项 |
| --- | --- | --- |
| 普通 Genesis 启动 | `engine/membership_transition` discovery 的 `marker_path_at → decode_seal`；随后 `disk::DiskResources::open → DiskHandle::open` | 旧 active seal 阻止普通资源进入 recovery；控制锁例外如上。测试旧池及健康多数加一个旧 seal，源 seal/对象/日志不变 |
| Generation 启动 | `discover_root_authority → verify_held_generation_from → predecessor_seal_matches`，最终资源再次准入 seal | discovery 的单成员错误记录不能成为覆盖权限；测试混合成员、合法 predecessor、排除根不被访问或覆盖 |
| `membership-pending` | `admission::inspect_new_root` 解码并比对完整 PendingDiskSeal，启动/晋升另经 marker、predecessor 与 activation namespace 判断 | active seal 门禁不能覆盖此路。新 PendingFormat 不得把旧非空根视为未初始化；无 seal 的旧 staging 还要由 ArchiveBinding 准入拒绝 |
| native archive | `engine/backup_preparation/native_control/archive.rs::read_observed → verify_observed → original_journals`，随后 snapshot 准入 | typed archived seal 和新 snapshot 都必须准入后才能安装目标材料；外壳新、内层旧仍拒绝 |
| native serving restore/resume | `prepare_resume → authenticate → native::read → authority::read_archive_base → persistent::read_generation_base` 解码实际 ArchiveRoot、State、candidate、seal | 这些步骤先于 `PreparedBackupResume::into_completed → saved.complete()`；打开已有 writer lock 不创建缺失锁。新 archive/State 格式仍须落实并验证 completed/unfinished 两条路 |
| history package import | `restore_history_package → load_package → graph::resolve_with_objects → manifest.validate` | 源读取先于业务 mutate/内容安装；不依赖 seal，必须独立拒绝旧 history marker、缺标签 map 和节点标签字段 |
| foreign import | `ForeignSource::inspect → foreign_import/inspect.rs::decode_seal → read_local_disk_configuration`，另有 identity closure | inspect 只读不是 commit 准入证明。发布前 source revalidate、manifest projection 和新标签映射须同批闭合 |
| FILE rescue | `inspect_object_database_material → FileScope::read → file_database::read_material`；重建经 `validate_records → SCHEMA.encode` | 此 API 只生成 SQLite bytes，不安装全局 State；输出文件调用方仍是独立边界。旧 rows 不得通过新 wrapper 重编码并沿用旧 hash |
| maintenance | 已打开 Engine 的 `create_split_plan/commit_plan` 及 `maintenance.rs` 身份映射 | 计划持久化和安装前须有新标签记录完整证明；不能因启动已准入就漏掉新身份的独立标签 |
| offline check/export | `offline_check_in_context` 先 decode seal；export 另验版本、local publication、payload | export 经 `verified_full_copy` 写目的地。旧池断代也拒绝旧正文导出，测试源不变、目的地未创建/覆盖 |

`ArchiveStore::admit` 对非空 staging 先解码/比对 `ArchiveBinding`；`install_generation_archive` 先认证/解码 ArchiveRoot 后安装。两者必须使用同一新语义，拒绝无 seal 的旧 staging，不能绕过旧 pending 拒绝后从 archive 继续写。

入口缺口不能以普通 startup 已通过替代。尤其 pending、backup resume、foreign commit 和 rescue 输出调用方必须在格式切换验收前给出新模型的具体源码闭包及 GitHub 行为证据。pending 旧 seal、新 seal 加旧 archive、无 seal 加旧 staging、completed/unfinished resume、两种 archive 均需独立拒绝测试，同时保留新格式中断恢复成功用例。提交完整候选以运行定向 CI，不表示上述验收已完成。

## 7. 实施与验收

实现前先完成第 6 节门禁审阅，按合同分工；main 只接收消费者完整、可构建的提交。本批原规则并集已超过单 job 的 16 个 Cargo 配置上限，因此先交付同一准确提交的定向分片。默认单片选择仍为 16 个 Cargo 配置和 8 个 Python 模块；每片保留原目标、前缀、数量和 900 秒单测试上限，完整选择不漏不重，最终汇总验证所有分片的提交、基线和计划身份。此配套不启用全仓矩阵，不删原组规避限制。

必须实际执行并记录：

1. typed tags 的空值、空白、控制字符、UTF-8 字节、16 项、重复项和 generation 耗尽边界。
2. 普通与 Current 真实读写、CAS、无变化、撤权、精确重放及不同请求复用操作号拒绝。
3. 写前后正文/HEAD/commit/tree/fmeta 不变；禁止额外写集的实时与冷回放测试。
4. 版本切换、移动、删除恢复不改变标签；独立复制后修改互不影响；新建空标签。
5. snapshot、history package、FILE、native backup、cold reopen 的真实往返。
6. 旧格式、缺记录、悬空键及错误来源拒绝；失败后权威文件和日志原字节不变。
7. 真实 HTTP 路由和 Web native/WASM 消费者；编辑关闭、导航、注销、晚响应、CAS 草稿和未知结果行为。

仅允许 GitHub 定向编译/测试。本地只读写代码。准确区分待验、失败、通过与未执行，不能以静态审阅或新测试名字替代结果。

### 7.1 当前证据边界

- CI 分片基础已在提交 `6803ce8cd8dd6c9ff1bb8a2db97971fd92360c79` 的
  [#1293](https://github.com/isarmg/zxs/actions/runs/43212237593) 实际通过：7 个 Python
  模块共 132 项，0 Rust；同一提交、基线、policy/plan 摘要与唯一分片的覆盖收据一致。
  这证明该基础片，不是容器标签生产行为或真实多分片 Rust 构建已通过。
- 本批新增旧 seal、混合成员、pending/两类 archive 拒绝测试，比较受控临时根中的
  全部目录集合与文件字节。它们不覆盖权限或 mtime，pending 也不宣称完整错误码矩阵。
  native preparation 正例是 history/native 准备后冷重开续作，不代替完整池激活恢复。
- 非空标签复制回归覆盖真实 foreign prepared graph 的新 FileId、generation 0 与捕获
  后源变化隔离，未安装该目标，不能声称安装后双向写入已验证。maintenance split
  回归另用真实 API 覆盖两个独立容器的写入及冷开；这些新增测试尚待本批 GitHub 执行。
- browser-smoke 消费者已经改用真实 GET/CAS 和容器编辑器。定向组执行 Node 观察器
  夹具并编译 Rust 驱动，不运行 Chrome；合成历史分页仍明确为合成输入。
- 新格式真实 HTTP 捕获已从 #1300 准确提交的受控 wire 工件逐字取回，作为独立的
  历史、标签 Snapshot 和 Operation 正向 fixture；对应 HTTP 单项通过，离线契约消费
  尚待后继执行。原工件、摘要与边界见[契约证据](../validation/container-tags-contracts.md)。
  旧真实节点标签捕获保留原字节作拒绝证据，不能手改它冒充新服务响应。

### 7.2 首次完整定向执行与修复待验

[#1300](https://github.com/isarmg/zxs/actions/runs/43219203395) 的准确 head 为
`f3dd9403ccaf62302a93b8ae13846f1072733f33`，比较基线为
`39bcae218116b2d8f925120d9a8f34cea791df5a`。计划选择 34 个 Cargo 构建配置、
13 个 Python 模块，三个分片全部返回工件。#1295 与 #1297 在选择阶段失败，
没有产品验证；#1296 是配套 CI 小范围运行，不作为本轮产品证据。

| 分片 | 实际通过 | 失败及未执行 |
| --- | --- | --- |
| 0，工件 13947791171 | 292 Rust、76 Python、2 编译检查 | 五项 server 行为、一个 Python 断言、Engine unit 编译及原备份 900 秒超时；超时后十个配置 blocked |
| 1，工件 13944983138 | 185 Rust、106 Python、5 编译检查 | 两项 Engine 冷开行为与 SMB 集成编译失败 |
| 2，工件 13945945489 | 30 Rust | 本片通过，不替代其他片 |

合计 507 Rust、182 Python、7 编译检查通过，但整批失败。真实标签 HTTP 成功不能
掩盖两处冷恢复问题：首次 Current registry 发布在冷回放前缺少对应已发布证明，以及
重复已完成 FILE 安装尾被误判为多个未完成意图。本修复批分别恢复精确 publication/
成员 D3/基线证明，以及重持有、复验并完成既有安装尾；原拒绝边界与永久材料保留。

五项 server 失败和 SMB 编译失败由旧节点标签夹具触发，改用当前 GET/CAS 请求。
node-note 的元数据节点改由真实历史合并创建，不再要求标签写入创建节点。主机分页
断言跟随独立容器草稿流程，保留分页、merge、字段与迟到响应覆盖。

#1301 的 `3c534060ebe507e8a7419d2cf8ef3663ad579f3f` 工件 13947621528
进一步暴露九条 Engine unit 编译诊断：缺失路径 oracle、缺失 BTreeMap 导入和两个
State 穷尽分类遗漏。它们不是测试通过；恢复独立 oracle 与标签字段分类，不用通配
忽略新字段。相关修复及新增回归均待后继准确 SHA 执行，本地未编译或测试。

Web 原生/WASM、browser Rust、contract-check、FILE history copy 和 phase1_save_tags
在 #1300 因备份超时后的子进程清理无法证明而 blocked，不能称已编译或通过。
本修复批只把原备份的完整 Cargo 身份安排到独立分片，不删除其原选择或放宽 900 秒。
后继仍以同一完整基线重跑全并集，任一失败、超时、缺失或 blocked 都使总验收失败。
真实 Chrome、全池激活恢复与未选择目标仍不在本次证据内。
