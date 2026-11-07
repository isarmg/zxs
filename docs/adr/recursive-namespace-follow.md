# 显式递归备份的命名空间路径跟随

[ADR 目录](README.md) · [空目录独立发布](empty-directory-backup-publication.md)

状态：有限实现设计；本文不构成编译、运行、性能或发布验收结论。

本文正文记录先前 `file.namespace.recursive` / `directory.namespace.batch.recursive` 的 file-only 语义；其中非空范围限制仍约束这些永久旧 kind。当前新目录跟随能力见文末独立协议，不把新要求追溯施加到旧收据。

## 背景与边界

已显式保存且包含文件的递归备份配置需要随主逻辑路径 rename 或 move 更新多级备份路径。普通自定义前缀不能仅因文字形似盘名而被解释为递归配置；永久操作收据也不能被事后补写。空目录发布是另一种冻结目录快照，不是活目录镜像。

本片只跟随当前已验证、仍有效的非空递归策略及其绑定文件。普通自定义策略或覆盖后的自定义配置不被重解释。空 scope 配置不建立跟随索引；之后新增文件即使继承保护配置，也仍须显式完成一次包含文件的递归重配置才进入本片自动跟随范围。空目录的既有发布不随 rename 或 move 改写，需要另一次显式快照发布。

## 决策：新操作种类与精确请求闭合

新的原生操作种类是 `file.namespace.recursive` 与 `directory.namespace.batch.recursive`。新种类必须携带严格类型化的 `recursive_follow` 计划；不能靠可选字段赋予旧种类新能力。

规划和重放都从已准入的 before 视图绑定请求：仓库、源 entry 身份、期望 head、目标路径、批量 scope 与所属分支。复用业务 relocate 和 canonical location 纯函数重建 owner entries、`file_locations` 与 `file_names`，核对真实提交的 parent、身份、序号及 principal。请求摘要自洽不等于请求授权了任意实际命名空间差分。

跟随计划记录来源操作和策略，按完整预期集合核对策略及 `file_backup_paths` 增量，同时保持绑定、placements 和 `file_deletions` 等不允许变化的集合。遗漏计划、额外路径、错误来源及把新种类降级成旧种类后仍改递归路径均须拒绝。策略 scope 移动同时更新 path prefix、备份前缀及 generation。

已有 operation ID 先使用已绑定的操作种类与请求摘要执行永久重试。旧永久收据不可变，不补 marker、不修改历史结果；旧种类未自动改写备份路径的历史语义保持可重放，而不是要求旧发布满足新跟随计划。

## 派生索引与冷恢复证明

`CurrentNamespaces` 持有可重建的策略到已验证递归收据、capture 和 roots 的派生索引，不增加新的持久化 State 权威字段，也不盲扫 `State.operations` 或按前缀字符串猜测资格。

只有完整 backup publication 验证后，非空 `backup.configure-recursive` 收据才建立索引。热路径按变化策略失效旧条目，按已验证跟随结果转移有效条目。策略覆盖或删除移除旧资格。成员 registry 续接从先前已验证视图保留索引；首次进入 Current 时索引为空，此前不能合法执行需要 Current 的递归发布。

冷恢复沿完整 publication/archive 链逐步重建：验证归档证据、顺序、seal 和快照承接，逐次推进 namespace view，随后核对 generation source 的精确 snapshot/hash。已有 exit registry 的恢复不能通过 checkpoint 快捷路径跳过这些发布。若未来改变该恢复入口，必须证明索引仍由完整已验证链恢复，不能用空索引或未验 State 扫描替代。

索引共享使用 `Arc`；一次更新首次触发写时复制时会克隆整个索引。若索引有 P 个策略，此项成本为 O(P)，不能把总体更新成本宣传成仅与 changed keys 成正比。 请求闭合仍按本次涉及的当前 owner 命名空间比较完整 Entry 集合；路径计算按受影响文件及其固定路由增长。没有把这些树级成本宣称为常数，也没有新增按永久操作历史逐次扩大的热路径扫描。

## 路径、删除与磁盘边界

