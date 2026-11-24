# ZXS

按文件保存完整版本、标签和回收记录的存储套件。各项目独立构建。

| 项目 | 职责 | 文档 |
| --- | --- | --- |
| [ZXS 服务端](zxs/) | 保存文件容器、版本、标签和回收记录 | [上手](zxs/README.md) · [架构](zxs/ARCHITECTURE.md) · [状态](zxs/TASKS.md) |
| [Helper](helper/) | 首设、磁盘维护、离线导入和密码重置 | [上手](helper/README.md) · [架构](helper/ARCHITECTURE.md) · [状态](helper/TASKS.md) |
| [Rust 客户端核心](zxs-core/) | 新版普通目录编辑、显式提交和标准 ZIP 打包 | [上手与状态](zxs-core/README.md) |
| [Workspace](zxs-workspace/) | 现有桌面和移动端客户端，本地 ZIP 工作区在迁移范围内 | [上手](zxs-workspace/README.md) · [架构](zxs-workspace/ARCHITECTURE.md) · [状态](zxs-workspace/TASKS.md) |
