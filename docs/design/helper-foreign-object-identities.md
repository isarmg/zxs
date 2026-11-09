# 外来完整链的对象命名准备

本片接续[原完整 State 与三范围数据库](helper-foreign-original-databases.md)，解决“只给磁盘新 ID，尚未逐项绑定对象身份”的下一层准备问题。**它不是完成接纳**：实际对象记录/引用尚未按映射改写，未构造联合 State，未安装目标身份数据库、共同发布或活动切换。`foreign_admitted` 与 `complete_installation_ready` 保持 false。

## 输入与可信边界

1. 原整批 capsule 已完成；每个真实低级根的原 SYSTEM、原盘 current/history 和 FILE/目录库必须从自身原生完整 State 独立验证。完成 marker、健康同伴或单份 SQLite 都不能选举来源。
2. 重新构造全部新登记提案，覆盖从一级到来源高级的完整真实链。Engine 再验证新 Config 只是完整新增链，原成员和全部参数不变；新绑定代数为 1，原成员都 Enabled。材料只能保存到提案的准确低级根，最高一级仍绑定原来源路径，不向来源写入。
3. 在当前 Engine 及现有管理员授权下捕获完整当前 State 哈希、sequence、当前 Config 哈希、新 Config 哈希、原操作/整批摘要和新登记提案摘要。它是不可反序列化的 `ForeignIdentityTarget`，不是 membership 或长期授权凭据。来源扫描在 Engine mutex 外，每个材料写入前及整批结束后重新检查当前授权与同一 State。Config 不变但业务记录已变也拒绝，不将旧意图重新绑定到新现池。
4. 每个低级目标继续使用其自身完整原生前缀。原固定 genesis、32 盘、2,048 发布等既有支持边界不扩大，缺件或不支持布局保留拒绝。

## 对象命名与历史身份隔离

通过穷尽 `State` 字段分类收集本地拥有的实体身份，包括仓库、历史、文件/目录分支、文件、版本、提交及 tree、目录项、路径/元数据/删除节点、合并与边界记录、放置规则和保护策略；隐式文件/目录身份从真实 namespace 与 pending/inherited 字段补齐。投影字段复用原 State 的完整闭包验证。新增 State 字段必须明确分类，不能被 `..` 静默漏过。

映射按原完整 State、当前完整基线和身份域确定新 ID，保留已有类型前缀，拒绝与当前/原有/本批派生对象冲突；不碰撞重试、不凭名称合并不同来源。原 ID 在本地私有映射中保持可追溯，映射摘要只通过有界管道返回。整个原 State 的身份库存都参与准备，**不表示所有原历史的正文当前可用或已映入新的活动目录树**。

原 `operations`、`operation_principals`、`plans`、`events` 与 `control_auth` 不复制成现池活动记录。操作、历史作者和计划分别使用 `foreignop_`、`foreignactor_`、`foreignplan_` 审计命名空间；前者不是可解析的现池 `OperationId`，作者名称不变成现池账户或权限。原永久记录保留在原 native 材料，并以独立用途哈希绑定。事件顺序不重新编号，不改写未知 `result/input` JSON，不导入源认证凭据，也不把外部边界 provenance 当成本地活动引用。

这只是完整命名表与证据绑定，**不是引用转换器**。后续实际联合 State 构造必须逐个类型转换活跃引用、路径与 placement，并保留独立的历史审计命名空间，不能用递归替换字符串或把历史成功回执伪造成目标成功。

## 不可覆盖的同批准备与续作

每个真实低级父目录保存独立三文件，保留原 capsule 和数据库字节不变：

```text
.zxs-foreign-identities-<operation>-intent.json
.zxs-foreign-identities-<operation>.json
.zxs-foreign-identities-<operation>-complete.json
```

intent 先于映射写入。父绑定包含真实 device/inode、原 capsule 摘要及完整期望 report；映射采用现有原子不覆盖写入，文件和父目录同步后才保存完成记录。每次查询从该目标自己的原生证据重新推导完整映射并逐字节比较，不信任自报摘要。完成记录存在但 intent 或映射缺失时拒绝重建；错误字节保留拒绝。最后 marker 中断窗口只允许验证已有准确材料后补最后记录，不覆写映射或由其它盘补缺。

root 保存 `.zxs-helper-foreign-chain-<operation>-identities` 和相应 `-complete` 的 `plan.json`，沿用原 immutable bundle。首发前先核每个真实目标，所有目标独立派生同一 report；全部验证并同步后才记录整批准备完成。整批完成后任一目标缺件拒绝补造。现池变化、认证变化、原来源/根更换或摘要变化均只报原操作失败，不能换来源、清理或回滚。

独立 Web 增加：

- `/api/disks/foreign-chain-objects-preview`：查询原批命名闭包/准备状态。
- `/api/disks/foreign-chain-objects-prepare`：确认词 `prepare-complete-chain-object-identities-without-admission`，仅保存上述材料。

继承同源/CSRF、root 手动运行、单任务、同版内嵌 ZXS 和 64 KiB 管道边界，没有 helper→server/contract 新生产依赖，也不启动服务。Web 写前保存并读回原选择器；重复点击只发一笔，断连保留原操作与摘要，刷新不自动重放，新输入使旧预览失效。映射文件不在响应中传输，仅返回基线、计数、摘要和每个原目标的状态。

## 资源、仍缺与验证状态

命名对象与审计身份总数最多 100,000，序列化映射最多 16 MiB；增量库存预算在分配前约束实体/作者/审计键。独立原 State、三范围镜像、当前身份库存及 JSON 输出有额外同时驻留内存，16 MiB 不是进程内存上限。每个低级目标会再次读取完整原生前缀及三范围镜像，成本随源 State 和目标数增长，不宣称常数时间或吞吐改善；未减少同步、校验或权限检查。

仍缺：映射后的完整对象/路径/placement 引用转换、保留现池永久记录与独立外来审计的联合 State、联合三范围数据库、各层真实正文可用性/落位、共同 authority/epoch/Publication、Config/locator/Stop/fence/serving 切换及冷恢复。不能把本片名称准备或原 SQLite 的“完成”当成其中任何一项已经完成。

原候选父基线 `cb7fb288`，本次对齐 `c9b22db6`，当前组合没有重新验证。按用户最新要求，**未本地编译、未执行测试、未触发 GitHub CI**；仅对明确受影响 Rust 文件运行格式化，`git diff --check` 通过；这不证明类型、逻辑或运行正确。新增六项临时目录集成、两项命名模型、两项请求门禁以及真实 JS handler doubles 场景目前都只有源码，未执行，未操作真实盘/服务或进行浏览器验收。上一片最后一次生产组合 check 未通过；“共享旧缓存导致新 Operation 缺失”仍只是未复验推测，本轮不提供通过或已修复结论。

后继本地片已有[来源域 typed 历史图转换](helper-foreign-mapped-graphs.md)：把对象名应用到各类型的局部引用并保留来源审计/路径/placement 域，新增单目标隔离图准备 API，尚未接 Web/同批协调、联合 State 或正式接纳。该后继未编译测试，不改变本文命名阶段的准确完成语义。
