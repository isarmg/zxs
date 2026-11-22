# ZXS

ZXS 是 Linux 文件容器存储服务。用 [Web 管理](docs/web-management.md)设置首装选定目录的访问模式、查看版本和手动提交；普通文件读写走内置 Rust SMB 3.1.1 的 `\\服务器\ZXS`。两个入口共用[一个账户](docs/first-account-v1.md)，分别认证、授权。管理界面仅监听 HTTP 0.0.0.0:8443。

## 新安装的默认目录

```text
/zxs/
├── bin/zxs                      可执行文件
├── state/deployment.v1          本机部署选择记录
├── state/<32位小写实例ID>/      site.v1、rust.install、vault/credential.v2
├── log/zxs.log                  前台启动前／结束后的诊断
└── disks/
    ├── system/SYSTEM.sqlite3    池元数据；SQLite 回滚日志在同目录
    ├── data/<卷名>/             已登记普通卷的挂载目录
    └── recycle/                 与 SYSTEM、普通卷分开的专用文件系统
```

由宿主挂载磁盘。[Helper 的 v18 首设入口](../helper/docs/private-fresh-zxs-v18.md)默认关闭，限隔离新池验证。已完成首设时，从任意工作目录无参数运行首设批准的 ELF（默认 `/zxs/bin/zxs`）；ZXS 用受保护的 `/zxs/state/deployment.v1` 选择原站点及其绑定的可执行文件和卷根。程序不创建布局、不从旧目录或环境变量选站点、不迁移旧池。宿主自行管理进程。

若需文件日志，在同一 `/zxs` 文件系统预备 `log/`（root:root 0700）及普通文件 `zxs.log`（root:root 0600、单链接、无 ACL）。无法安全追加时写标准错误。日志只记时间和前台阶段。详见[站点契约](src/smb/launch/site/CONTRACT.md)。

## 文件与存储

- **名称**：普通目录与文件名区分大小写，保留 UTF-8 原拼写。SMB 显示 `方案.xlsx`，底层容器为 `方案-<完整32位FileId>.xlsx`。[命名规则](docs/storage-names.md)
- **保存**：每个 FILE 最多一个写者，兼容读固定已确认视图。每轮首次实际改动新建工作增量，后续保存更新同一稿；手动提交才产生正式版本。断开不会自动确认或发版本。[工作稿与历史](docs/portable-storage.md)
- **回收**：同一 FileId 及历史移入独立 R；按原名称和 DeleteOp 找到该次删除，再还原到已获读写权限的普通空目标。R 不可用时停止相关操作，已核验普通卷可继续。[回收与还原](docs/recycle-and-restore.md)

[SQLite SYSTEM](docs/sqlite-system.md)保存登记、授权、名称、位置、历史关系与操作回执；FILE 容器保存正文和直接索引。存储使用通用文件系统接口，不依赖 reflink 或特定文件系统。架构和职责分工见[顶层设计](ARCHITECTURE.md)与[架构图](docs/architecture-maps.md)。

## 构建与验收

[当前范围与验证](docs/status.md) · [单 ELF 构建](BUILD.md) · [内置 SMB 业务范围](src/smb/BUSINESS.md) · [验收用例](TASKS.md)
