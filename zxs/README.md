# 文件容器存储服务

服务保存单个文件的完整版本、分支和标签。首次保存创建 `main` 和第一节点；提交当前分支头追加一节点，从 `main` 的历史节点提交则创建新分支，保留 `main`。删去文件时，整个容器移入专用回收盘。

在 [Workspace 桌面应用](../zxs-workspace/docs/container-management.md)里访问和管理容器。使用 [Helper](../helper/README.md)设置部署。服务器只提供文件容器协议，不出管理页面，不运行 SMB。ZXS 在你指定的地址提供 HTTP，不选择或配置反向代理。Workspace 的外部 HTTPS 连接只支持 HTTP/2。

- [架构和操作](ARCHITECTURE.md)
- [构建](BUILD.md)
- [启动 HTTP 服务](docs/headless-activation.md)
- [交付和验收](TASKS.md)
