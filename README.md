# ZXS

| 项目 | 用途 | 文档 |
| --- | --- | --- |
| [ZXS](zxs/) | SMB 3.1.1 文件共享、手动版本、回收和 Web 管理 | [使用](zxs/README.md) · [架构](zxs/ARCHITECTURE.md) · [任务](zxs/TASKS.md) |
| [Helper](helper/) | 首次部署、系统与磁盘状态、离线维护 | [使用](helper/README.md) · [架构](helper/ARCHITECTURE.md) · [任务](helper/TASKS.md) |
| [Workspace](zxs-workspace/) | 整目录 ZIP 版本和工作树，提供命令行与桌面界面 | [使用](zxs-workspace/README.md) · [架构](zxs-workspace/ARCHITECTURE.md) · [任务](zxs-workspace/TASKS.md) |

三个项目分别构建。ZXS 的 SMB 和 Web 编入同一个可执行文件，不需要 Samba 服务端；Workspace 的 `.zxs` 文件是 Store ZIP。

ZXS 当前只面向新池。与 Helper 的 [Linux 单机联合验收](zxs/docs/web-and-smb.md#本轮正式验收)：未运行。普通文件跨数据盘移动尚未接入 SMB。

[历史快照](https://github.com/isarmg/zxs/tree/8c385b1ac514fffc4e0b98b530ecbf8b1e7f597e) · [旧任务清单](https://github.com/isarmg/zxs/blob/21a24b18fcb9e440b9438f5163cc663c4e36392c/TASKS.md) · [历史许可证与来源](https://github.com/isarmg/zxs/tree/8c385b1ac514fffc4e0b98b530ecbf8b1e7f597e/compliance)
