# Backup generation 前的共同路由准入

本片基于真实 `314c59d5` 的已选路由材料继续修改生产调用链，并独立对齐真实 `055143e4`，保留 helper 的本地 Samba 密码功能。按用户要求未审查、编译、测试、静态检查或运行 CI，没有操作真实磁盘。

## 实际修改

`route_topology::Topology` 只表达结构闭包，不包含磁盘根、不打开文件、不构造 Config、Current 或 AdmittedState，也不授 I/O：

- 旧的 `primary + copies` 请求只能取得唯一线性前缀。达到分叉时返回明确错误，不依配置数组顺序 `.find` 一个下级，不隐含“全选”，不改用户副本数。普通 `PlanningContext::route` 和 `CurrentNamespaces::backup_configuration_route` 均已接该路径；选定 slot 仍逐个要求当前写准入/Enabled 身份。
- 完整显式集合以准确 slot 为输入，要求无重复、一个 primary、每个备份的全部低级祖先都在同一集合且逐级相邻，返回稳定父先子后的顺序。不因其它分支存在就把它加入当前 I/O 义务。
- 文件历史复制的旧请求只有副本数，不含用户显式分叉选择。因此 native 冷态门禁必须重新生成与真实写者相同的唯一前缀，不能从 result 中的 route 向量推断另一份选择授权；并要求精确顺序、binding 集合和当前 Enabled 身份。原全部 payload、精确 State 变更、receipt/principal 与原操作摘要规则不变，旧合法单链顺序不变，没有收据重写或缺字段兼容。
- `RecursiveBackupCapture::extend_prefixes` 不再用邻接 `windows(2)` 推断关系，而对每个目标独立沿其冻结直接父链计算名字。兄弟 B/C 同属 A 时各得 `A/...`，D 属于 B 时得 `B/A/...`；不会生成 `B/A/...` 给 C。相同原单链仍得到相同捕获路径，永久 receipt 名称不重新采样。
- 已决定 writer 路由准备对原 primary→source 保留链加全部选定目标使用同一显式闭包。现有持久材料字段、值和原证书字节不变，不新增猜测目标、分片或副本策略。

结构检查只触及实际请求链。构建结构索引不要求无关旁支在线；真正选中的盘仍须通过既有身份、完整材料和 I/O 门禁。父子同盘、整文件全部版本和目录单位义务仍来自原全目标实体/placement 准入，不能仅用祖先路径检查替代它们。

## 普通激活不可消费 Backup 选择

`activate_pending_membership` 发现原 source fence 属于 Backup 后，不能把 registry/route 准备投入普通 Add 资源 promotion，也不能取走其 pending 资源冒称已激活。若实际发现的后继是已经认证的 Abort，则仍走原 Abort 冷恢复；本片没有凭空造 Abort 或放松原 Q。

普通 `Generation::verify_activation_candidate` 同样明确拒绝非 Abort 的 Backup origin。这个拒绝不表示真正的 Backup generation 已实现；它防止未来接线仅复用普通 Add 的首个 Candidate 而遗漏 placement、目标正文/数据库、新 writer 和旧副本义务。

## 明确未完成

本片没有放宽 `ValidatedConfig` 的单后继规则，因此不能以本片初始化/发布活动扇出代。显式树解析是实际冷门禁与准备共同使用的结构能力，不是一个可独立传入的 writer 权限。

真正的 Backup authority core、完整目标范围历史数据库、全部新目标 seal/journal、bootstrap registry/locator、同一首个业务 Candidate 和成功后的原决定恢复仍缺。必须由那个共同协议证明旧源保留、新目标有效与所有写者义务同时闭合，才能解除源围栏。当前仍无可安全执行的“最后切换成功”入口，原池可能停写且足够 Backup 选择后的 Abort 可能不可达。

本片未改变停止状态、原 State/Config、旧收据、源正文、目标 native marker 或普通设置/helper 路径。不把结构准入、格式化或代码提交当作活动 writer 已通过验证。

## 成本

结构索引按提供的拓扑构建，旧唯一链查询只沿所需前缀；显式集合逐项检查祖先，成本受请求路由长度和祖先深度影响。准备阶段沿用完整 genesis 最多 32 个原盘和最多 8 个新目标的既有界限；没有新增磁盘扫描或后台正文 I/O。
