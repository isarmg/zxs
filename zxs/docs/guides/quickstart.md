# ZXS 首次部署

给第一次设置 ZXS 的管理员。这页只讲首设与启动，不包含构建、挂载或配置反代。先备好 Linux x86-64、专用普通系统账号和空存储根，再完成首设、启动和第一次只读连接。

## 1. 登记一套新存储

准备[经过审核的 ZXS 二进制](../../BUILD.md)（标准部署位置为 `/zxs/bin/zxs`）和 [Helper](../../../helper/RELEASE.md)。示例假定 `helper` 已在 PATH 中。管理员先准备磁盘及目录，随后用同一个非 root 服务账号执行首设。

- SYSTEM 记录容器位置和回收状态；DATA 保存容器；RECYCLE 保存回收的容器。
- `/zxs`、`/zxs/bin`、`/zxs/state` 由 root 管理，不允许其他账号写入；空的 `/zxs/state/headless` 和各角色根由服务账号所有，权限设为 `0700`。
- 使用实际绝对路径，三个角色目录不重叠。回收盘和其他角色用不同文件系统；有多块 DATA 时也要分别位于不同文件系统。
- 本流程只给空目录做首次设置。不会格式化磁盘，也不迁移旧池。更多先决条件见 [Helper 首设说明](../../../helper/docs/guides/headless-offline.md)。

下列路径是示例，只替换为你已准备好的目录。先检查，不写入磁盘：

```sh
SYSTEM=/mnt/zxs/system
RECYCLE=/mnt/zxs/recycle
DATA=/mnt/zxs/data
helper setup-headless check "$SYSTEM" "$RECYCLE" "$DATA"
```

确认检查通过，再首设。把示例登录名 `operator` 改为你选的 ZXS 账号；它不是系统账号参数。在本地终端核对角色路径，输入 `SETUP`，再两次输入不回显的口令。

```sh
helper setup-headless new "$SYSTEM" "$RECYCLE" "$DATA" operator
```

若使用多块 DATA，把各路径依序写在 RECYCLE 之后，`new` 的登录名仍放最后。首设成功后会写角色标记和 `/zxs/state/headless/server.conf`，不会启动服务。

## 2. 显式启动

仍用原系统账号，在同机反代的示例中运行：

```sh
/zxs/bin/zxs --listen 127.0.0.1:8080
```

端口需要明确指定，地址必须是数字 IP，IPv6 用方括号。ZXS 在前台提供明文 HTTP。请把明文入口留在本机或可信网络内，不对不可信网络暴露口令。结束时向进程发 `SIGTERM`，等它退出。

## 3. 只读连接

如需连接 Workspace，由你配置 HTTPS 入口并启用 HTTP/2。反代须保留真实请求路径和凭据，正文按原长度转发；ZXS 仍核对自己的登录，不接受代理身份头。[反代和运行细节](../headless-activation.md)。

在 Workspace 的「存储容器」页点「连接…」，填 HTTPS 地址及刚设置的账号、口令，再点「连接并读取列表」。新池的列表为空是正常的。读取列表不上传文件；查看、保存或回查原提交按[容器页指南](../../../zxs-workspace/docs/container-management.md)操作。

## 验证范围

[CI通过] [隔离 Linux 客机的已通过阶段](https://github.com/isarmg/zxs/actions/runs/45322511258/job/138204176887)覆盖首设、整文件写入、原操作回执、跨卷回收和真实 HTTPS 上的 HTTP/2 写入读回。该作业之后在 FUSE 阶段失败，缺盘重挂未执行；实机与发行待验。
