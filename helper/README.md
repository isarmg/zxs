# Helper

Helper 是 Linux x86_64 命令行程序，负责 ZXS 首设、离线盘管理、文件导入和密码重置。不常驻、不启动服务，不格式化或删除原盘。存储业务由 [ZXS](../zxs/README.md)负责。

## 首设和离线维护

先挂载所选目录；按 [ZXS 部署要求](../zxs/docs/guides/headless-activation.md)准备专用非 root 服务账号。初始选择一个 SYSTEM、一个 RECYCLE 和 1–62 个 DATA；后续按原登记顺序给出全部目录。

```sh
helper setup-headless check SYSTEM RECYCLE DATA [DATA ...]
helper setup-headless new SYSTEM RECYCLE DATA [DATA ...] LOGIN
```

首设在同一前台终端展示选定目录，键入 `SETUP` 后两次输入不回显的密码。成功只发布角色标记和配置 `/zxs/state/headless/server.conf`。不确定时保留原件，先核对，不覆盖或换目录盲试。

[离线维护指南](docs/guides/headless-offline.md)包含系统服务单元文本预览、加盘、初始化已登记空盘、退空盘和密码重置。维护前停业务服务，用原账号运行。首设后以同一账号运行 `/zxs/bin/zxs --listen IP:PORT`。

把一个活动容器移到另一块已登记的 DATA，见[停机移盘指南](docs/guides/offline-data-move.md)。

## 导入外部文件

[文件导入指南](docs/guides/offline-imports.md)包含单文件、明确映射的批量、整目录选择及重放示例。先准备、审阅并同步保存原计划，再离线应用；部分完成时只用同一份原计划重试。不会覆盖同名容器，不删除原件，不就地接管非空盘。

## 可选内嵌发布

[内嵌发布](docs/guides/embedded-image.md)仅在特定构建提供，独立验证原 ZXS ELF 和发布目标。

## 开发与交付

Helper 有独立 Cargo 工程、锁文件和工具链。见[架构](ARCHITECTURE.md)、[任务](TASKS.md)、[依赖](DEPENDENCIES.md)和[交付证据](RELEASE.md)。
维护者可继续查看[角色、锁和原件](docs/invariants/offline-authority.md)。源码检查不等于实机验收。
