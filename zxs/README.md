# 文件容器存储服务

一个容器保存一个文件及其完整历史版本。常用操作是首次保存、追加完整版本、从历史 `main` 节点分叉、改名和标签。删除时移入专用回收盘。ZXS 不提供网页管理界面、TLS 终止或 SMB。

## 5 分钟上手

备好 Linux x86-64、原生二进制、专用普通账号和空存储根后，按[首次部署](docs/guides/quickstart.md)操作：

1. 用 Helper 检查并登记 SYSTEM、DATA 和专用 RECYCLE。已有旧数据不要清空或就地转换。
2. 用同一系统账号运行 `/zxs/bin/zxs --listen 127.0.0.1:8080`。这个地址只是本机示例，服务不自动开放网络。
3. 为远程访问配置你自己的 HTTPS/HTTP/2 入口，再从 Workspace 的「存储容器」页登录读取列表。明文后端仅放在可信网络内。

## 当前范围

新回收记录使用持久磁盘身份，见[磁盘格式](docs/specs/recycle-record.md)。[仅静态] 新格式的重挂验收待跑。

| 功能 | 验证状态 |
| --- | --- |
| 首次设置、整文件写入、原操作回执和跨卷回收 | [CI通过] [隔离 Linux 客机的已通过阶段](https://github.com/isarmg/zxs/actions/runs/45322511258/job/138204176887)，也覆盖真实 HTTPS 上的 HTTP/2 写入读回。 |
| FUSE、缺盘冷启动及恢复挂载 | 同一作业在 FUSE 阶段失败，缺盘和重挂步骤尚未执行。实机和发行待验。 |
| 内存登录会话、HTTP Range、按字节分块续传 | [设计中] 当前 R3 是整文件传输。中断后按原操作号核对，不表示已支持按偏移续传。 |

## 进一步阅读

- [首次部署](docs/guides/quickstart.md) · [启动和反代要求](docs/guides/headless-activation.md) · [构建独立二进制](BUILD.md)
- [Workspace 容器操作](../zxs-workspace/docs/guides/remote-files.md) · [Helper 离线维护](../helper/README.md)
- [架构](ARCHITECTURE.md) · [交付与验收](TASKS.md) · [原启动参考（固定版本）](https://github.com/isarmg/zxs/blob/011c58f2e0f5df80e939fcae058e94d045b5ff68/zxs/docs/headless-activation.md)
