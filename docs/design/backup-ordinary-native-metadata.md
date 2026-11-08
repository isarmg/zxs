# Backup 普通原生对象安装与 Current 重放

[任务清单](../../TASKS.md) · [SYSTEM 普通库](backup-ordinary-system-database.md) · [首笔冷重放](backup-generation-first-replay.md) · [Engine](../components/engine.md)

本片把原生首笔完整 State、Candidate、history、不可变 generation authority、registry 父／后继图、实际 Finalize／Publication 和 PublicationSeal，安装到**每个原盘和全部选定目标的普通原生地址**。不再只在目标的 `backup-generation-*`、`backup-published-registry` 隔离目录中保存这些对象。SYSTEM 保存同一原操作的共同安装描述，不另建一套 SYSTEM 原生对象库。

按用户要求未审查、编译、测试、静态检查、格式化检查或 CI，未操作真实盘。新增六项临时目录单元测试源码未运行；它们不覆盖最终 Engine 切换或多盘断电。

## 全体实际证据先于写入

原 registry/placement 后台任务在普通 SYSTEM 库安装完成后继续这一阶段。重新观察原盘 current/history、SYSTEM 已关闭 SQLite 的普通索引和保留集、全部目标自身的完整实体／正文／库／namespace，以及每个原 metadata 成员真正的首笔 Publication 日志和 seal。没有脱离源的独立写入口，也不以单目标完成记录替代原全体成员。

安装对象严格来自同一准确的已验证首笔和 registry 配方，不增改原 Candidate 的 registry 字段、operation、正文义务或旧永久回执。全体原盘仍保留，替换旧末级数据路由不是移除旧成员，更不把新数据目标升级成 metadata voter。多下级完整单位、父子同层同盘和完整低级链仍由原决定约束。

对象实际进入生产分片地址 `metadata/objects/<kind>/<前两位>/<hash>.json`；真实原 seal 进入 `metadata/publication-seals/<candidate>.json`。复用生产不可变对象写者，拒绝 flat 旧格式、同名异内容和异常文件，重试只同步同一准确字节，不覆盖、不回滚、不删原材料。全部原生依赖及完整 registry 父／后继页面都读回，不用内存中可重建的页面代替丢失文件。

## 未激活的目标普通 metadata

首次普通 metadata 写入之前，SYSTEM 持久保存 `backup-preparation/native-metadata.json`，随后每个目标持久保存绑定实际目录身份与准确共同摘要的 `backup-native-metadata-selected.json`。这一记录不是 generation authority，也不从中解码 I/O 路径或 State。

原先“metadata 必须不存在”的隔离门禁现在只允许这一明确的**未选择活动代际**阶段：目标仍不能有 `disk.json`；metadata 顶层仅允许 `objects` 与 `publication-seals` 两个目录。`pool-membership`、generation journal、活动 selector、任何未知顶层条目仍拒绝。已有冻结读者只继续读取原隔离域和原证明，不能借记录访问普通业务或获取 writer。原安装器另用重新推导的准确对象集合递归核对普通 metadata，拒绝未知／旧式／多余文件；中断的原子临时文件有总条目界限并保留，不清理。

因此同原决定冷续作可识别“已经有普通原生对象但尚未激活”的真实阶段，不把自己的部分安装误认成外来已活动盘；没有无条件放开任意 metadata。

## 从普通对象重建只读 Current

每个原盘独立读回完整首笔 State、registry 和实际 Publication/seal。每个目标还将**该目标普通 metadata 中实读的** State、registry、Publication 输入同一个 Current 重放器，重放原完整归档，保留 D／undo、退役冻结、历史身份和递归策略。隔离对象和普通对象共享这一实现，不复制 namespace 判定。

这证明普通原生材料能够支撑准确的只读 namespace；不会把只读 loan 转成 `Engine.current_namespaces`，也不把首笔可读语义当作第二笔发布许可。`inspect_backup_native_metadata` 在原 actor/operation、所有原锁与完整目标下重新观察这一状态，无补造、写者或解围栏行为。

## 部分完成与资源边界

在任何完成记录出现前，可按原全体证据补入尚未写出的准确对象；任一参与根的 `backup-native-metadata-installed.json` 已存在，意味着此前已完成全体实际读回屏障。此时必须保有全部普通原生对象、目标选择与 SYSTEM 原描述，丢失／损坏不能降级成可重新生成的准备任务。只缺后续参与方的完成记录时，可在全体验存后补存相同记录。

每个对象遵守原 metadata 单对象上限。合并的原生集合最多 12,292 个对象／256 MiB，描述最多 4 MiB，目标选择最多 4 KiB；目标库存扫描最多 49,168 项。对象集合借用已有准备字节，不为每个盘同时复制整份集合。安装逐对象、逐根进行，仍可能持有完整首笔与 registry 编码，并重复多轮全归档／正文／数据库验证；不宣称低峰值、增量成本或硬时限。容量预检包含各根独立缺失字节、最大原子暂存、描述、目录和原 metadata reserve，不能当物理预留。

**仍缺**活动 Config／普通 generation locator／Engine Current、目标 writer 生命周期、下一笔普通 publication、SYSTEM 查询解静默及安全源解围栏。没有新 `disk.json`、generation.json、journal、writer.lock 或 I/O epoch，未改变普通 Backup 拒绝分支。原 pool 继续停写，H03 与完整迁移均未完成。
