# 离线首设与密码重置

| 操作 | 命令 | 写入范围 |
| --- | --- | --- |
| 首设前检查 | `setup-headless check SYSTEM DATA RECYCLE GATEWAY_UID IPC_GID` | 无 |
| 首设 | `setup-headless [web] new SYSTEM DATA RECYCLE GATEWAY_UID IPC_GID LOGIN` | 三个角色标记及 `/zxs/state/headless/server.v1` |
| 源目录预检 | `preflight-import SOURCE SYSTEM DATA RECYCLE GATEWAY_UID IPC_GID` | 无，仅统计目录元数据 |
| 重置密码 | `setup-headless reset SYSTEM DATA RECYCLE GATEWAY_UID IPC_GID` | 同一份 `server.v1` 中的账号代次、随机盐和验证值 |

使用同一非 root 服务账号和专用 IPC 组。参数使用实际的绝对路径及数值 ID。
三个根应已挂载；回收卷须与 SYSTEM 和数据根位于不同文件系统。
首设及重置都不格式化、挂载或修改业务文件。Helper 不需常驻。

重置前先停止业务服务。Helper 取得与服务共同使用的离线排他锁后，
核对现有账户、三个实际角色目录和原标记，再在同一前台终端显示所选对象。
键入 `RESET` 后两次输入不回显的密码；不会接收命令行或网页口令。
用户名、网关 UID、池和角色保持不变。退出码 0 表示确认；
退出码 3 表示结果不确定，保留原目录和可能存在的暂存记录，先查明再继续。
不向旧 SQLite 格式写入或迁移账户。

本机磁盘、硬件和镜像观察不需要业务停机，见[磁盘面板](disk-web.md)。
当前首设/重置不包括磁盘加入、移除和非空盘转换；网关审核与启动
见[部署说明](../../zxs/docs/headless-activation.md)。

## 非空源目录预检

`SOURCE` 是待清点的外部目录，后面三个参数是当前已配置的角色，不是导入目标。
先停业务服务，使用同一服务账号与专用 IPC 组运行预检。源目录可在同一文件系统的
不重叠位置，但不能与任何已配置角色、状态路径或安装路径重叠，也不追随链接或跨挂载。
逐项观察元数据，显示文件数、目录数、空目录数和文件逻辑总字节。多硬链接按名字计数，
并单列提示。遇到链接、特殊项、意外跨挂载或观察到原件变化时不显示部分结果。
不设总项数或自定超时，路径长度和可用描述符仍受系统限制。统计不确认目标命名兼容性，
不读取正文，不选择目标，不预留空间，也不执行转换。
