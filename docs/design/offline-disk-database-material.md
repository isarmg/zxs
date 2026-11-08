# 离线盘根数据库材料与恢复结果

[辅助工具](../components/helper.md) · [当前目录](disk-current-catalog.md) · [本盘历史](disk-local-publication-history.md) · [系统全局库](system-change-database.md)

## 当前切片

`inspect_disk_database_material(directory, path, expected_identity, scope)` 是实际 Engine 库只读入口；`zxs-helper disk-databases SELECTION` 是其辅助工具 CLI 调用方。它不打开 SYSTEM 配置或索引、不构造 Engine、不扫描其他磁盘，也不安装或修复数据库。必须先停止全部写者；CLI 持有当前 ZXS 二进制共用的 `/zxs` 生命周期租约，旧二进制、其它主机或未参与该租约的写者仍须手动停止。

选择文件由当前用户拥有，有 64 KiB 上限，示意如下（身份须替换成明确选择的真实登记，不根据数据库猜测）：

```json
{
  "root": "/mnt/data-one",
  "expected_identity": {
    "pool_id": "pool_11111111111111111111111111111111",
    "disk_id": "disk_22222222222222222222222222222222",
    "slot_id": "data-one",
    "binding_generation": "1"
  }
}
```

```sh
zxs-helper disk-databases selected-disk.json
zxs-helper disk-databases selected-disk.json --scope history --records
```

默认 `--scope both` 读取盘根 `.zxs-disk.sqlite` 和 `.zxs-disk-history.sqlite`。可明确选择 `current` 或 `history` 以读取仍完好的一份，另一份不访问、不判断缺失、不修复。被选择的文件只允许真实缺失，不把损坏、权限或 I/O 错误当作没有文件；所有所选文件均缺失时拒绝，不返回“空盘已恢复”。

默认输出清单摘要。`--records` 在整个受限读取成功后输出 JSON 行，包括摘要、当前版本/目录行、历史变化行和由历史折叠重建的版本/目录行，最后才写 `complete:true`。写 stdout 失败保留错误；输出不是持久恢复收据，也不会在服务器/源盘写文件。可保留此材料用于后续明确的救援判断，不能直接拿它覆盖运行目录。

## 精确范围与内部一致性

目标目录先持有安全 FD。现有盘识别器校验同一 FD 的 seal、物理目录身份和本地配置声明，必须等于调用方提供的 pool/disk/slot/binding。它不因为本地声明就认证全局权威。SQLite 仍在内存连接中解码；严格 schema、scope、规范表示、行数/字节上限和内部摘要复用当前格式实现。结束时重新核原文件 FD 对应的 inode/字节摘要、所选缺失文件仍缺失，以及具名目录/seal/角色没有变化。没有写入、清理、兼容升级或新授权能力。

历史不只检查最后一行：从空集合逐条执行 Entered/Changed/Departed。进入时不得已有同版本；变化/离开必须精确等于完整前置本盘行。一个 sequence 内的 operation/candidate/snapshot 必须相同，一个 operation 不得占多个变化序号；已发生变化的 publication 分组不得超出 header 计数，最终变化恰在覆盖边界时必须匹配边界 snapshot。哈希和 genesis/非空边界的表示必须有效。重建结果保留完整本盘版本/owner 路径和逻辑内容状态，不从最终 placement 给旧事件重新分盘。

如果当前库声明序号不晚于历史库，折叠到该序号时必须逐行等于当前库；不是只比较计数或二者最后序号。若当前库比历史新，返回 `history-behind-current`，不能推断缺失的后段没有变化。只有历史时可以在内存恢复其覆盖边界的清单表示，**不安装一份新的当前库**。只有当前时不伪造过去变化。返回 `scope` 明确区分真实缺失和用户未选择检查。

## 不能据此取得的保证

此入口只有本地格式和内部前后变化闭合，不认证原始 candidate/publication 链。被恶意整体重写且内部一致的材料可能通过；原发布缺失的变化也不能仅靠自摘要检测。因此结果明确 `source_publications_verified:false`、`content_availability_verified:false`、`authority_restored:false`。它不授予读正文、复活退役节点、注册外来盘、选择新权威、运行池初始化或三范围多数表决的权限。

现有 `disk_history_observation` 仍使用完整已准入来源重算；新增离线入口不替代它。系统盘保存全局、盘根保存本盘、文件/目录内部保存本对象的范围仍不同，不将三份数据库简单复制或用字节相等选权威。完整目录/标签/alias 历史、代际归档、独立三范围权威及实际内容恢复还未完成。

## 原 restore-system 的后置派生恢复

原 `restore_from_disks*` 在元数据成员的原权威准入、恢复、工作区和系统索引完成后，现在调用盘根当前库与历史库维护。只使用已准入配置/持有资源，未明确提供或不可用的盘保留实际失败，不打开不可信新根。独立 `PreservePending` 读取和首次初始化不会借本接口新增这些自动写入。

这不是先从 SQLite 选择权威。每盘使用原安全 installer，保留旧物和未知物；派生安装失败不撤销已完成的权威恢复，也不伪报整套数据库已恢复。辅助工具输出保留 `restored:true`，同时明确 `restored_scope:authoritative-system-state`、`all_database_scopes_restored:false`，并分别提供系统派生索引、当前库和历史库的真实维护状态/错误链。用户不能因某一可选库失败而删除恢复目录或换新操作号重做原决定。

## 资源与交付边界

每个 SQLite 文件最多 32 MiB、规范行最多 24 MiB；历史最多 65,536 条变化、2,048 个 publication。离线折叠 O(变化数 × log 当前版本数)，保留完整有界变化、当前/重建清单以及最多两组 SQLite 解析瞬态，不是总 RSS 上限。文件重核会再次读取有界字节；没有并发快照/所有外部写者已停止的额外保证。

2026-11-08：仅写入生产源码、CLI 和文档；按用户要求未进行审查、编译、测试、静态检查或 CI。源码中的调用/协议边界不是运行通过证据。

## 空目录材料

当前新格式和历史 fold 同时保留已发布原生递归目录副本，版本数为零不再等于没有本盘目录。helper 的摘要增加 `backup_directories` 计数，`--records` 输出 `current-backup-directory` 及 `history-inventory-backup-directory`；身份、路径、来源代表与当前覆盖一起返回，不在源盘建目录。完整范围、旧字节保全及未验证状态见[目录范围规范](disk-backup-directory-catalog.md)。本片没有扩大正式恢复或 authority-restored 保证。

## 对象内部库的独立材料

盘根 current/history 与文件/目录内部库仍是不同范围。后继 [对象库救援归档与重建](object-database-material-reconstruction.md) 只处理明确选中的单对象SQLite，允许从已保留归档生成独立救援镜像；不依据盘根owner路径选对象、不安装活动库、不授予本入口新的系统恢复权威。
