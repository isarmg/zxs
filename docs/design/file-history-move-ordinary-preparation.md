# 跨盘移动的普通正文与未发布数据库安装意图

[任务清单](../../TASKS.md) · [唯一普通 Candidate](file-history-move-native-candidate.md) · [原选择与围栏](file-history-move-joint-choice.md)

原 `2a01b0e8` 候选基于 `7583e229`；本地后继最初对齐 `ac6e1096`，定向验证前续对齐已发布 `04af3778`；停止本地编译后仅源码对齐 `e03c1caf` 并保留后继 `23621d9b`，本片继续仅源码对齐 `90450cf0`；原候选均不变，继续同一原 move 的执行准备与冷恢复。**未接入 SMB 成功，不写 PREPARE/ACCEPT/FINALIZE/PUBLICATION，不发布源 D，不显露目标 namespace，不解除 choice 围栏。** 原候选创建时未编译或测试；本后继实际验证见文末。没有真实数据操作，准备测试不等于正式发布、SMB 或掉电验收。

## 原材料不挪走，普通正文另存独立完整副本

`prepare_file_history_move_ordinary` 要求准确的原恢复封套、全体 choice、原 FULL 及其复制身份、两端 FILE、native/scoped 材料，以及前片全体元数据成员的完整普通 Candidate 已齐。它重新从同一候选推导目标版本/placement，不接受调用方提供的路径或 SQLite 作为权威。

每个原目标在 `file-history-move-ordinary/ordinary-inputs.json` 固定原 candidate/choice、真实盘登记、目标文件/history/name、全部版本摘要与对应原 staged-copy 身份。目标根和 stage 都使用持有目录能力，重新核根 inode、seal 和 I/O admission；仅目标正文盘不因此获得全池 SYSTEM。

目标普通容器先独占创建于 `histories/H/file-containers/F/<目标名>/versions`，三层目录原 inode 固定在 stage 的 `container-origin.json`。同名但无原记录的空目录也不采用，不用补写来源记录给已存在对象授信；也不接受已提前发布的 locator。

每个目标版本另做完整普通字节复制，来源必须仍是此前原目标准备留下的、copy identity 一致的实际持有文件。不能借硬链接、reflink、去重或任意同内容文件完成它。原源文件和 `file-history-move-target` 都保留原处。新的版本目录先在普通准备 stage 中独占创建，写入真实 `version.json`、`checksums.bin` 和 `payload.json`，正文经现有 FULL 复制器完成长度/分块/全量摘要校验和同步。

在任何重命名前，`origins/V.json` 记录实际新目录 inode、实际正文 inode 和跨 rename 保持的 mtime，以及原 candidate/version/copy 身份。随后分别通过 no-clobber、目标目录先同步的原语：

1. 把该原版本目录移到 `histories/H/versions/V`；
2. 把同一正文 inode 移到已绑定目标容器的 `versions/<storage_name>`。

不用通用安装器的“目标同内容则删除准备副本”分支。每次恢复必须在两种目录地址中恰有一个原目录，在该目录 content 与已绑定最终容器中恰有一个原正文；两个都在、两个都缺、替换 inode、mtime 或元数据不符一律保留拒绝。复验直接打开这个原正文，而不是让 resolver 任选相似副本。完整校验期间核 held/named 文件 stamp，读完再核目录和正文位置。

## 失败与冷续作边界

原 origin 完整后，中断在版本目录 rename 前、两次 rename 之间或目录同步尾部，可以在准确原操作下继续。同一正文不重新复制；恢复补齐原目录/文件的同步屏障。只读 `read_file_history_move_ordinary` 不补文件，只有两种普通最终地址均完成、所有原 FILE/current 安装对及其冻结记录逐份一致时，才报告正文数量、字节数与实际数据库意图数量。

