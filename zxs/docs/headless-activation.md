# 部署 HTTPS 入口

服务只监听私有 Unix 套接字 `/zxs/run/headless.sock`。Workspace 连接独立的 nginx HTTPS 入口，优先 HTTP/2。外部 nginx 负责 TLS 和接收完整 HTTP 请求，服务用专用 Unix 对端 UID 和原始账户凭据认证。代理不承担容器授权，不把转发的用户或来源当作身份。

## 准备主机

- 先完成 Helper 的 SYSTEM、数据盘、回收盘和管理员设置。安装审核过的 `/zxs/bin/zxs` 与独立 `/usr/sbin/nginx`。此入口使用 x86-64 Linux、原生 PIDFS 和正常的受保护 procfs。
- `/zxs`、`/zxs/bin`、`/zxs/state` 由 root 所有，组和其他用户不可写；`/zxs/state/headless` 属于非 root 服务 UID，权限 `0700`，里面的 `server.v1` 由 Helper 创建。服务 UID 与 nginx worker UID 不同。
- `/zxs/run` 属于服务 UID、IPC 组，权限 `0710`。让 nginx worker 使用专用账号和该 IPC 组。服务创建权限 `0660` 的套接字。配置里的 worker 账号解析出的 UID 必须与 Helper 描述文件和入选记录一致，不与其他进程共用该 UID。
- 根据 [独立 nginx 模板](nginx-headless.conf.example)填写实际域名、证书和密钥路径、日志和上传暂存目录、worker 账号和 IPC 组。暂存盘需要能容纳并发的完整上传，不使用容器数据盘或回收盘。证书密钥只供 nginx 使用，日志不记录账户凭据或请求体。
- 有效配置固定为 `/etc/zxs-gateway/nginx.conf`。`/etc/zxs-gateway` 和配置文件由 root 所有、组和其他用户不可写；配置文件为普通文件、单硬链接，服务 UID 必须能遍历父目录并只读打开此文件（例如目录 `0755`、文件 `0644`）。不要在配置里放私钥内容。配置必须独立，禁止 include、热更新、重载和在线二进制替换。变更时停止两个进程，重新审核和入选。

## 审核和入选

使用已审核的 nginx 构建。模板依据 [nginx 请求缓冲文档](https://nginx.org/en/docs/http/ngx_http_proxy_module.html#proxy_request_buffering)和 [nginx 1.30.7 HTTP/2 请求体实现](https://github.com/nginx/nginx/blob/release-1.30.7/src/http/v2/ngx_http_v2.c#L3859-L3984)：完整收完原始请求且声明长度正确，才建立上游请求。同样确认所选构建和最终配置对取消、提前断流和正文长度不符的请求不发给上游。禁止改变请求路径、自动重试、升级、转发 trailers 或重设原始 Authorization；不把客户端声明的 Content-Length 直接覆盖到上游。GET/DELETE 必须无正文；写请求提供单一、规范的 Content-Length。前端只转发协议路由前缀，具体操作和权限仍由服务判定。校对证书、HTTP/2、账户映射、暂存空间和进程隔离后再入选。

向固定的 `/etc/zxs-gateway/activation.v1` 写入以下七行。文件 root 所有、权限 `0400`、普通文件且单硬链接。每行结尾 LF；摘要用 64 位小写十六进制，ID 用无前导零的十进制。尖括号表示须替换的值，不是文件内容。不要从未经审核的当前文件自动生成批准记录。

```text
ZXS-HOST-ACTIVATION-1
<nginx 可执行文件的 SHA-256>
<已审核的完整有效配置的 SHA-256>
<nginx worker UID>
<zxs 可执行文件的 SHA-256>
<zxs 服务 UID>
<IPC 组 GID>
```

使用 `host-activation` feature 构建临时 `activate-headless`，由 root 无参数调用。它启动已入选 nginx，按原始 PIDFD 和入选状态调用低权限 `zxs`，随后退出；不安装开机服务。用实际 Workspace 客户端确认可连接后再提供使用。若残留 `/run/zxs-gateway.pid` 或 `/zxs/run/headless.sock`，程序不会清理或接管；先由主机管理员核实原进程已退出，再处理残留。单次启动返回成功表示已执行进程交接，独立确认套接字和容器访问后才算启动检查完成。
