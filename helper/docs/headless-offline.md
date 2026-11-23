# 离线首设与密码重置

| 操作 | 命令 | 写入范围 |
| --- | --- | --- |
| 首设前检查 | `setup-headless check SYSTEM RECYCLE DATA [DATA ...]` | 无 |
| 服务单元预览（可选） | `setup-headless preview-unit IP:PORT SYSTEM RECYCLE DATA [DATA ...]` | 无，写标准输出 |
| 首设 | `setup-headless new SYSTEM RECYCLE DATA [DATA ...] LOGIN` | 所选磁盘的角色标记及 `/zxs/state/headless/server.conf` |
| 源目录预检 | `preflight-import SOURCE SYSTEM RECYCLE DATA [DATA ...]` | 无，仅统计目录元数据 |
| 网页首设 | `setup-headless web new [--preview-unit IP:PORT] SYSTEM RECYCLE DATA [DATA ...] LOGIN` | 同首设，仍在终端确认 |
| 重置密码 | `setup-headless reset SYSTEM RECYCLE DATA [DATA ...]` | 同一份 `server.conf` 中的账号代次、随机盐和验证值 |

使用同一非 root 服务账号。目录参数使用实际绝对路径。
目录应事先挂载。每个数据盘须位于不同的文件系统；回收站盘与系统盘及所有数据盘分开。系统盘可以和一块数据盘共用文件系统，但不能共用目录。
首设及重置都不格式化、挂载或修改业务文件。Helper 不需常驻。

重置前先停止业务服务。Helper 取得与服务共同使用的离线排他锁后，
核对现有账户、所有已登记的磁盘目录和原标记，再在同一前台终端显示所选对象。
键入 `RESET` 后两次输入不回显的密码；不会接收命令行或网页口令。
用户名、池和角色保持不变。退出码 0 表示确认；
退出码 3 表示结果不确定，保留原目录和可能存在的暂存记录，先查明再继续。
不向旧 SQLite 格式写入或迁移账户。

本机磁盘、硬件和镜像观察不需要业务停机，见[磁盘面板](disk-web.md)。
当前首设/重置不包括磁盘加入、移除和非空盘转换。首设后用同一非 root 服务账号直接运行
`/zxs/bin/zxs --listen IP:PORT`，见[部署说明](../../zxs/docs/headless-activation.md)。

## 非空源目录预检

`SOURCE` 是待清点的外部目录，后面的参数是当前已配置的系统盘、回收站盘和全部数据盘（按登记顺序），不是导入目标。
先停业务服务，使用同一服务账号运行预检。源目录可在同一文件系统的
不重叠位置，但不能与任何已配置角色、状态路径或安装路径重叠，也不追随链接或跨挂载。
逐项观察元数据，显示文件数、目录数、空目录数和文件逻辑总字节。多硬链接按名字计数，
并单列提示。遇到链接、特殊项、意外跨挂载或观察到原件变化时不显示部分结果。
不设总项数或自定超时，路径长度和可用描述符仍受系统限制。统计不确认目标命名兼容性，
不读取正文，不选择目标，不预留空间，也不执行转换。


## 可选服务单元预览

与首次检查使用同一次有序的 SYSTEM、RECYCLE、DATA 原目录准入；仅在初始目录检查通过时生成。请在首次设置之前运行预览。输出的 `RequiresMountsFor` 是按登记顺序列出实际目录路径，不认定目录本身就是独立挂载点；启动时服务仍核对原卷。预览保留可表示路径的精确内容；含控制字符或非 UTF-8 字节的路径不生成单元。

预览供管理员审阅。显式填数字 IP 和非零端口；IPv6 用方括号。单元以当前非 root 服务账号直接执行 `/zxs/bin/zxs --listen IP:PORT`；`Type=simple` 只跟踪进程，不证明 HTTP 已就绪。停机先通知服务并等它收尾。Helper 不写 unit、不调用 systemd，也不启动或启用任何服务。首次设置也可用 `setup-headless web new --preview-unit IP:PORT …` 匹配原终端并展开相同纯文本；没有显式开关，网页不显示路径。

## 离线加入空数据盘

只适用于当前多数据盘清单格式，包括只登记了一块数据盘的清单。旧版独立服务的单数据盘配置不能直接启动或重置，本命令也不转换旧配置。请保留旧配置和数据。

停止 ZXS，以原服务账号运行：

```sh
helper setup-headless add-data NEW_DATA SYSTEM RECYCLE DATA [DATA ...]
```

`NEW_DATA` 为已挂载的空数据盘目录。后面的目录必须按登记顺序给出。确认完成后再启动 ZXS。命令不迁移文件、不加入非空盘，也不移除磁盘。若已写入新盘标记但未更新清单，用相同参数重试；如果已有临时文件，命令只续写能核对为本次操作的内容，不覆盖无法核对的文件。