写正文或容器 origin 前中断留下的未登记对象不会被自动采用、覆盖或清理。已登记正文后来丢失/损坏也不会从原 stage 自动生成替代 inode。此类情况保留原源、原 target stage 和其它已写字节，返回错误，需要后续显式诊断，不声称自动恢复已覆盖所有中断点。

准备返回前还重新核原 choice、Candidate、全部普通正文和持有安装意图。失败包装为 `RecoveryRequired` 时仅说明可能留有部分材料，不能据此推断成功或回滚。原 Engine State、日志和读写围栏不变。

## FILE 与盘根 current 接到通用 preserving installer

普通正文安装后，从同一私有候选 State 走现有 namespace/FILE 准备器。实际捕获到的 `(slot, history, file)` 集合必须精确等于原源全部 placement 加全部目标 placement；通用 cache 准备中跳过的缺失容器不能被计作完整。此入口不应用 layout/name/owner/container.json，也不把目标 FILE 提前放到活动别名。

通用 `PreparedInstall` 拆出仅暂存步骤：普通的 apply 保留原有交换流程；新增 `stage_unpublished` 只持久化原 payload 与 intent，再核活动别名仍是原 old、stage 仍是原 new。两者都按 inode、长度、摘要绑定。已经提前交换、意图被替换、原 payload 缺失或已位移都不能充当“未发布”准备成功；已决定普通修复原有的保留/续作策略不因此改成 move 的权限。

盘根 current 只由私有 `NativeMoveCandidate` 的真实 inventory 编码并解码比较。全部原参与盘先捕获普通安装意图，拒绝 Backup 已选择根、未知 SQLite 侧车或声称超过源已发布 sequence 的活动库，再按同一未发布流程暂存。没有执行交换，原 `.zxs-disk.sqlite` 保持不动。

数据库 intent 继续使用原通用 scope/entity/bytes 绑定；调用方必须先通过原 choice 和候选重建，不能直接拿 intent 选 State。失败后的显式 prepare 重捕获同一来源，复用原 old/new 对，不把丢失的已登记 new 重制为另一对。所有源/目标 FILE 和原参与盘 current 在最终返回前再次校验。

**SYSTEM 仍只是前片候选镜像，盘根 history 的新 move 行仍未生成。** 新 history 行必须来自真实共同 Publication，本片不伪造。普通 FILE/current 的 pending intents 不是三范围共同安装完成，更不是决定。本后继只读重建完整候选 FILE/current 镜像并逐一核对原安装对；它不走布局/名字缓存准备，不创建目录、匿名 cache 或安装 attempt。两类 intent 计数只在全体核验后返回；计数不是活动安装或决定。

## 冻结原安装对，而非重新制造同内容数据库

在所有 pending FILE/current 均同步保留后，`ordinary-installations.json` 将相同 candidate/choice 与准确 scope 集合、每一份通用 intent 的摘要绑定。摘要包含实体目录、暂存目录、old/new 正文 inode、长度与内容散列，不只是 SQLite 字节。该不可变记录复制到原 SYSTEM 和全部原参与盘，不写 journal，也不安装数据库别名。全体记录读回一致后，普通准备入口才返回完整观测。

任一副本还在，冷 prepare 必须先用纯读重建所有原安装对并与原记录逐项相同，再允许补齐缺失的记录副本。整个 pending generation 丢失、payload 被同内容新 inode 替换、局部 intent 被一起改写、活动别名被提前交换，均拒绝，不重新创建数据库对或覆盖原记录。只读入口要求全部记录已齐；不会把单成员原记录当作其它成员或全体原安装仍存在的证明。

此记录只是收紧恢复，不能授权下一槽、源 D、目标可见、已决定修复或清理。尚无完整记录的早期失败仍保留已有 intent；原 intent 中已登记 payload 丢失不能被重制。全体记录和全部相关安装材料都被外部删除后，不声称能凭空证明它们曾存在；原 choice 与源材料仍保留，正式 journal 权威尚未产生。

