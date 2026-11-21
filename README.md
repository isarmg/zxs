# ZXS 源码仓库

本仓库包含三个独立项目。各自拥有源码、构建配置、文档、任务清单和测试；没有根级构建、共享依赖图或联合发布。

| 项目 | 用途 | 文档 |
| --- | --- | --- |
| [`zxs/`](zxs/) | SMB 文件共享、手动版本、跨盘移动、回收及管理 Web | [使用](zxs/README.md) · [架构](zxs/ARCHITECTURE.md) · [任务](zxs/TASKS.md) |
| [`helper/`](helper/) | 首次部署、离线维护和磁盘诊断 | [使用](helper/README.md) · [架构](helper/ARCHITECTURE.md) · [任务](helper/TASKS.md) |
| [`zxs-workspace/`](zxs-workspace/) | 整目录 ZIP 版本、工作树、命令行和桌面界面 | [使用](zxs-workspace/README.md) · [架构](zxs-workspace/ARCHITECTURE.md) · [任务](zxs-workspace/TASKS.md) |

开发时进入对应的项目目录。单独取出任一项目不需要另外两个项目的源码、进程或私有数据。各自任务清单说明哪些功能已写、哪些仍待实现，以及哪些验收没有运行。

**SMB：** 只维护编入同一个 `zxs` 二进制的内置 Rust SMB 3.1.1，服务于 ZXS 的文件共享。不另发 SMB 服务程序，不要求部署 Samba 服务端，不提供通用 SMB 服务或后备实现。当前是面向新池的源码候选，不是已发布的安装包，也不代表 Windows、macOS 或 Linux 客户端已通过验收；不迁移或接管旧实现的既有池。见 [ZXS 任务清单](zxs/TASKS.md)。

## 历史资料

[整理前的旧工程快照](https://github.com/isarmg/zxs/tree/8c385b1ac514fffc4e0b98b530ecbf8b1e7f597e)保留原源码、文档、测试、脚本、演示和部署配置，包括[旧 GitHub Actions](https://github.com/isarmg/zxs/tree/8c385b1ac514fffc4e0b98b530ecbf8b1e7f597e/.github)及[许可证与来源记录](https://github.com/isarmg/zxs/tree/8c385b1ac514fffc4e0b98b530ecbf8b1e7f597e/compliance)。[拆分前的任务清单](https://github.com/isarmg/zxs/blob/21a24b18fcb9e440b9438f5163cc663c4e36392c/TASKS.md)供追溯任务与历史证据。这些不是现行构建、安装或验收说明，不应据此操作现有磁盘。
