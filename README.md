# ZXS

| 项目 | 用途 | 文档 |
| --- | --- | --- |
| [存储服务](zxs/) | 保存文件容器、版本和标签；删除移入专用回收盘 | [使用](zxs/README.md) · [架构](zxs/ARCHITECTURE.md) · [任务](zxs/TASKS.md) |
| [Helper](helper/) | 初始化服务、系统与磁盘诊断、离线维护 | [使用](helper/README.md) · [架构](helper/ARCHITECTURE.md) · [任务](helper/TASKS.md) |
| [Workspace](zxs-workspace/) | 桌面管理、虚拟磁盘、编辑及提交；也可独立管理本地 ZIP 工作区 | [使用](zxs-workspace/README.md) · [架构](zxs-workspace/ARCHITECTURE.md) · [任务](zxs-workspace/TASKS.md) |

三个项目分别构建。桌面应用使用 HTTPS 调用存储服务的文件容器协议。文件改动在本机进行，显式提交时逐个上传有新正文的文件，管理界面在桌面应用内。独立的本地 ZIP 工作区和远程容器提交是两个入口。
