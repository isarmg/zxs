# 备份原生 generation 的选择身份与激活隔离

[Engine](../components/engine.md) · [任务表](../../TASKS.md) · [真实围栏归档](backup-generation-control-archive.md)

## 本阶段实现

在现有 `PortableAuthority` 的原生 generation 解析中加入严格独立的 `Backup` core，格式为 `backup-registry-generation`。它携带原来全部成员实际选择的完整证书、原 `membership-control-archive` 根、未发布完整路由快照及相应输入摘要；使用既有 `membership-generation` 域编码和计算 identity，不把普通 Add/Abort、SYSTEM 进度或孤立的目标完成文件当作新 authority。

该 core 的外层及证书都有严格字段闭包。证书重新核原 transition、所有原成员及真实选择 receipt 的规范关联；新增冷态投影检查将完整源配置、被冻结的源和原末端、每个指定目标的身份/物理绑定/根名/逐级前缀与原任务逐项绑定。所有目标的完整文件和目录仓库地址转成相同的相对单位表，父子仓库及版本集合不分散到不同目标。完整旧 State 的序列、规范摘要及 native snapshot 根必须与 source fence 原出版边界相同。

这些是**选择身份与材料准入**，不是已经发布的新活动 generation。类型中没有偷偷把原 Config 作为新的有效拓扑：`effective_config` 对 Backup 明确失败，原生 registry/Current 转换、I/O 限定和普通激活仍拒绝。即使把这种 core 放进普通 generation 标记，也不能用旧 voters/旧 Config 启用目标 writer。原 Add/Abort/Exit/Readmission 行为没有被映射为 Backup。

本片按指示没有审查、编译、测试、静态检查或 CI。仅对编辑的 Rust 文件执行格式化，不是编译或产品验收。

## 后台任务与原任务恢复

原 registry/placement 后台任务在全目标 `GenerationControlPrepared` 后继续到 `GenerationAuthorityPrepared`：

1. 从每个目标自己的完整 State、路由快照及真实 fenced control archive 重新推导原生 core；没有旧源、SYSTEM 或其他目标回退。
2. 所有目标分别得到相同的精确原生对象并满足本地空间预留后，才开始写入。
3. 在隔离目录 `backup-generation-authority/objects/membership-generation/<prefix>/<hash>.json` 无覆盖持久化。不同旧字节或未知文件拒绝并保留。合法中断临时名最多 1,024 个，不自动删除、晋升或计入证明。
4. 全目标实际读回并经过原生 `PortableAuthority` 严格解析，才分别写 `generation-authority-prepared.json`。原源选择再次观察成功后才更新 SYSTEM 中的诊断进度。

这个目录不是 `metadata/pool-membership/generation.json`；没有写活动 disk seal、Journal 帧、Candidate、Publication 或成功 operation receipt，也没有赋值 Config/State/Current 或获取活动目标 writer。

`BackupDecidedTargets::inspect_generation_authority()` 为只读全目标重验。显式 `resume_authority_preparation()` 仅无覆盖补写同一已决定任务的隔离 authority 材料，调用者须另有目标写入权限；它仍要求所有真实围栏归档和路由输入齐全，缺失的旧原 journal 不能由计划或历史前缀合成。原先 `resume_generation_preparation()` 的较早阶段语义不变。

离线可信摘要仍须来自独立保存的原决定，不可从同一待验目录自取。解析 native core 本身并未解决外部信任、真实目标身份、实际数据验存或活动 I/O 准入。

## 资源与未完成范围

- 证书沿用 32 MiB 上限；新 native authority 对象沿用 64 MiB 原生元数据上限，外围清单与完成标记各不超过 1 MiB。
- 每次只保存第一份待写对象和当前目标推导结果，不同时收集八个全量副本。既有 State、路由、control-archive 读取各自预算仍生效；每目标均独立计费，不用去重或其他盘空闲空间冒充本盘容量。
- 实际 I/O/fsync 可能阻塞，取消检查点不构成任意故障下的硬关机承诺。
- 仍只支持完整固定 Genesis、单 primary 祖先链、替换一条原末端及明确选择的一至八个全副本目标；复杂旧分叉、异盘父子合流与外来高等级一次完整链正式接纳均未完成。

**最终活动切换仍未完成**：新的有效多下级 Config/CurrentRegistry、首笔 native Candidate/Publication、对应代际 journal/封印、serving locator、完整新绑定历史数据库、未来备份策略延续、目标 writer 和源围栏安全解除都还需要共同闭合。旧正文、旧数据库和全部永久收据不删，源池仍可能停写，已作 Backup 选择后不能假装可任意改作 Abort。不能把 `GenerationAuthorityPrepared` 显示成迁移成功或 H03 外来盘接纳成功。