记录上限 64 KiB，每参与盘配置与物理预留计入该上限加 4 KiB；SYSTEM 写入继续受原 metadata reserve 检查。无新后台扫描或轮询，不保存新的全池 SYSTEM 到仅正文盘。冷核验每份 FILE/current 重建一次受限内存 SQLite 并重新读取实际 old/new，仍非性能优化。

## 资源与仍缺工作

普通副本逐盘预留完整正文、checksums、每版本 64 KiB 及容器/清单开销，配置容量叠加原 target、joint/native/scoped/choice 和普通 Candidate；物理空间按缺失副本加 metadata reserve。复制/全量读取在 64 KiB 边界可取消，原 checksum 上限不放宽。

准备数据库时先捕获所有 FILE/current，再检查逐盘合计新暂存空间与保留材料的配置容量。FILE 沿用 64 MiB 聚合限制，新增 current 镜像聚合最多 128 MiB，单库保持既有 32 MiB；通用每 history 的 8,192 项/256 MiB 保留预算继续生效。普通 prepare_names 仍可能创建匿名 cache 和独立布局准备材料；通用数据库 capture 已改为纯读，retention 目录只在实际 stage 阶段创建。只读入口不调用名字/布局准备器。新普通 body、原 stage body 和原源共同占空间，不宣称只需一份正文容量。

本片会反复全量验证候选、源/目标正文和数据库，保留每次 rename 及各同步尾部的原语；尚未测量吞吐、峰值内存或 fsync 成本。它不是性能优化或高可用验收。

尚缺同一 N 成员普通决定、SYSTEM 与 FILE/current/history 的决定后交换和恢复、源 D 与目标可用性的双端读取围栏、单一 `file.history.move` 在冷 D 生命周期准入中的正式发布，以及准确解除 choice。五项共同发布义务全部保留。不得借这片准备入口自动接 SMB、伪造第二个 `file.delete` 收据或把目标正文存在当作 move 成功。

原候选六项集成用例覆盖：独立普通 inode/原 stage 保留/活动数据库及日志不变、冷续作、两次 rename 中断地址、同内容替换 inode 拒绝、已登记缺件不重造、无原记录容器与取消拒绝；通用安装器另有原 old/new 冷复用、缺失 new 拒绝及 intent 已写但未交换的中断用例。


## 本地后继验证

本片新增四项临时根集成测试源码：仅一份原冻结记录时准确补副本、整个安装 generation 丢失不重造、同内容新 inode 连同改写 intent 不替代原安装、提前交换仍拒绝。通用安装器增加 capture/缺失 pending 纯读不创建目录的用例。

2026-11-09 在 `04af3778` 父组合、最终 ENOENT 修正之前的实际结果：

- 原稀疏工作树的首次命令因 workspace 成员 manifest 未检出而停止，未发生编译；随后复用已有完整工作树。
- `cargo check --locked --offline -j1 -p storage-engine --lib` 通过，36.40 秒，无 warning。
- `cargo test --locked --offline -j1 -p storage-engine --test file_history_copy move_ordinary -- --test-threads=1 --nocapture` 完成独立测试二进制构建，耗时 1 分 54 秒；运行结果 **0 通过、10 失败、77 过滤**。十项均在共享夹具的普通首次 commit（`file_history_copy.rs:33`）遇到 ENOENT，尚未进入 move 准备；不得据此声称上述恢复用例已通过。

只读 capture 暴露了 `optional_metadata` 不吞中间目录不存在的契约。最终源码已在 capture 与保留容量核算中按固定两级逐层观察 `.zxs-layout`、`database-installs`，保持只读与其它 I/O 错误传播，不把任意错误视作材料不存在。**此修正及 `e03c1caf`、`90450cf0` 上的组合未再编译或测试**：用户随后要求“本地也不再编译，直接提交”，全部后续本地验证计划已取消。

通用安装器单测、完整套件、Clippy、真实 SMB／磁盘／断电均未执行；未重新触发 GitHub CI。此前格式检查只覆盖当时源码，不替代最终组合的编译或运行。