每个目标 root 由原 capture 中逐跳直接来源盘名和仓库前缀推导，再拼接文件当前 canonical path。例如 A 到 B 再到 C，C 的前缀包含 `B/A`，不是每一级都仅加 A。名称来自绑定 capture，不按当前字符串形状重新推测。

跟随前复核 capture 路由中各 slot 当前身份、层级、直接上游及可用模式。停盘或路由身份不再满足要求时拒绝此次跟随，不静默降级、跳过目标或创建新复制路由。软删除冻结规则先于跟随生效；不能借路径更新复活 Entry、分支、节点或改写删除事实。历史与自定义保留路径不被扩大成自动跟随对象。

## 物理投影与发布顺序

路径跟随与命名空间变化属于同一候选 State。`changed_files` 包含备份路径变化，`prepare_names` 使用同一冻结 before/after 投影准备物理变更；共享目录布局的 outside `stable_goal` 与普通容器路径使用同一逐 slot `projection_path`，备份路径优先于主逻辑位置。

实际容器移动仍在 Finalize 决议后应用，失败由原发布恢复机制处理。不另起独立路径写入，不增加新复制任务，不改变完整版本内容或保护路由；缺失副本不因命名变化被合成。主盘共享目录布局与备份目标路径不得互相覆盖。

## 替代方案与验证入口

拒绝以 prefix 猜测递归资格、扫描未验证收据恢复资格、在旧操作结果上加可省略 marker，以及冷恢复时跳过无法解释的跟随差分。这些做法分别丢失来源、授权或冷热一致性。

定向验证入口：

- [`test/engine/integration/recursive_backup_follow.rs`](../../test/engine/integration/recursive_backup_follow.rs)：真实 API、路径跟随、策略覆盖、删除与冷恢复场景。
- [`test/engine/unit/engine/current_recursive_backup_tests.rs`](../../test/engine/unit/engine/current_recursive_backup_tests.rs)：原生发布与篡改请求、计划及额外路径的拒绝场景。

测试源码存在不表示已经执行通过。实际验证结论必须绑定最终源码与运行证据，不能由本文推断。

## 空目录后续语义

已捕获空目录现在由独立 `file.namespace.recursive-directories` / `directory.namespace.batch.recursive-directories` native kind 跟随，并保持本文既有 kind 的原语义。详见[空目录发布 ADR](empty-directory-backup-publication.md)。旧快照/收据不改写，新增目录和捕获盘名前缀仍须显式重新保存。


## 按实际保留路由发布的新语义

新的 Current 递归改名/移动使用 `file.namespace.recursive-routes` 或 `directory.namespace.batch.recursive-routes`；旧 operation ID 永久重试仍先选择其原 kind，旧 file-only 与 directory-follow 四种 kind 的验证不变。新 kind 同时保留精确 `recursive_follow` 与 `recursive_directory_follow`，不借省略目录计划换取能力。

一个策略可以描述多条独立来源链。已有文件显式配置可能持有策略全部逻辑路径键，后来继承的文件只持有其实际链的键。新计划只改已有且属于原绑定 roots 的键，不凭空补其它链、不删除额外键、不重新采样名字；实际必需备份键仍必须存在且等于原 capture root 与旧规范路径。

物理准入与逻辑策略键分开：一次扫描 previous 的全部版本，为本次移动文件汇总所有 placement 路由和绑定代，拒绝缺 placement、缺 binding 或同槽代数冲突；不只看 HEAD，也不按 Available 或 Current 可见性过滤历史。所有必要路由保持原捕获身份、直接上游及现行可写状态。真实历史副本所在盘停止仍阻断，不能把它称为无关盘；只有其它文件/未使用策略链不扩大本文件的检查范围。scope 前缀变更本身不是向全部策略根写盘；捕获目录快照仍验证其全部实际 replica 目标。元数据成员、冻结、闭合 State、Finalize 和恢复门禁不变。

新路由汇总只扫描一次版本集合，BTree 查询和插入仍带对数成本：上界 O(V log(F+1) + R log(S+1))，V 为当前 State 版本数，F 为实际跟随文件数，R 为这些文件保留路由总长，S 为每文件不同槽数；不声称 changed-key 常数成本。返回的路径计划、全部版本和 placement 不变条件由冷热同一验证器证明，不以派生缓存作为权威。
