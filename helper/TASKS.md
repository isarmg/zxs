# Helper 状态与验收

这些证据只属于所列提交及环境。自动化通过不代表真实服务端、宿主磁盘、安装或发布都已验证。
常用命令见[使用说明](README.md)，原件和权限边界见[架构](ARCHITECTURE.md)。

## 已核证据

| 提交和范围 | 状态 | 可追溯证据 |
| --- | --- | --- |
| `7f249da`，Helper 与 ZXS 独立默认包 | [CI通过] | [Helper](https://github.com/isarmg/zxs/actions/runs/45332479121/job/138236653527)、[ZXS](https://github.com/isarmg/zxs/actions/runs/45332479121/job/138236653813)严格 Clippy、原生测试与第一方格式检查。 |
| `7f249da`，批量导入的独立原生用例 | [CI通过] | 同次两项 Rust 作业覆盖计划上限/封存、原回执优先状态、逐项 ACK、输出失败、结束包和非法票据拒绝，不是两进程端到端。 |
| `7f249da`，默认独立 ELF 构建 | [CI通过] | [Linux 配对构建记录](https://github.com/isarmg/zxs/actions/runs/45332479104/job/138236653339)。它构建并检查两份独立 ELF，不发布或安装它们。 |
| `ca2dc248`，单容器离线核验 | [CI通过] | [ZXS 7 项](https://github.com/isarmg/zxs/actions/runs/45336495507/job/138249700531)、[Helper 4 项](https://github.com/isarmg/zxs/actions/runs/45336495507/job/138249700205)覆盖 DATA/回收原件、正文摘要与终点、原角色绑定及结果帧。 |
| 较早的默认构建和源码检查 | [CI通过] | `85a8988` 的两项作业见[交付](RELEASE.md)；保留 `7d86855` 的[默认构建](https://github.com/isarmg/zxs/actions/runs/45223533959)和[原生作业](https://github.com/isarmg/zxs/actions/runs/45223533974)作为历史证据，不归给当前源码。 |
| 独立依赖材料 | [仅静态] | [依赖](DEPENDENCIES.md)保留 26 份锁定上游源码、许可与对应材料，没有当前发行制品的归属结论。 |

## [仅静态] 功能的目标环境缺口

| 功能 | 下一道验收门槛 |
| --- | --- |
| [首设与密码重置](docs/guides/headless-offline.md) | 验证原路径及角色标记、运行中拒绝重置。停机重置后核对同一登录名与新口令、池和文件不变；覆盖受损清单与中断。 |
| [加盘、初始化、退盘](docs/guides/headless-offline.md#离线加入空数据盘) | 核对相同参数续作、只初始化原目标、空盘前提、[回收回执及原档案](docs/specs/recycle-retirement-receipts.md)。非空、未知暂存或错盘应拒绝。 |
| [源目录预检](docs/guides/import-preflight.md) | 来源不重叠，权限不足/原件变化不报部分结果；真实已登记目标名称必须按原拼写核对。 |
| [导入与恢复](docs/guides/offline-imports.md) | 真实调用一个固定配套 ZXS 进程。覆盖原回执存在而源已缺失、输出断开不推进、部分持久化后按原票重放、目标满与换盘；核验结束包、EOF、退出和来源未变。 |
| [单容器核验](docs/guides/offline-verify.md) | 在隔离实盘核对 DATA 与回收盘重挂后的同一原件；源码用例未覆盖真实重挂。 |
| [服务单元预览](docs/guides/headless-offline.md#可选服务单元预览)、[可选内嵌发布](docs/guides/embedded-image.md) | 实际挂载顺序与前台启动分别验证；另验完整 ELF、无覆盖发布、冷启动、复建一致性与发行材料。默认构建证据不覆盖内嵌版。 |

下次获得运行授权后，用独立临时数据与明确准入的隔离挂载。
若没有合格环境，记录缺项，不以业务数据、实盘改造或跳过失败来补验收。

## [设计中] 尚无完整入口

- 跨 DATA 盘离线迁移整容器。先核验完整目标并确认原路由已持久切换，再考虑源清理或退盘；容器身份和历史保持不变。
- 一次核验全部容器；当前只支持指定一个容器。
- 原目录标签和显式 ZIP 打包。当前文件选择、名称映射和批量导入不自动打包。
- 五类公共入口收敛。可选 ELF 发布和现有命令拼写仍保留，最终入口尚未完成。

旧 SQLite/SMB、[六类镜像工具和本机网页](docs/archive/retired-volume-tools.md)已退出交付。
[仅静态] [六个独立宿主观察入口](docs/archive/retired-host-observation.md)已从本次源码移除。保留离线维护使用的挂载、路径和原件证据。
不承诺旧池转换、就地擦除/接管非空盘，或重建 ACL 和硬链接拓扑。
