# 救援数据库发表后与冷态核验

[对象库救援](object-database-material-reconstruction.md) · [盘根救援](disk-database-rescue-reconstruction.md) · [辅助工具](../components/helper.md)

## 本片解决的缺口

原两种重建在内存生成 SQLite 后已重新解码完整记录，输出通过无覆盖写入和 fsync 持久发表。本片再把**实际发表后的文件**与选中的完整救援材料核对，并提供进程重启后的只读 `verify`，不把“内存结果正确”或“输出文件存在”直接报告为实际文件已核验。

范围包括文件内部、目录内部、盘根 current 和盘根 history 的救援镜像。SYSTEM 权威恢复、原 Config/registry/publication、正文、外部引用和三范围共同活动安装不在此片；这不是三个同字节副本的表决协议。不修改普通 Engine 或备份写者，不开放外来盘正式接纳。

## Engine 语义核验

`ObjectDatabaseReconstruction` 与 `DiskDatabaseReconstruction` 新增：

- `maximum_database_bytes()`：该范围的既有 SQLite 上限。
- `verify_database_bytes(bytes)`：严格有界解码一份独立读回的 SQLite，逐项对比先前从准确归档重建出的完整记录及 header。

对象核对 file/history 或 repository/directory 身份、原 sequence、全部本地行和内部摘要，不因为新库同对象但更新就接受。盘根重建结果保留所选完整 snapshot，而非只保留 hash/计数：身份、明确 source、覆盖序号、所有版本/目录/变化均按选择核验。相同记录但不同 sequence、current 与 history 混淆、记录被补删、不同对象或错误 schema 都拒绝。

SQLite 页布局本身不作为记录身份。允许编码后页字节不同但完整规范结构/记录相同的独立镜像；返回的 SHA-256 则明确记录这次实际读取的文件字节。上述方法不读文件、不创建临时 SQLite/WAL，也不返回安装或来源准入能力。

## helper 实际文件事务

两个 `rebuild` 现在执行：

1. 按现有输入上限解码明确身份/范围及完整归档，构造原语义重建结果。
2. 只允许新的 `.recovered.sqlite` 名称；固定安全父目录 FD，确认所有同名 `-wal`、`-shm`、`-journal` 均不存在。
3. 按原 0600 临时文件、完整写入、文件同步、无覆盖 rename、父目录同步发表，保留发表器返回的准确 device/inode 及原文件 FD，直到读回完成，避免发表与首次读回之间发生 inode 回收复用。
4. 从同一父目录打开实际新文件，必须仍是该准确 inode。文件须由当前用户拥有、单链接、普通文件、受界限约束且无特殊位/组或其它用户可写权限。
5. 持有原文件 FD，记录身份、长度、owner/mode/link、mtime/ctime，完整读取；读取前后这些信息必须相同。调用 Engine 做全量语义核验。
6. 核验后从同一原 FD 再完整读取，SHA-256 和属性必须不变；重新打开具名文件须仍是原 inode/属性，再核无侧车、调用方具名父目录仍指向原持有目录。
7. 仅全部成功才报告 `file_readback_verified:true`。报告包含所用归档字节的 `archive_sha256`、实际镜像字节的 `readback.sha256`、长度、原父目录和镜像 device/inode，以及原语义 scope/source/sequence/records。

父目录可为明确选择的公共输出目录，但不通过重开可变父路径偷换发表目标。最终具名父目录核验包括调用方选择的父路径别名；别名改指或目录换名后没有完整成功报告。发布器由 `platform-io::publication` 单一实现提供；既有消费者仍用原返回接口及原行为，没有因此自动新增权限或改动。

**发表成功但读回失败不是“什么都没发生”。** 报错会明确已发表的路径/device/inode、读回未完成及原错误链，保留输出及所有未知材料，不撤销、不覆盖、不重新生成。此前 rename/fsync 本身的未知结果继续走原持久发表错误，不伪装成可安全重试。

这些检查描述读回事务的观察边界，不是永久证书或跨进程写锁。操作者应停止一切会改写该救援副本的程序。无侧车不证明永远没有外部写者，成功后的外部修改也不在上一次报告保证内。

## 冷态只读 verify

