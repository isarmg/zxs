# Backup 目标真实 writer 文件围栏

[任务清单](../../TASKS.md) · [普通原生对象](backup-ordinary-native-metadata.md) · [首笔冷重放](backup-generation-first-replay.md) · [Engine](../components/engine.md)

本片继续原 registry/placement 任务，在全部原盘和目标的普通原生对象、数据库、实际首笔 Publication 及目标正文重新成立后，创建并独占锁住每个选定目标真实的 `.writer.lock`，持久绑定其设备／inode。不是只增加 writer-route 描述，也不是活动 `DiskWriter`：不授予目标运行时 I/O，不写 `generation.json`、`disk.json` 或活动 SYSTEM Config，不解开原全池写围栏。

原候选曾因 main 更新被平台拒绝而保留；本批已获用户准确批准，现仅将原 writer 文件锁及恢复持锁范围对齐 `04af3778`。后继 DiskWriter I/O、目标 seal 和 locator 独立候选不包含在这批中。

按后续验证要求，Rust 1.99.0 下生产 `cargo check -p storage-engine --lib`、`cargo check -p storage-engine --tests` 及所有变更 Rust 文件的格式检查通过且无警告。针对八项私有临时目录单测的运行尝试，在生成整体 engine libtest 时被 SIGKILL，未到测试执行阶段；没有重复链接，不计为通过。未触发 GitHub CI，未操作真实盘；这些类型检查不构成 Engine 最终切换、并发交错或断电验收。

## 全体证据与准确文件身份

SYSTEM 的 `backup-preparation/target-writer-fences.json` 绑定原 source fence、decision、真实 Publication、普通原生安装、authority、Candidate、registry 及完整选定目录身份。写前仍需原全体 source 能力和所有目标的实际完整材料；描述文件自身不是 authority，不从中读取 I/O 路径或 Config。

每个目标先持久保存 `backup-target-writer-intent.json`，随后无覆盖安装空 `.writer.lock`，取得同一真实文件的非阻塞独占 flock，重同步文件与根目录，最后持久保存 `backup-target-writer-binding.json`。绑定包含准确 intent 摘要及 writer 设备／inode。已有异内容、链接、异常文件或无原 intent 的 writer 名称均保留拒绝，不能把未知空文件当作已授予的 writer。

绑定落盘前，原 intent 允许显式继续之前中断的空文件安装。绑定一旦存在，后续打开和显式重试都必须使用相同 inode；缺失或替换不是新的初始化机会。正常读验／冷重获锁不会创建名称。这个阶段不做 rename、unlink、清理或旧格式兼容。

所有目标的真实锁与绑定齐全、原全体证据再验后，SYSTEM、各原盘和全部目标保存相同的 `backup-target-writer-fences-installed.json`，含全部目标实际 writer 身份。任意一份共同完成记录存在，就要求全部目标的 intent、绑定及 writer 实物原样存在；不能补造丢失的绑定或重建 lock inode。只缺其他参与方相同完成记录时，显式原任务可在全体验存后续写这些记录。

## 真实持锁生命周期

原 registry worker 与 Engine 保留同一目标目录能力、独占目录锁及 writer owner。后台线程结束、返回错误或被 join 后，Engine 中的 worker 仍持有这些资源，不把“后台完成”当作释放目标 writer 的信号。writer 获锁后先存入共享 owner，再做可能失败的同步和绑定写入，因此错误路径不会悄悄解锁。

同一原任务重试及 Engine 的来源绑定检查借用同一批持有资源，不释放再获取，不以进度缓存生成新的 writer。目标路径和原物理目录每次仍须匹配；已持有的 writer 名称替换会阻断后继阶段。重启后，原操作先取得目标目录锁，再读取原 intent／绑定并重新 flock 同一个 writer inode，随后重新通过全部原生材料门禁。

这也改变了并行使用边界：运行中的原 Engine 保留 registry 目标目录时，独立 helper／离线目标 opener 不能同时取得这些锁，应先关闭原 Engine；不能为检查暂时放掉它们。Engine 自身的准确原操作读验可复用持锁能力。`inspect_backup_target_writers` 要求该 Engine 已保留原目标资源，再进行全体只读观察；不会通过一次临时开锁而声称长期拥有目标。

关闭 Engine 时先请求后台停止并等待退出，再释放最后持有者。落盘的 intent、绑定、完成记录及 `.writer.lock` 全部保留。关闭资源不解除来源的持久 MembershipFence，也不意味着原操作取消或成功。

## 成本与尚未完成的切换

目标仍为原准确的一至八个，原盘上限沿既有三十二个；各描述上限 64 KiB，每个参与根须保有原 metadata reserve 加至少 1 MiB 的新增记录空间，物理预检不是容量预留。目标额外占用最多两类 OS 锁和一个共享 owner；检查绑定及记录为线性目标／参与根成本。文件创建、原子记录与目录均持久同步，复用已有 no-clobber 原语，不能为了减少 fsync 跳过原绑定屏障。完整来源、归档、正文和数据库仍被重复观察，不宣称增量成本、低峰值、硬时限或吞吐提升。

最终还需要把准确 Backup authority、完整 registry 和 Current 接入普通代际发现／配置／locator，建立目标 `DiskWriter` I/O epoch 和普通后续 publication，再共同开放 SYSTEM 与新代际写入。当前普通 Backup activation 仍硬拒绝，较晚槽仍未获准。提前写活动 selector／disk.json 会让冷开转入尚未闭合的通用代际路径，因此本片不这样做。

**真实 writer 文件围栏不等于活动 writer，目标互斥不等于可安全解开源围栏。** 活动切换、完整迁移和 H03 外来完整低级链接纳均未完成；所有旧数据、永久收据、原成员及原围栏保留。
