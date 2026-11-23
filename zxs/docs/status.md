# 当前范围与验证

ZXS 的入口是管理 Web 和内置 Rust SMB，不是磁盘管理工具。下表说的是当前新池源码；部署前看本页下方的实际验证记录。

| 范围 | 当前源码 |
| --- | --- |
| 安装与启动 | Linux x86-64 GNU，构建一个静态 ELF，内置浏览器资源。由宿主挂盘，独立 Helper 完成新池首设；Helper 默认构建不含私有新池首设入口。已首设站点用已批准的 ELF 无参数前台启动，按固定部署记录找站点，不升级旧池。[构建](../BUILD.md) · [首设](../../helper/docs/fresh-deployment.md) · [启动](../src/smb/launch/site/CONTRACT.md) |
| Web | 仅提供明文 HTTP，监听 `0.0.0.0:8443`。共用账户单独登录，管理首设选定的数据目录模式；Rust/WASM 界面浏览已授权的目录和文件名、管理标签、查看版本、显式提交工作稿。没有文件正文接口或磁盘监控。[管理 Web](web-management.md) |
| SMB | 一条内置 SMB 3.1.1 签名路径和一个 `\\服务器\ZXS`，无匿名或来宾。已接入普通文件的受限创建、打开、读写、改长、FLUSH 和正常 CLOSE；有普通子目录新建、列举和同父改名、普通 FILE 同一数据卷内改名或换父、单文件回收与同树还原。普通 FILE 可在同一树中两个独立的可写数据文件系统间无覆盖移动；Linux 客户端验收未运行。普通 FILE 已有窄范围的独立非阻塞字节锁，未做真实客户端验收；覆盖式替换、阻塞或多元素字节锁、目录跨父或整组回收还原不在现有入口内。[详细业务范围](../src/smb/BUSINESS.md) · [跨卷验收](../tests/acceptance/rust-smb-client.md#普通数据卷到普通数据卷) |
| 存储与运维 | SQLite SYSTEM 决定归属、已确认稿、正式版本和操作结果，正文留在普通文件容器。每个 FILE 一名写者，保存不发版本节点，手动提交才发。回收使用独立文件系统。宿主挂载、磁盘诊断和需要停机的维护由宿主与 Helper 分别处理。[存储契约](sqlite-system.md) · [Helper](../../helper/README.md) |

## 已有证据

模式 20 的新配对待验收。

| 修订 | 已经验证 |
| --- | --- |
| `f0728833` | [Linux Rust 检查](https://github.com/isarmg/zxs/actions/runs/45039609819/job/137260892181)：原生单元测试 444 项通过、247 项忽略；模拟浏览器检查及 `wasm32` pedantic Clippy 通过，Linux 原生严格 Clippy 未通过。 |
| `f0728833` | [Linux 配对构建](https://github.com/isarmg/zxs/actions/runs/45039609854/job/137260891797)：单 ELF 及其内置浏览器资源通过构建核对，带私有 v18 首设入口的 Helper 也构建通过。制品未在该流程启动。 |
| `3ad5d878`、`93053adc` | [Helper 专用夹具](https://github.com/isarmg/zxs/actions/runs/45022155081/job/137207236617)通过 25 项；另一次[终端预检](https://github.com/isarmg/zxs/actions/runs/45027190972/job/137222665340)确认了真实前台 PTY 和当前测试进程的 TTY 审计未启用。该客体继承的 MEMLOCK 软、硬上限均为 8 MiB，低于首设要求的软上限大于 64 MiB；没有改动上限。 |

[隔离 Linux 正式验收](web-and-smb.md#本轮正式验收)尚未运行：需要同一对核准的制品完成新池首设、双角色启动、真实浏览器与签名 SMB 客户端往返、离线重置和冷启动。实际掉盘、断电和跨平台应用保存单独记录。[验收用例](../TASKS.md) · [历史实验](legacy-experiments.md)
