# 逐盘权威的离线存储切片

[存储核心](../components/engine.md) · [离线工具](../components/rescue.md) · [架构决策](../adr/0015-independent-disk-journal.md)

## 当前接线与边界

本分支提供显式选择的 `independent-disk-journal` 格式。真实调用链是 `zxs-rescue authority` → `storage_engine::disk_authority::IndependentDisk` → 本盘持久意图、不可变文件和发布决定。它不是旧池旁的 sidecar，也不借旧池 quorum 决定本盘成功。调用方必须停止所有相关写者，工具只提供本机排它锁，不提供跨主机围栏、在线用户会话或管理面授权。

尚未切换的消费者包括当前 `Engine` 的 State、目录命名空间、文件分支/目录 Git、自动备份、在线恢复、HTTP/SMB 写入和原系统 SQLite。这个切片不能作为已完成生产逐盘架构、外来盘迁入向导或当前池接纳的声明。新格式不读取 managed-directory、membership-generation 或独立协议实验格式，不补 incarnation，不迁移或重置已有目录。

独立盘来源图已新增管理面只读调用链，见[产品只读预览](independent-source-product-preview.md)。这不会切换上述在线写入消费者，候选仍不可直接登记。

## 权威和证明

每盘身份显式保存来源四元组 pool/disk/slot/binding_generation、incarnation 和角色。一级只保存本盘文件历史 heads 及完整父链，不保存二级/三级或下游配置。二级绑定实际核验过的一级 checkpoint，三级绑定实际核验过的二级 checkpoint；随盘保留完整原一级来源。不同来源中的相同 history/file 文本不能混为同一文件。

普通一级写入必须提供完整正文。正文物理键是不可变 commit/version 身份，内容摘要只校验字节；不同历史、文件和后续版本即使内容相同，也各存一份独立 FULL 文件。备份盘另写独立字节，不通过硬链接或内容摘要隐式去重。同一不可变版本被多处引用仍是同一版本，不重复制造版本。相同 history/file 的再次写入连接前一个 head，同时保留其他文件 heads。此图仅覆盖独立文件版本链，不表达当前产品的完整分支、目录 Git、目录位置、跨原来源连通历史或所有权迁入。

备份只从已核验的物理来源读取完整闭包，review 返回拥有字节的不可变计划。执行不再从可变来源路径读取。缺失的历史正文保留 `temporarily_unavailable`，存在但损坏、链接、元数据缺失或身份冲突明确失败。备份目标与任何来源路径双向重叠时，在 intent 写入前拒绝。

审核摘要绑定目标及来源的真实路径、device/inode、稳定物理 mount-root、身份和 checkpoint。目录重叠除名称外还通过已打开 FD 的祖先物理身份检查；同文件系统但不同物理 mount-root 的子目录挂载别名明确拒绝，完整文件系统别名用真实祖先链判定。挂载实例 MNT_ID 不进入持久身份。完整本机选择只存于 `operations/<operation>/local.json`，便携 intent 保存其摘要，不泄露本机路径。移动一个待执行目标不能继续写；移动已发布盘可核验和备份。复制到另一路径的同字节来源不能复用旧审核摘要。数据在 review 时已收齐，因此已 prepared 的恢复不要求旧来源在线。

## 初始化和提交

初始化只接受空私有目录，先原子、不覆盖地保存准确 `initialization.json`，再取得 writer 锁，保存身份和子目录，最后写 `initialized.json`。相同身份和本机目录的已持久初始化意图可以重试；其他身份或移动后的未完成初始化拒绝。完成标记只证明初始化，不代表有文件已发布。

第一份意图的原子创建尚未完成时，可能只剩临时文件；没有可验证意图的未知残留和旧 lock-only 残留保留并拒绝，不隐式清空。本切片不提供这些前缀的人工接管/修复命令，也不宣称每个系统调用之间均可自动恢复。

一次文件/备份发布依次执行：

