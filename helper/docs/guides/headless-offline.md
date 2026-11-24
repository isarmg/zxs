# 首设与离线池维护

供部署和离线维护人员使用。先挂载所选目录，以同一专用非 root 服务账号操作。
检查和预览只读；下列维护动作不格式化磁盘、不启动服务。

| 要做的事 | 命令 | 写入范围 |
| --- | --- | --- |
| 首设前检查 | `helper setup-headless check SYSTEM RECYCLE DATA [DATA ...]` | 无 |
| 首设 | `helper setup-headless new SYSTEM RECYCLE DATA [DATA ...] LOGIN` | 所选角色标记和 `/zxs/state/headless/server.conf` |
| 服务单元预览 | `helper setup-headless preview-unit IP:PORT SYSTEM RECYCLE DATA [DATA ...]` | 无，写标准输出 |
| 重置密码 | `helper setup-headless reset SYSTEM RECYCLE DATA [DATA ...]` | 原描述文件的账号代次、盐和验证值 |
| 非空源清点 | [源目录预检](#非空源目录预检) | 无 |
| 文件导入 | [导入指南](offline-imports.md) | 应用原计划时写入已登记 DATA，在 SYSTEM 留存容器记录，保留来源 |

## 选择目录和首设

- SYSTEM 和 RECYCLE 各一个，DATA 为 1–62 个。填实际绝对路径，后续维护按原登记顺序输入。
- DATA 彼此使用不同的文件系统。RECYCLE 与 SYSTEM、各 DATA 分开；SYSTEM 可和一块 DATA 共用文件系统，不能共用目录。
- 角色目录及固定状态目录互不包含；不同拼写不能绕过这个限制。共用父目录的兄弟目录不因此被拒绝。

先运行首设检查，再运行 `new`。核对前台终端中的角色和登录名，键入 `SETUP`，然后输入两次不回显的密码。
首设成功后，以同一服务账号运行 `/zxs/bin/zxs --listen IP:PORT`，见 [ZXS 部署说明](../../../zxs/docs/headless-activation.md)。

Helper 不挂载或格式化磁盘。路径、锁和中断时的处理见[原件与离线权限](../invariants/offline-authority.md)。

## 离线重置密码

先停 ZXS。用同一服务账号、同一组已登记目录运行 `reset`。
核对前台终端中的账号和角色，键入 `RESET`，输入两次不回显的新密码。密码不作为命令行参数。

用户名、池和角色不变。退出码 `0` 表示确认；`3` 表示不确定，应保留原目录及可能的暂存记录，先核对再继续。
此命令不重置或迁移旧 SQLite 账号。

## 非空源目录预检

用 [`preflight-import` 指南](import-preflight.md)清点来源和可选的目标重名。
预检不导入文件；实际文件选择和原计划重放见[导入指南](offline-imports.md)。

## 可选服务单元预览

首设之前，在同一账号和目录选择下运行 `preview-unit`。填数字 IP 和非零端口；IPv6 用方括号。
初始目录检查通过后才输出；含控制字符或非 UTF-8 字节的路径不生成单元。

`RequiresMountsFor` 按选择顺序列实际目录，不证明目录本身是挂载点。
`Type=simple` 只跟踪进程，不证明 HTTP 已就绪。停机时先通知服务并等待收尾。

单元以当前非 root 服务账号执行 `/zxs/bin/zxs --listen IP:PORT`。
输出供管理员审阅。Helper 不写 unit、不调用 systemd、不启动或启用服务。

## 离线加入空数据盘

停止 ZXS，以原服务账号运行：

```sh
helper setup-headless add-data NEW_DATA SYSTEM RECYCLE DATA [DATA ...]
```

`NEW_DATA` 是已挂载的空目录；后面仍列原清单。确认完成再启动 ZXS。
该动作不导入、不迁移文件，不转换旧版单数据盘配置。当前格式只登记一块 DATA 的池可以加盘。

如果角色标记已经写下而清单未更新，用相同参数重试。只有认得出本次准确原件的暂存才可续作；不要覆盖陌生文件。

## 离线初始化一块已登记的空 DATA

已有池的 SYSTEM 和 RECYCLE 须已初始化；目标 DATA 须已登记。先停机，以原服务账号运行：

```sh
helper setup-headless init-data /mnt/archive/zxs-data /system /recycle /data-one /mnt/archive/zxs-data
```

参数包含当前清单的完整顺序。只在所选 DATA 准备 `owner`、`data`、`staging`；不加盘、不改清单，不初始化或清理其他卷。
多块新盘可逐块执行。已经具备有效空布局时不重建；首次全池初始化仍不是此命令的工作。

拒绝条件包括：

- 不是原角色标记或可验证的有效空前缀，或存在额外条目、正文、暂存。
- 已有指向该盘的容器登记、待确认的 SYSTEM 路由、相关退盘暂存或已发布的退盘记录。
- 所装 ZXS 不提供配套初始化入口。不会改为启动服务。

不确定时保留原件，用同一角色和目标重试同一命令。

## 离线移除空数据盘

停止 ZXS，以原服务账号运行：

```sh
helper setup-headless retire-data TARGET SYSTEM RECYCLE DATA [DATA ...]
```

当前清单须包含 `TARGET`，并至少保留一块 DATA。目标仍须可访问。
仅有角色标记的盘不得有容器路由。已初始化的盘则要求 `data` 和 `staging` 都为空；指向该盘的旧路由须有[选定回执及对应回收原件](../specs/recycle-retirement-receipts.md)。

命令只更新 SYSTEM 和清单，不擦盘、不改目标角色标记。中断用相同参数重试；旧角色标记不可用于再次加入。

`container … has no selected recycle record` 表示无法证明已回收，不能退盘。即使没有可见文件，首次保存中断后仍可能留有路由。
若由 `import-one` 中断引起，且原票和匹配源文件都在，仅对原目标重试该票。补齐保存并将容器移入回收站后才可退盘。
不要手工删除登记记录。
