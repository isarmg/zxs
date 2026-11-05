# 当前登记下的历史分支归档

## 范围与验证状态

基线 `db110ff9`，2026-11-05 复用旧候选并补齐准入边界。本批包含真实 `archive_branch` 消费者、专用 metadata-only planner、私有 current helper、根 `test/` 测试源码与本文。普通 `require_operation` 仍拒绝 `branch.archive`；只有真实原生请求通过专用 planner 的完整证明后，候选才可进入公共发布路径。

已执行固定 Rust 1.99.0 的定向 rustfmt 与 `git diff --check`。没有执行构建、测试、产品运行、模型、基准、CI、提交或远端推送，没有真实部署。本文的测试向量均待统一执行，不是通过记录。

不改变 history fork、rename、merge、生命周期/固定成员 N/Q、工作区准入、认证入口、正文存储或源账本。

## 实际变更与证明

原 `archive_branch` 先核对仓库主分支、全局文件选择及路径 context，再拒绝仍有 Open/Sealed workspace 的 branch。原 actor 必须非空；服务器已有认证/对象授权契约不变。已有 operation 仍先按原 actor 和请求摘要返回永久结果，不重新解释原回执或裁剪结果。空 actor、main、选择中分支和 busy workspace 的原错误保持。

真实业务只将目标 `Branch.state` 设为 Archived，并将 generation 增一。不同 operation 重复归档仍照旧增代；同一 operation 的永久重试返回原 generation。`pending_file_versions` 不清空：它仍支持归档历史的继承正文读取与容器派生描述。本批没有新增任何 pending 清理。

当前登记下，真实消费者追加 `namespace_archive` 来源记录，绑定完整 registry commitment、本次 activation sequence、mask、仓库、目标 branch、原 head、原 generation 与原状态。它只使用已准入 scope 身份，不要求该 scope 获得正文可写路由。Disabled/Detached owner 的历史 metadata 可归档；scope 的 writable、entries、hidden paths、历史 mask 和所有旧 workspace fencing 不因此改变。

专用 planner 拒绝任何 `Prepared` 正文写入，核对它返回的完整永久回执与候选 State 内的新回执一致。公共 publication 以及 `NamespaceReplay`、`install_recovered_namespace_state` 的冷恢复继续调用 `advance`，在任何不变 namespace 快速返回之前独立重建同一证明。活动日志的 `recover_prepared` 还在 candidate-bound snapshot/history 已读验后、该候选的 payload/metadata replica 修复和 names/publication completion 之前调用同一纯 `advance`；不提前安装 State/view，完成后仍保留原安装验证。该共用挂点同时覆盖 plan.cancel，不另写一份 archive 专用调用：

- 按 transaction-local operation 变更键发现归档。恢复失去 tracking 时遍历持久操作，不分配一份全量操作键数组。已有归档回执不得重写或移除；其独立 actor 写集和归档 event 也单独核对，不能通过保持 operations map 或 HEAD 不变绕过。
- 核对准确 operation ID、原 principal、原 `branch.archive` 请求摘要、Published/Healthy/NotApplicable 回执、发布序号及完整来源记录。结果字段必须恰为原生返回值和标准 envelope，timestamp/membership_epoch 使用规范无符号十进制，不能附加未知字段或正文保护项。branch、head、fork、history 与 repository 的键及历史归属一致；原 head 仍对应已准入 scope。原 main 和选中分支规则也从前态重验。Open/Sealed workspace 规则留在真实实时消费者，因为 workspace 不属于候选 State；恢复完成既有决定，不把冷启动时的 workspace 当作新授权。
- 穷尽比较目标 Branch：只允许 state=Archived 和 generation+1，名称、scope、head/fork、种类、file/node identity 与 pending map 全部保留。
- 保留真实历史校验的 alias 条件：merge 消耗名必须保持 Merged，最终 owner 必须保持 Active。归档既不删除 alias 也不重定向其 owner，因此 source 或 target 仍被 alias 引用时拒绝，即使该 owner 已不在当前 selection/context 中。
- 以穷尽 State 解构声明允许写集。只能追加这一个 operation、同一 actor 和标准 event；其他 branch、原永久收据、Commit/DirectoryTree、entry/file/version、placement、选择/context、shared alias、备注、策略、认证、topology 等全部不变。business generation 按原 planner 增一。真实序列号允许既有 journal 间隙，不假定 previous.sequence+1。
- `file_names`/`file_locations` 必须等值。它们参与 `projection_path → PreparedNames::apply → synchronize_location` 的实际容器路径，不是无副作用的读缓存。已发布 State 已捕获名字；真实冷恢复只丢 tracking，不丢这些值。缺项或篡改前态不得借归档从 raw head 补名字、移动 owner 或重排 alias。
- 即使 receipt 的 kind/source 被去掉，真实 Archived 变更也必须存在对应认证；已归档分支不能借无 HEAD 变化事务恢复活动状态或删除。即使 branch/directory 完全没变，伪造归档成功收据及重写旧 actor/event 仍不能走快返。

