# 终态移除的命名空间证明闭包

[任务清单](../../TASKS.md) · [逐笔管理链](terminal-retirement-chain.md) · [元数据恢复](../architecture/terminal-metadata-resume.md)

基于真实 `d3660993` 开发并对齐真实父 `16790208`。本片交付每笔终态移除的完整 occurrence mask、Detached 登记分区及未激活命名空间投影，复用原 helper 执行/原结果路径。没有移除 metadata readmission/resume 门禁，没有签发目标 writer 或服务启动许可。未审查、编译、测试、浏览器验证或运行/查询 CI，未运行 helper 或操作任何真实系统。

## 为什么管理登记还不够

仅从 registrations 移除一行不能证明历史 HEAD 的哪些条目仍可见，也不能把旧 Config 中的同一盘重新解释为已登记。新 successor 必须同时保存准确的 `DetachedBinding` 和原操作绑定的 occurrence mask；历史拓扑、不可变 Commit/DirectoryTree、全部操作回执、曾用文件/条目身份及不可用版本仍保留。

每笔 `retirement-successor.json` 使用必需格式 `terminal-retirement-masked-registry`，包含 registry commitment、必需 occurrence-mask hash，以及两者的完整分页对象。原 intent/digest、Stop 计划和 completion 通过 successor 摘要共同绑定这些字节。没有缺省 mask、双读或旧 successor 隐式升级；旧或缺字段材料拒绝并保留，不重置管理池。

## 同一冻结来源，逐笔投影

原 `VerifiedTerminalAuthority` 在重放归档和验证最后 voter 的完整原 mask 时，保留 `PreparedCurrentNamespaces`。这是已认证但**未激活**的准备态，不是 `Engine.current_namespaces` 或普通读写视图。

下一笔从该准备态和准确 registry 捕获来源：完整 registry 状态/commitment、原 snapshot、原业务 sequence/generation 和命名空间 HEAD 必须一致，voter 集合必须为空且原终态 intent 仍在。管理链新增的永久回执 sequence 不冒充原业务快照。来源捕获不打开数据盘，也不从未过滤历史 HEAD 重建前笔已删的条目。

移除目标仍须是当前登记的准确 pool/disk/slot/绑定代数。mask 使用既有完整 DeregistrationPreview 语义枚举分支、目录分支、历史 Commit 和 DirectoryTree，保存 owner、准确 HEAD/全部条目摘要、路径及 entry/file/version 身份；读回重新比较完整语义。重算一个删掉页/条目的哈希不能使遗漏有效。

registry successor 只新增该目标的 DetachedBinding，绑定前驱、视图代数、原主体/operation/digest 和 mask，保留最初 terminal pending-exit，不伪造新的活跃退出决定。完成恢复还会核确切原 Stop、实际 I/O 排除及永久结果，然后安装准备态用于下一笔。历史 frame 不长期保留每一份完整 namespace 图，只持有原结果和排除 guard。

## 保留与不可复活

每笔从前笔已验证 entries 继续过滤，既有 hidden paths、曾用 file/entry 身份、不可用版本及递归策略索引按原 consumer 语义保留。某版本只要仍有已登记持有者就不因失去一个备份盘而被误删；最后持有者移除后才进入不可用集合。同名路径、同文件不同版本与目录组合不合并为一个 file ID。原不可变历史字节不改写。

源 SYSTEM 缺页、错哈希、前驱不一致、缺旧闭包或旧格式都失败并保留证据。正常同原操作的已持久阶段依旧通过原 fsync 恢复；不能借正常断电恢复自动修复任意损坏的证明。

## helper 和尚未接通的 readmission

独立 Web 仍使用原终态预览、提交、原摘要查询/续作。预览说明本片需要 mask，不声称预览已经完成 mask；永久结果只有核心重新验证后才含 `namespace_projection_verified: true` 和准确 `occurrence_mask_hash`。`active_namespace_admitted` 与 `metadata_resumption_supported` 仍为 false。

恢复活动存储还需要新的完整 resume authority 把整条管理链、管理期永久回执、原业务基线、新 voter 决定/实际 Publication、当前配置与 serving locator 一同纳入原生可移植证明，并在冷重放和 I/O 新 epoch 路径消费。当前普通 ResumeCore 仍只理解最初 terminal authority，故现有早期门禁保持。不能仅移除 `require_resumable`、把当前准备态传给 Engine，或把已注销绑定当 Disabled readmission。

外来二级及以上盘的 H03 仍需要一次性全部低级链正式接纳；备份 Publication 材料仍无可安全激活的目标 writer/最终接纳 API。本片不替代它，不清源围栏，不新增 PlannedOffline，不操作 systemd/SMB 或凭据。

## 资源和证据

完整 mask 与 registry 编码在保存新 intent 前执行上限检查；整体 successor 仍受现有单元数据对象 `MAX_METADATA` 上限约束。大型集合超过该界限时明确拒绝，不丢页或放宽限制。生成/语义比较扫描冻结 scope 与版本，冷恢复按全部 frame 重建准备态，未作性能或容量验收声明。初始准备态、当前前驱和当前 successor 使用已有 Arc 表结构；不存在每个历史 frame 永久保留一份全图的新增成本。

新增测试源码包括连续备份持有者移除、不复活原已隐藏路径、保留不可变历史与曾用身份、错前驱投影、重哈希遗漏、冷态 mask 页缺失和旧 unmasked successor 拒绝。全部未运行，不能作为通过证据。
