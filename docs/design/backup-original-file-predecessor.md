# Backup 切换：如何认定原盘 FILE 的前件

**状态：待集成、待测试。** 原盘上的 FILE 不一定在原池最后一次发布时被写过。仅验证行集相同、库头序列不大于原池序列，会接受来源错误但格式完全有效的 SQLite。本说明记录跨分支的接线和验收条件，不启用安装、选盘、投票或恢复，不改变盘上格式。

## 两侧的准确来源

- 生产证据：Core `47933769b12c24edd77e8cbd0de341bcad683b7c` 及其路由修正 `a4be88281200e8ffdbafb5dc5cdc4fa52ce5f9c4`。后者的 `src/engine/src/engine/backup_preparation/file_epochs.rs` 按已认证原生发布前缀和实际普通 FILE 写入路由，记录每一已登记实物的最后写入序列、行集摘要、历史身份和落点。仅有登记、其他盘写入或修复请求都不能建立该盘的新写入证据。`PublishedBackupOriginal::require_file_predecessor` 在首笔 Backup Candidate 绑定的原盘范围内核对它。
- 消费者：Backup `892f5abcee851fbf3cda34b9034ff942dc38b181`；以下两个输入 blob 到 `5ba62efbdcfafcd0daea7fabbe4059f1139a0b76` 未变。`src/engine/src/container_storage/file_database/material.rs` 为 `47078add0a32d8c76f7e4fc9b034eef18aa56ba1`；`src/engine/src/container_storage/ordinary_preparation/backup/originals.rs` 为 `d5885203156d0ed99633e1cce5f7fa754dc2f5f8`。
- 回归夹具：上述 Core 路由修正中的 `test/engine/unit/backup_original_published_tests.rs`，blob `484eb1deeda85a39ae13d5812813572a757e7024`。仅扩展 `original_file_epoch_is_not_the_latest_whole_pool_sequence`；保留 Backup 同文件中对缺失保留原件、同字节不同 inode 和无覆盖续作的断言，不整文件替换。

## 接线

1. `require_backup_predecessor` 保留原有 SQLite 严格解码、文件和历史身份、序列范围及冻结原行集核对。通过后把**实际读出的** `observed.sequence` 与 `&observed.records_hash` 交给参数 `require_writer: impl FnOnce(u64, &str) -> Result<()>`，返回它的结果；不使用全池序列或新库头代替。
2. `BackupOriginalImage::capture` 向它传入 `|sequence, rows| source.require_file_predecessor(id, sequence, rows)`。沿用 `ExistingExchange::capture`：未交换时检查持有的原命名 inode；已交换时，命名 inode 须是同一交换记录的新件，并对该交换实际保留的原 inode 调用同一检查。仅有同字节新件、缺原件、换 inode 或借用别盘前件均不能补证。
3. 保留原根、来源链、FILE 容器、交换记录和两件实物的读回及身份核对。该证明只授权核对前件；不是从投影造出可安装的原库，也不替代整组完成记录、三范围恢复或首笔发布。

## 回归判据

真实夹具先写两个普通 FILE，再另建一个无关目录。读出一个原盘 FILE 的实际旧字节及序列。临时把同一 inode 的字节替换为从冻结原 State 准备的完整同作用域 SQLite，独立读取并断言所有行及摘要均相同、库头已变成无关操作较大的全池序列。此时真正的 `BackupOriginalImage::capture` 必须拒绝，整棵夹具的目录、inode 和字节不得变化。写回保存的实际原字节后，同一来源必须重新通过只读 capture。另保留原来对已交换保留件、精确重试和不属于该盘的写入路由的核对。

没有运行该夹具、构建或 CI；不能据此认定全池切换通过或原 900 秒问题已解决。
