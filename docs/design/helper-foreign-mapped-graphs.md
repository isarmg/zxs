# 外来链的来源域历史图转换

本片续接[完整对象命名材料](helper-foreign-object-identities.md)，把其中的对象名实际应用到 typed 历史图。**它仍不是正式接纳**：得到的是独立、来源域明确的图草稿，不是现池 State、HistoryPackage、目标 SQLite、Publication 或任何可开启 writer 的凭据。

本片只有 Engine 生产准备 API；**尚未接 helper 同批协调器、离线线协议或 Web 按钮**。既有 Web 的对象命名“完成”仍仅表示原命名阶段，不会暗中调用新的写入阶段或声称图草稿已经保存。

## 类型化转换与不转换的来源域

每个目标从自己持有的原 native 前缀推导同一完整命名表。保存图前，自己的原三范围数据库及准确命名材料必须已完成并独立逐字节验证；不能拿任一同伴报告、用户 JSON 或自报哈希作来源。图与命名表的完整 report 必须一致，仍绑定现池全 State/sequence/Config、新完整实物链、原操作和来源前缀。

转换逐个类型处理 `HistoryManifest` 的全部字段，并穷尽解构各记录的字段：

- 仓库、历史、提交/tree、namespace 项、文件/版本、文件及目录分支、目录树的键与局部引用。
- 文件路径/元数据/删除节点、父边、合并、别名、pending/inherited、按路径上下文和选择代数所属对象。
- 目录 Git 的工作区、index、已提交 HEAD、子仓库固定树引用和边界目录键；保留原先父子提交顺序，不把全部序号压成一个新序号。
- 历史边界的**本地端**、放置规则/保护策略的对象身份、保留策略绑定、备注/位置/备份路径等投影键。全局上下文仅改仓库键前缀，真实路径字符串不递归改写。

目录身份明确区分根 repository 与子目录 entry；混合文件节点只允许命名表中的 version/path/metadata/deletion 域。缺少准确映射直接拒绝，没有保留原 ID 的活动引用降级。映射的单射性及与新旧对象不冲突复用上一阶段的构造保证，不另生成随机 ID 或在碰撞后重试。

以下字段**保留原字节语义，并在归档 `preserved_domains` 明确归属原池**：历史 operation/author/sequence，原 slot、binding/topology generation、SMB 绑定数据，路径/storage_name/备份前缀，以及 provenance 和跨历史边界外部端。它们不被解释成新池同名账户、现池幂等成功回执、目标 placement 或现池序号。不可借新对象名把原 `smb_enabled` 等元数据激活；本 API 不返回 `State`/`AdmittedState`，也不导入源认证库或修改永久收据。

转换后的完整 manifest 调用既有纯逻辑闭包验证。它只证明图的局部结构约束，不能证明新路径可用、共同 authority 或真实 FULL 就绪。归档保留每个原版本的 availability、原 publication 哈希、原主盘 group 与 manifest 哈希；缺失正文依旧是 `temporarily_unavailable`，不转成 available 或 tombstone。数量相同或图“Verified”都不构成正文可用性结论。

## 准备与准确续作

公开入口是 `ForeignMigrationTransaction::preview_history_graph` 与 `prepare_history_graph`，使用不可反序列化的 `ForeignIdentityTarget`。调用者必须继续持有现池排他/认证 owner，并在解锁来源扫描后及写入前后重新核同一现池基线。它没有扩大既有固定 genesis/32 原盘/2,048 发布的支持边界。

每个已完成 capsule 的原真实低级父根保存三个独立文件：

```text
.zxs-foreign-graphs-<operation>-intent.json
.zxs-foreign-graphs-<operation>.json
.zxs-foreign-graphs-<operation>-complete.json
```

reservation 绑定实际父 device/inode、该 capsule 摘要和完整图 report；先 intent，再原子不可覆盖图文件，最后 complete，沿用已有文件和父目录同步。查询始终从本目标原生证据重建期望，按准确预期长度读取并完整比较字节，不信任 marker 的摘要自述。completed 缺 intent/图时拒绝补造；篡改字节保留拒绝；只有最终 complete 中断的窗口允许确认准确原字节后补最后记录。不会覆盖原 capsule、原库、命名文件或源高级盘。

当前 API 是**单个真实低级根的准备原语**。在接入 helper 之前，后继仍须先核整批各层期望完全一致、建立 root 原请求/完成记录、逐根重新授权/基线检查、处理整批完成后的缺件拒绝，不能仅循环调用本 API 就宣布整批接纳。

## 预算与未完成边界

来源 manifest 在复制前通过有界计数 writer 限制为每历史 8 MiB，整批图与最终信封上限 64 MiB；命名仍受 100,000 项/16 MiB 限制。图记录按需克隆，原不可变提交/目录树仍保持原对象不变；输出只编码一次并同时取用途隔离摘要。原 native/三范围重验、图转换及逻辑闭包有全量 CPU/内存成本，64 MiB 是归档限额而非进程 RSS 上限；没有性能或最大规模验收结论，也未减少 fsync。

后继已暂停额外 helper 图准备按钮，改为直接接[共同决定与冷态联合 State](helper-foreign-joint-protocol.md)，保留本片 typed 图作为转换实现而不是要求操作员多走一步。该后继仍未实现真实 FULL/目标三范围安装、原根接入和最终活动切换。现有低级链只有完整实物与准备材料，不能提前启用任一层；`foreign_admitted`/`complete_installation_ready` 不变。

## 实际验证状态

本地候选父 `24f0c31b`。新增四项私有图转换模型与四项临时目录集成用例，覆盖可逆映射、审计原域、目录 Git 固定引用、预算、不完整前提、各层一致/缺正文、完成后缺件及中断/篡改；**只有测试源码，未执行**。仅格式化明确受影响 Rust 文件并检查补丁空白，没有 Cargo、编译、测试构建、本地测试、GitHub CI、真实盘/服务操作或浏览器验收。

先前最后一次生产 check 未通过；旧 Operation 枚举缺失与共享缓存的关联仍为未确认推测，本片不宣称修复或验证该原因。当前组合未验，原候选完整保留，等待唯一合并槽。
