# ZXS 交付与验收

给维护者：以下按功能列出运行证据和待验边界。不同提交、不同环境的结果不能相互替代。

## 已有证据

| 验证范围 | 证据和限制 |
| --- | --- |
| [跨 DATA 离线移动](../helper/docs/guides/offline-data-move.md) | [CI通过] [`559820a` ZXS 源码作业](https://github.com/isarmg/zxs/actions/runs/45396945893/job/138455127052)。另见两项目源码与合并相同的[隔离 ext4 验收](../helper/TASKS.md#安装后的普通迁移)，未验安装后故障注入或掉电。 |
| 代码检查与普通 Linux 夹具 | [CI通过] [`85a8988` ZXS 源码作业](https://github.com/isarmg/zxs/actions/runs/45331407209/job/138233242638)：严格 Clippy、格式检查、78 个单元及 2 个入口用例通过；7 个需另配环境的用例未运行。 |
| 固定批量导入 | [CI通过] [`7f249da` ZXS](https://github.com/isarmg/zxs/actions/runs/45332479121/job/138236653813)与 [Helper](https://github.com/isarmg/zxs/actions/runs/45332479121/job/138236653527)的严格 Clippy、原生测试及格式检查。只证明各自的测试范围，未做跨进程故障恢复。 |
| [正文 Range 读取](docs/specs/payload-range.md) | [CI通过] [`090e76d` ZXS 源码作业](https://github.com/isarmg/zxs/actions/runs/45337302724/job/138252297285)的 7 个用例覆盖 Range 解析、原 R3 路由隔离、鉴权和响应边界。未验安装后的真实 HTTPS/2 读回。 |
| 独立构建 | [CI通过] [`7f249da` 的独立 ELF 构建和检查](https://github.com/isarmg/zxs/actions/runs/45332479104/job/138236653339)。这不证明已安装、发布或签名。 |
| 旧提交的隔离客机 | [CI通过] [`7a88536` 的通过阶段](https://github.com/isarmg/zxs/actions/runs/45322511258/job/138204176887)：普通账号首设、全量写入、同号续查、双 DATA 和回收，以及实际 TLS/HTTP/2 全量写入读回。后续 FUSE 阶段失败；缺盘冷启动与重挂未执行。 |
| [Linux CLI 分块续传](docs/guides/resumable-upload-installed-qualification.md#已核对的-cli-客机验收) | `d75a1fe` 安装在无网卡 Ubuntu/ext4 客机，收到首块确认后断线、首块确认答复完全丢失，两种情况均按原号续传。回环 HTTPS/2 上传，独立 HTTP/1.1 逐字节读回；同尺寸的默认 Basic 整传仍只发一次。 |
| [Linux CLI 单块流式上传](docs/guides/resumable-upload-installed-qualification.md#已核对的单块流式上传) | `d75a1fe` 在无网卡 Ubuntu/ext4 客机以同一后端 TCP 连接接收 8 MiB，耗时在 112.415 至 112.417 秒之间。完整 HTTP 200 在 112.527 秒收到；另查持久偏移、唯一原回执和独立逐字节读回均一致。 |

## 待验门槛

| 范围 | 放行条件 |
| --- | --- |
| [首次选盘](docs/specs/data-placement.md) | [仅静态] 用两块独立 DATA 在实际隔离客机反转可用空间；核新容器选盘、已登记归属不变、原盘不可用不改盘。 |
| [回收 RC2](docs/specs/recycle-record.md) | [仅静态] 在隔离真卷核原盘冷重挂、设备号变化、回收正文损坏时保留 DATA、源清理过半后的同号恢复、配套 Helper 空盘退役。 |
| [离线维护](../helper/docs/guides/headless-offline.md) | [仅静态] 已登记空 DATA 的初始化只建目标三个命名空间；按原命令验证中断、续作以及不改其他原件。真实挂载别名不得绕过角色隔离。 |
| [外部文件导入](../helper/docs/guides/offline-imports.md) | [仅静态] [固定批次](docs/specs/offline-import-batch.md)仍需跨进程验父进程失答、原回执无源重放和半批恢复。保留来源；不就地覆盖非空盘，不迁移旧池。 |
| 运行时断电、掉盘与权限变化 | [仅静态] 验缺盘冷启动、原盘恢复、其他容器独立可用、离线盘相关全局结果拒绝、掉电后的确定回执以及撤权不误报未找到。 |
| [会话与后端复用](docs/specs/sessions.md) | [仅静态] 待已验证的 HTTPS/H2 入口确认新旧模式、到期、重启与只读原回执找回。 |
| [分块上传与续传](docs/specs/resumable-uploads.md) | [仅静态] [剩余流式链路](docs/guides/resumable-upload-installed-qualification.md#流式链路待验)仍待验超过固定入站时限的拒绝和正文截断。服务端重启、会话到期和桌面断点续传也未验。 |

## 可复核的验收入口

[一次性 Linux 客机工作流](../.github/workflows/linux-guest-product-smoke.yml)用同一提交构建 Helper 与 ZXS，在无网卡客机为普通账号配专用临时卷；不使用现有磁盘，也不产生安装包。

- **默认模式：** 首设、独立的 DATA 和 RECYCLE、精确正文与回执、送完不读回包再重启查原号，以及缺盘和原盘重挂。
- **`workspace_h2`：** 只在同客机回环验证真实 TLS/HTTP/2 和完整 R3 传输，不启动 Qt。
- **`workspace_fuse`：** 另用固定 Ubuntu 客机和经过核对的 FUSE 组件；验证编辑卷、正常卸载及全量提交。Windows、macOS 桌面另验，见 [Workspace 任务](../zxs-workspace/TASKS.md)。

挂载、断电、实机运行和签名发行各自留证，不用源例结果替代。