1. 内存 review 校验闭包、角色、精确选择、预算和基线，不创建 intent 或目标预约。
2. writer 锁内重新校验本盘身份和基线，永久绑定 operation ID、request digest、local review。
3. 独立写入并同步元数据和正文，保存 prepared 证明。未发布候选 publication 不落入普通 objects。
4. 再验本次必须存在的正文和完整闭包后，不覆盖地写 `publications/<sequence>.json`。它是唯一决定。SQLite、prepared 或审核记录均不能替代。
5. 返回不可手工构造的 `DiskPublicationReceipt`。它不是 namespace 导入成功、在线接纳或旧池恢复凭证。

决定后的响应丢失须用原 operation/digest 查询和恢复。只读查询可观察可见的决定，但不补同步，也不替代耐久 ACK；写入型 execute/resume 的已发布重放会用同一个已校验 FD 再同步决定文件和 publications 目录，目录同步仍失败时继续返回错误。`status` 是只读的 Absent/Incomplete/Intent/Prepared/Published 观测；目录存在但 intent 不完整不能解释为 Absent。`resume` 只恢复准确 prepared 或核验已有 publication。未 prepared 的 operation 仍需原审核材料重新 review，摘要必须不变。

## 系统投影

`rebuild-projection` 从明确选定的实物盘完整核验结果构建真实 SQLite，数据盘不写入。目的目录必须位于已存在的私有父目录下，并独立于所有数据根；没有专用格式标记的非空目录拒绝，不能覆盖旧 Engine index。首次选择即持有原父目录和已存在目标的 FD，来源核验结束后只相对该父目录排它创建原先缺失的子目录，期间出现同名目录明确拒绝；实际打开的目标还要重新与保留来源 FD 的物理祖先身份排除重叠，再写任何标记或缓存。写入后读取结果仍保持同一个目标目录，不按路径重新选择。SQLite 只在内存中构建和事务提交，序列化为有界字节并显式关闭后写入持有目录内的私有文件，同步后原子替换 `cache.db`。缓存删除或损坏可重建，任何缓存字段都不能反向生成写入或发布能力。

SQLite 读取先从已持有且验证的 FD 读取有界字节，再以只读 deserialize 导入内存；不把路径或 `/proc/self/fd` 别名交给 SQLite 重开。文件大小、行数、单行/值、SQL 和虚拟机均有限额，外部 WAL 不参与读取；被改成 WAL 布局的主文件可以拒绝并要求重建。聚合前限制总保留内存和行数，不能先收集任意数量完整盘再计算上限。数据盘核验使用只读 FD；系统缓存目录仍要求可写。

## 命令

`identity.json` 输入必须显式包含 origin、incarnation、role。`selection.json` 输入必须包含 path、identity、known_checkpoint，后者可为 null。已知 checkpoint 在持有对象后每次读取、写入和恢复验证中继续约束祖先链，不代表能证明全局最新。首次观察只证明所选盘最后完整本地 checkpoint。

```sh
# 只初始化新空目录，不转换当前运行池。
zxs-rescue authority initialize /srv/new-independent-disk identity.json --accept-new-format
zxs-rescue authority inspect selection.json

# 先只读审核，再以完全一致摘要显式执行。
zxs-rescue authority review-file selection.json OPERATION_ID \
  --base empty --history HISTORY --file FILE input.bin
zxs-rescue authority publish-file selection.json OPERATION_ID \
  --base empty --history HISTORY --file FILE input.bin \
  --request-digest REVIEWED_DIGEST --confirm-write

zxs-rescue authority review-backup backup-selection.json OPERATION_ID \
  --base empty source-selections.json
zxs-rescue authority publish-backup backup-selection.json OPERATION_ID \
  --base empty source-selections.json --request-digest REVIEWED_DIGEST --confirm-write

zxs-rescue authority status selection.json OPERATION_ID
zxs-rescue authority resume selection.json OPERATION_ID \
  --request-digest REVIEWED_DIGEST --confirm-write
# 父目录须预先存在、由当前用户持有且权限为 0700；命令不会替用户改权限。
zxs-rescue authority rebuild-projection /srv/private-projections/cache selections.json
zxs-rescue authority export selection.json COMMIT_DIGEST /srv/export/new-file
```