成功后继续共享原 owned namespace 表，不从 raw head 重建 entries，不读取或复制正文，不添加 workspace 准入 operation。归档元数据不能使已隐藏 occurrence 或旧 held workspace 复活。

## 待执行测试

`test/engine/unit/engine/current_branch_archive_tests.rs` 通过新 helper 私有模块接线，没有生产测试 hook。14 个测试函数覆盖：

1. 两次真实 D3 后归档，原 Commit/version/placement/shared alias/pending 不变，当前投影仍隐藏退出项，显式历史仍完整；不同 operation 再次归档、原 actor/digest 永久重试和冷恢复；旧 held workspace 的暂存保留、stage/commit 均不重新准入。
2. Disabled 和 Detached owner 分别归档。真实 D3 取得 closed proof 后将目标根改名，并以普通文件占据原路径；归档和冷恢复仍不依赖该根，保存的目标目录字节不变，原 writable=false 保留。
3. main、空 actor、Open workspace 仍用原业务错误拒绝；显式 abort workspace 后才可归档。
4. 当前选中文件分支仍拒绝归档；未选中的文件分支可归档，pending 继承和原选择不变。
5. 40 类篡改在 tracked 与序列化/反序列化 State 下分别拒绝：actor、digest、operation、kind、registry/mask/activation、head/generation/state、原 Commit/版本/placement/pending、shared alias、context/write-origin、仓库/其他 branch、旧回执/actor/event、缺失归档 operation，以及 envelope、正文保护、认证和拓扑的额外写入。
6. 不完整前态不能借归档补 file_names/file_locations；错误名字和替代 owner 路径也拒绝。真实完整前态的冷重放在前述正例中覆盖。
7. registry 建立前的原归档成功收据，在两次 D3、之后同 branch 的新归档与冷恢复后仍按原请求返回，不要求为旧成功补当前来源字段。
8. Sealed workspace 继续阻止归档，暂存项保持；显式 abort 后才能归档。
9. 通用 kind gate、缺失 registry、错误摘要/来源、额外 State 写入、正文 Prepared 和无真实分支变更的归档候选，在离开 planner 前拒绝；准确 metadata-only 候选保持共享 scope。
10. 无新归档回执时，独立修改旧 actor/event、重放重复归档 event、复活或删除已归档分支，分别在 tracked/cold State 下拒绝。
11. 八种 branch/head/fork/history/repository 身份缺失或错绑，在生成来源证明之前拒绝。
12. `fault-injection` 下在 finalize、容器名字投影、首个 publication 后中断；旧内存状态保留，冷恢复只完成原归档一次，永久重试不重增代，两次冷开仍保留 mask、历史和 stopped 根原字节。该组是逻辑故障注入，不能替代物理断电验收。
13. 真实文件 merge 后改选另一个分支，再完成 D3。merged alias 及已不被选中的原 owner 均拒绝归档，原 alias 边和状态不变，冷开仍通过原历史校验。
14. 在测试内实际编码 snapshot/history/candidate 并写固定成员 Prepare/Accept，以及可选完整 Finalize/Publication/Seal。同一合法 archive planner 候选分别保持原样、篡改来源 mask、偷改 file_locations，再移除一个候选专属 snapshot 副本。真正 `Engine::open` 对合法对照应修复并完成；对两种无效证明必须在补写副本或投影之前拒绝，缺失对象仍缺失，所有存活 metadata/data 根、SYSTEM 和已停止目标的文件字节保持原样。测试保留真实哈希与固定成员证据，不以 hash 错误替代业务证明拒绝，也不只调用纯 helper。

这些是临时目录 Engine 测试源码，不能替代真实物理设备、断电、HTTP/SMB 或全部生命周期验收。未运行的前置/旁路行为不能视为已通过。

## 成本和同步预算

常规发布按 operation/branch changed keys 定位归档，未改 map 借已有追踪身份跳过内容比较；归档的不可变字段按目标 pending map 的规模比较。main/selection/context 检查沿用原引用表，alias owner 校验增加 O(alias 数量) 的只读遍历，不增加反向索引。允许写集不会 clone 全 State、复制全部历史或重算 namespace entries 摘要。冷恢复失去 tracking 时比较相关持久 map；不克隆或重建名字/位置 map，不扫描 raw main heads 补缺项。

没有新增正文 I/O、发布阶段、fsync 或 quorum；容器 pending 描述依既有公共 publication 路径更新。冷恢复新增的提前校验与后置安装校验均为内存证明，未缓存跨生命周期授权。拒绝边界是无效候选的 replica/name/receipt 副作用，不宣称整个启动流程不执行既有 journal file/parent fsync。Excluded root 不重开的实际行为仍以统一测试结果为准，没有宣称吞吐或同步预算已完成验收。
