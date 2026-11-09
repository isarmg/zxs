# 外来高级盘的完整低级链实物准备

本片接入手动 root 辅助工具的独立 Web，将来源检查、完整低级链选择和实际复制接成一个可按原请求续作的操作。**结果仅是整批隔离材料，不是外来盘或低级盘正式接纳。** H03、H04 与正常 writer 的未完成条件不解除。原切片基线 `5c14a7a9`，发布对齐父基线 `a8a128f2`；未审查、编译、格式化、静态检查、测试或 CI，未操作真实磁盘或系统。新增测试只作为源码保留。

## 当前可执行范围

- 既有 production 池、原服务账户与固定 `/zxs/data/{state,management,disks}` 布局。管理凭据、当前 Config 摘要和目录身份全部绑定原计划。当前仍要求所有历史登记启用；遇到 stopped/retired 登记在盘目录遍历前拒绝，不借此重开旧盘。
- 一个经现有 `ForeignImportPreview` 完整本地证据核验的外来二级或三级 managed-directory 来源。来源来自另一 pool，不能重导入当前池身份；独立 disk-journal 原型、空历史来源、未知格式不在此入口支持。
- 二级来源必须同批指定一个真实一级空根；三级必须同批指定真实一级与二级空根，按级别明确列全。每一级的槽位、上游和元数据成员意图显式绑定；来源也明确指定新槽位、最高低级上游及元数据意图。不产生虚拟槽位、默认盘或“以后补上游”。
- 来源、已有池和全部低级根必须是 discovery 的不同实际直接子目录，属于指定服务 UID/GID，权限为 0700。所有实际子目录均须被当前配置或本批精确覆盖；拒绝未知内容、同 device/inode 别名、路径重叠、缺根或替换根，不修改权限或创建盘根。

此片选择“来源保持不变，所有低级使用新空根”的实际复制模式。它不承诺所有外来处理模式已经确定，尤其不实现复用已有低级目录、原盘重写角色、三级降二级或命名空间导入。已有空目录 Add 入口不接纳这些非空材料根。

## 单一原批次与实际写入

`/api/disks/foreign-chain-preview` 返回完整原计划及 SHA-256 请求摘要。计划同时绑定当前账户/布局/凭据/Config、来源逻辑登记与实际 inode、来源证据摘要、全部低级真实选择及完整拓扑。每个 native capsule 使用同一 operation ID、同一整批绑定摘要和不同明确低级层级；任意目标、角色、关系或来源变化都会改变其 native 计划摘要。

`/api/disks/foreign-chain-prepare` 需要明确的 `prepare-entire-foreign-lower-chain-material` 确认。root 协调者先持久保存：

```text
/zxs/.zxs-helper-foreign-chain-<operation>/plan.json
```

同一 operation 不能另存不同计划。root 释放自己的 host 锁后，由同一程序的无环境、无补充组、非 root 服务账户子进程取得 host lifetime lease，打开既有池与管理凭据。它先检查**每一个**目标与全部原 native 计划，再开始第一个目标的复制，不允许只挑已准备好的子集继续。

每个低级根使用原有可恢复 capsule 写者，在持有的真实父目录下形成：

```text
<lower-root>/.zxs-foreign-<operation>.json      # 原 native 父目录保留意图
<lower-root>/.zxs-foreign-<operation>/           # 未发布暂存阶段
<lower-root>/foreign-chain-material/            # 隔离完成胶囊
```

真正复制来源的完整有界文件库存、目录、原始 seal/日志/证明/本地正文及原生来源分组；不使用硬链接、reflink、去重或裁剪来替代独立副本。每个目标自行检查全部文件承诺、来源证据与历史投影，文件/目录按原事务顺序同步，再 no-clobber 改名。来源无写入，原 history/pool/disk/slot/binding 和各原始一级分组均不改；同名仓库不合并。已有证据中的 `temporarily_unavailable`、purged、migrated 分类照原样保留，缺失正文不会被伪造或删去历史。

逐目标可能出现部分准备，但不会产生任何活动成员。所有目标完成、同步并再读验证后，root 才保存绑定原摘要和完整目标结果的：

```text
/zxs/.zxs-helper-foreign-chain-<operation>-complete/receipt.json
```

`batch_material_prepared=true` 仅对应这个整批材料终点；`foreign_admitted`、`changes_existing_pool_membership`、`starts_service` 和 `complete_installation_ready` 始终 false。没有新增投票、seal 重写、Config 安装、活动 selector、命名空间发布或 writer。

## 原请求恢复与缺件

- `/api/disks/foreign-chain-original` 按原 operation+摘要读取 root 保留计划，再检查每个原目标的 native 父意图、真实目录绑定及完成材料。native 状态检查不 mkdir、不 copy、不 fsync、不修复，仅区分 Unstarted、Reserved 和完整验证的 PublishedCapsule。服务账户打开既有 Engine 本身仍可能执行既有恢复，因此此请求不是“整个系统绝对只读”。
- 显式 prepare 重试只继续同一原计划。未写目标重新核对最初空目录选择；已保留目标按原 operation/digest 恢复；已封存者不重新复制，准确已发布者只重验并重做所需同步。未知、损坏或冲突材料不清理，不以新 operation 绕过。
- 已有整批完成回执时任何目标不再 PublishedCapsule 都拒绝，缺失目标不补造。未形成整批回执前的局部 native reservation/staging 按原事务的允许窗口继续，不把缺最终响应当“从未写入”。
- 当前 helper 仍要求来源原目录、来源证据、当前账户/凭据/Config 和全部原根保持不变。单个 native 已封存 capsule 能独立验证/完成，不等于本 helper 批次支持来源失联后的任意恢复或重新绑定。
- Web 在第一次发送 prepare 前先保存并读回一个原请求选择器，刷新后仅填入原 operation/摘要供显式查询，不自动执行；可下载完整原计划。浏览器记录不是权威或授权，站点数据被清除/设备变化时依赖用户保留的原计划。保存失败不发送准备请求；未知响应保留编号，不能声称成功或自动重发。

## 资源与同步预算

最多两个低级目标，整个当前根集合最多 32 个。root 计划、IPC 请求和响应各不超过 64 KiB；低级根最多扫描 64 个顶层保留项。native 库存沿用最多 100,000 项、深度 64、64 MiB 意图上限及默认 256 MiB 元数据、64 MiB 日志、100,000 版本、1 TiB 本地正文检查预算，超限拒绝，不返回部分成功。

为保持原已有完整校验路径，本片没有减少哈希、复制或 fsync。每个目标都独立全量复制/读验，预览、重新计划、最终状态也进行完整证据读取，成本随来源字节量和目标数增长；不是常数时间预览，也不证明实际剩余空间或物理故障域独立。Web 与子进程受现有单任务 owner 和关闭排空约束，HTTP 断开不能遗弃后台写者。

## 仍缺的正式接纳闭包

完整来源 State、永久 operations/principals/events 的来源认证与目标身份映射；SYSTEM/盘根/FILE 三范围数据库在共同决定上的安装；所有低级链、来源与目标共同的 authority/epoch/Publication；全目标原操作冷恢复；活动 Config/locator/Stop/fence/serving 切换，均不能用 capsule 或本整批回执替代。本片只把之前没有可操作入口的完整低级**实物材料准备**接起来，不把 H03 标为完成。