`--base empty` 只用于零发布盘；否则提供准确本盘 checkpoint。输入变化、目标推进或物理来源选择变化都要求新 review。export 先核验完整元数据发布链、身份和已知 pin，证明所选 commit 可达且 owner/history/file/body 绑定正确，再只读验该版本的独立 FULL 文件并拒绝覆盖。未读取的其他正文不会输出可用性或产生写能力；report、备份审核和写入/恢复仍保持完整正文核验。缺正文返回暂不可用错误，不创建空文件。

## 资源、故障与后续接入

单个可达闭包硬上限是 4096 对象、16 MiB 元数据与正文、256 层遍历/本盘发布。写前遍历并计入所有保留对象、operation 和孤儿/临时残留，盘上保留预算为 64 MiB 和 32768 条目；还保守计入整个候选、证据及一个最大文件的临时写空间。失败或停止不会释放这些费用，也不自动删除残留。这是有界切片的容量门禁，不是持久预约保证、增量大型盘算法或性能承诺；低容量下即使幂等重试也可能拒绝，不能以自动清理绕过。

`fault-injection` 特性提供明确的持久阶段故障点，默认产品构建不启用。验证覆盖初始化 intent/identity/directories/completion、发布 intent/material/prepared/decision、真实子进程退出和缓存发布前退出。完成阶段故障、模拟 I/O 返回和真正断电是不同证据；每个 write/fsync/rename 切点、只读挂载、跨机竞争和实盘断电仍未验收。

CI 的既有 workspace I/O 链调用[独立盘发布验收入口](../../.github/scripts/check-independent-disk-publication-io.py)，从本次 Cargo 输出选择唯一当前 libtest，再运行[真实 fsync 驱动](../../test/engine/unit/disk_authority/publication-io.sh)。这个 ignored 用例必须由该入口明确执行；缺少注入完成标记、零执行、重复完成、子进程失败或缺少当前产物均使门禁失败，不按权限或环境静默跳过。

后续在线接入必须先给新格式定义真实 owner/会话授权、跨主机写者隔离、完整产品历史和 namespace 发布、保护目标与跨盘完成判定、目录转换和恢复准入。三级转二级还必须证明所选当前一级已接纳全部将声明为其备份的历史。不能把原一级来源四元组直接替换成任选的当前一级。

## 初始化权威互斥

独立格式初始化与当前 Engine 的 SYSTEM、数据目录初始化共用实际目录上的排他锁，并保持目录锁先于各自 writer 锁的顺序。当前 Engine 在写入任何数据盘锁文件前，对全部已持有数据根要求为空；独立格式的未完成 intent、已完成空盘、正文和未知残留均保留并拒绝，不转作旧池。SYSTEM 准入同样拒绝独立盘 identity／初始化记录，以及独立在线 selector／围栏高水位残留，不能因 selector 缺失改选旧权威。TerminalManagement 和恢复入口共用该选择锁。

这些是本机合作写者的格式互斥边界，不是跨主机 fencing、强制接管或物理盘独立性证明。初始目录锁不新增文件或持久化屏障；既有 publication 的同步次序不变。回归覆盖双向 SYSTEM／数据角色误选、初始化持久阶段、全部目标先准入、已有字节不变，以及真实线程同时争抢同一目录。

发布成功与旧回执重放在耐久屏障后重新核验完整链，保留屏障前 checkpoint，并要求准确回执及本次承诺的全部 FULL 仍可验证。复用的不可变文件还从同一已持有 FD 有界重读字节；同 inode、长度不变或 fsync 成功不能代替内容证明。新增的是有界读取／哈希开销，不减少或新增 fsync 阶段。`publication-io.sh` 同时运行重放期间真实文件改写／删除注入，覆盖 execute、resume 的决定、manifest、FULL、所需正文缺失和较新 checkpoint 消失；这些临时目录测试不是物理断电或恶意内核证明。