```sh
zxs-helper object-database verify file.object-db.json file-scope.json file.recovered.sqlite
zxs-helper object-database verify directory.object-db.json directory-scope.json directory.recovered.sqlite
zxs-helper disk-database-rescue verify disk.disk-db.json disk-identity.json --source stored-current current.recovered.sqlite
zxs-helper disk-database-rescue verify disk.disk-db.json disk-identity.json --source history history.recovered.sqlite
zxs-helper disk-database-rescue verify disk.disk-db.json disk-identity.json --source current-from-history folded.recovered.sqlite
```

verify 重新从完整归档和明确身份/来源构造准确期望，再对现有镜像执行同一冻结读回。它不依赖上一次进程或报告，不要求原 SYSTEM/源盘存在，不开 ZXS 服务租约，不创建或修复输出、不清理侧车，也不更新任务/authority 标记。输出标明 `files_modified:false`。活动数据库名仍不允许借此入口被选中；正常管理目录应使用对应独立 inspect 或原准入路径。

镜像或归档缺失、损坏、范围不合、侧车出现、硬链接/符号链接、预算超限或选择变动时拒绝。即使上一轮重建已成功，也不因这个事实自动重建丢失文件。需要恢复新副本时仍由操作者明确选择新的输出名。

## 认证与资源边界

归档 SHA-256 表示本次输入的确切字节，语义 records hash 表示明确范围的本地记录，镜像 SHA-256 表示本次实际文件字节，三者不能互换。它们都不是原始 publication 签名/认证，也不证明永久退役和正文可用性。所有成功结果继续声明 `source_publications_verified:false`、`content_availability_verified:false`、`authority_restored:false`、`active_database_installed:false`。

现有归档/行数/SQL 上限不变，镜像文件对象库至多 16 MiB、盘根至多 32 MiB。每次冻结读回增加两次有界实际文件读取、两次 SHA-256，以及一次完整 SQLite 解码；两次文件读取先后进行，不同时保留两个读回 byte buffer。盘根重建结果现在保留一个所选语义 snapshot，已有归档、重建 SQLite 和 SQL 解码瞬态仍可能同时存在，单项配额不是总 RSS 保证。只在显式 helper 救援命令执行，无后台或每次业务写入的额外扫描。

## 交付事实

原候选 `17fcb1d8` 基于 `7583e229` 写入十四项测试源码，当时按要求未执行检查。2026-11-09，本片对齐已发布 `5e9bb6a9`，沿用迁移后的平台单一发布器，补原 FD 持有回执及一项回归，不恢复 helper 中的旧发布实现。定向检查发现并修正 SHA-256 输出类型不支持 LowerHex 的编译问题，改用既有 `platform_io::lower_hex`；原目录失盘夹具明确创建 0700 私有目录，不放宽生产准入。

- 通过：`cargo check --locked --offline -p storage-helper --bin zxs-helper`，包含所依赖的生产 Engine/平台目标。
- 通过：8 项 helper 冻结文件读回、6 项对象/盘根 CLI 准入、5 项平台发布保全/FD 回执、3 项目录对象归档/读回，共 22 项 Rust 定向用例；均仅临时目录。
- 通过：架构门禁、来源 verify、Foundation 消费者与文档索引 verify、17 项发布资产和 6 项维护文档 Python 回归，以及差异空白检查。新增读回文件及平台改动做过局部格式化，不代表全仓格式通过。
- 未执行：文件内部完整记录与盘根 sequence/库族的三项新增 Engine libtest 用例。已发布基线存在另行修复中的终态退役测试旧接口编译错误，本片不混入该独立修复，也未借别的候选的测试二进制宣称通过。
- 未对本候选运行 GitHub CI、全仓测试、真实硬盘失联/掉电、CLI 整进程重建或安装验收；没有操作真实磁盘。

十五项新增用例中十二项已执行，三项 Engine libtest 仍仅源码。仍未完成任意单盘 SYSTEM 恢复、无归档完整重建、三范围共同准入或活动切换。

2026-11-09 后续合流：保留原 `75153227` 候选，将此片单独对齐已发布 `36e6a883`，合并最新 formatter 与平台 BundleState/原配置恢复增量。按最新要求没有本地编译、测试构建或新 CI；上面的 22 项等结果仅属于原 `5e9bb6a9` 组合，不是这次合流的运行证据。独立 SQLite 物理完整性候选 `483d3a26` 未包含于此片。
