# ZXS 业务管理：原生 HTTP

原池和唯一 Web 管理员确认后，使用有该池权限的普通存储身份运行验签版 ZXS。**业务管理直接监听 IPv4 `0.0.0.0:8443`**，不需要指定域名、准备证书、安装 nginx 或创建外部套接字。打开 `http://<服务器实际 IPv4>:8443/`，使用 Web 管理员登录，不接受 SMB 账户或访客。IPv4 通配监听不是浏览器地址，也不表示新增 IPv6 监听。

**这里的直连 HTTP 不加密。** 管理口令、会话和管理内容只有在你信任路径时才能走明文。要对不可信网络提供浏览器访问，由你选择的软件终结 HTTPS，并独立保护代理到 ZXS 的 HTTP 一段以及避免绕过代理直接访问 8443；本包不配置代理、证书、DNS、防火墙或转发规则。通配意味着每个可达 IPv4 接口都可进入登录页，同源/CSRF 和登录是另外的检查，不会把 8443 变成私网监听。别为访问使用不可信的任意代理头或伪造 HTTPS。 浏览器不会按端口隔离同一访问主机的 Cookie；其他能收到该管理 Cookie 的服务也必须可信。

## 最短启动路径

这些不是自动执行的安装动作。先按[首次安装](samba-domain.md#本机首设入口)留好原池与原管理员回执，安装与这份说明相配的已独立验签 **原生 HTTP 版** ZXS。旧版 Unix 套接字的二进制不能替代它。确认 8443 未被其他应用占用；冲突时先停止这次启动，不由本模板覆盖或停用别的服务。

在**该池实际登记的非 root 存储身份**的终端选择原 SYSTEM 和原 PoolId（只有池资料，不是额外 Web 配置）：

```sh
ZXS_SYSTEM_ROOT="${SYSTEM:?先设为原 SYSTEM 的绝对路径}" \
ZXS_POOL_ID="${POOL_ID:?先设原 PoolId}" /usr/bin/zxs
```

真正的服务由 ZXS 核原池、持有在线租约和处理登录/会话。启动命令、安装包存在或进程没有报错都不是登录验收。若只在本机访问，可用服务器的 `127.0.0.1`；这不改变实际的全 IPv4 监听。Helper 的[磁盘配对面板](disk-web.md)仍只从本机终端开随机回环端口，不共享管理监听或管理员会话。

## 可选：准备一份 systemd 单元

如果不想每次从终端启动，可在普通用户自有、他人不可写的工作目录，用验签的 helper 发行材料生成这一可选候选：

```sh
/usr/bin/python3 -I -S -B /usr/share/local-ops-helper/web-management/profile.py render \
  "${SYSTEM:?先设为原 SYSTEM 的绝对路径}" "${POOL_ID:?先设原 PoolId}" \
  "${STORAGE_UID:?先设池的存储 UID}" "${STORAGE_GID:?先设池的存储 GID}"
```

只写新的 `./zxs-management.candidate/zxs-management-http.service`，不读数据库/密钥、不验现场账号或端口、不装或启任何服务。失败留下的候选不是成功。生成器只核有界的原公开模板与四个参数；将路径作为带转义的 systemd 值，不拼 shell。 可选单元受 systemd 的含尾 NUL 路径长度限制；生成器不限制原生前台入口。管理员应独立复核原池与身份、单位内容和实际将生效的配置，没有重定向、旧配置、未知同名或 drop-in 后才将这个新名字以 root:root 0644 安装到受保护的 `/etc/systemd/system/`。已装的旧站点由其管理员另行决定停用，不会自动覆盖；旧站点若占相同端口，先解决冲突。只选择前台运行或这个单元之一。

原生 HTTP 不要求 Web 环境参数。只有明确使用外部 HTTPS 代理时，才按 ZXS 的部署约定成对设置 `ZXS_WEB_PUBLIC_ORIGIN`（浏览器使用的准确 HTTPS 站点）和 `ZXS_WEB_TRUSTED_PROXY`（与原生服务直接连接的代理 IP）。代理仍须保留实际 Host 与 Origin；不单凭任意 `Forwarded` 或 `X-Forwarded-*` 头承认 HTTPS。这个可选模板不选代理或代你写这两个值。

这个单元仅启动无参数的 `/usr/bin/zxs`，不含 `[Install]`、子进程代理、证书命令或数据库操作。其服务身份与目录限制不替代 ZXS 对原 SYSTEM、账户和权限的实际核验。管理员另行同意启动后，才可执行：

```sh
systemctl daemon-reload
systemctl start zxs-management-http.service
```

维护时先停止管理进程，确认真实存储租约已释放。前台终端送正常退出信号或由管理员停止这一独立单元；超时、信号或浏览器断开不当作独占权证明。SMB 及其他在线持有者的状态与这个 Web 进程分开判断。

## 待现场验收

确认原生 HTTP 能按准确地址从预先同意的 IPv4 网络到达、没有由本单位新增的 IPv6 listener。正常登录、退出、失效会话、错 Host/跨源、伪代理头、错误 CSRF 和超限请求须各自实测，且不能触达业务/磁盘正文。独立 HTTPS 终结、其站点认证、绕过与信任配置另验。本次只交付源码；没有构建、安装、启动、登录或修改现场网络。

参考：[systemd 单元执行环境](https://www.freedesktop.org/software/systemd/man/latest/systemd.exec.html)。
