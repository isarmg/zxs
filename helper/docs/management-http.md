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

只写新的 `./zxs-management.candidate/zxs-management-http.service`，不读数据库/密钥、不验现场账号或端口、不装或启任何服务。失败留下的候选不是成功。生成器只核有界的原公开模板与四个参数；将路径作为带转义的 systemd 值，不拼 shell。 可选单元受 systemd 的含尾 NUL 路径长度限制；生成器不限制原生前台入口。这是可预览的样本，不是上传入口。正式安装可在受信本机 root 前台选择同一个真池、首设原单和独立已验的 ZXS ELF 摘要：

```sh
/usr/bin/helper install-web-unit \
  --system "${SYSTEM:?先设为原 SYSTEM 的绝对路径}" \
  --pool "${POOL_ID:?先设原 PoolId}" --init "${INIT_OP:?先设原 INIT}" \
  --setup-op "${SETUP_OP:?先设原首设}" --approval "${UNIT_APPROVAL:?先记本次审批参考}" \
  --zxs-sha256 "${ZXS_SHA256:?先取得受信发行的独立 ELF SHA-256}"
```

低权限工人只读核已提交 INIT 和该次原首设的活动 Web 管理员；宿主持同一原 SYSTEM 的非阻塞独占锁，只安装给同一非 root 存储身份。它不读取口令或密验、不拿浏览器或旧候选授权。宿主核真实 PID 1 管理器当前未解析同名服务，并核固定原目录；在前台显示站点、身份范围、实际 ELF 和新主片段的摘要，再要求一次本地确认。正式名字 `/etc/systemd/system/zxs-management-http.service` 仅以同父无覆盖方式发表、同步、按原名完整读回；未知同名不接管。在没有主片段时，systemd 可能没有读取已经存在的覆盖片段（drop-in）；本次不证明以后实际加载的配置，也不保证没有其他独立系统配置会触发它。已有旧站点或其他服务占端口时由管理员另行处置。本操作不执行 `daemon-reload`，不启动、停止或启用任何服务。中断可能已经留有暂存名或最后名字；保留原件，不自动删或重试。这不是管理上线或独占业务证明；如只需要基本 HTTP，可继续用原前台方式，不必安装单元。

原生 HTTP 不要求 Web 环境参数。只有明确使用外部 HTTPS 代理时，才按 ZXS 的部署约定成对设置 `ZXS_WEB_PUBLIC_ORIGIN`（浏览器使用的准确 HTTPS 站点）和 `ZXS_WEB_TRUSTED_PROXY`（与原生服务直接连接的代理 IP）。代理仍须保留实际 Host 与 Origin；不单凭任意 `Forwarded` 或 `X-Forwarded-*` 头承认 HTTPS。上述可选单元只提供原生 HTTP；它在该进程里显式清除这两个值，防止从 PID 1 默认环境误继承信任。不读取、删除或改写全机环境。外部终结器若需开启 ZXS 的可选代理模式，请另用你独立管理的服务配置或原生前台方式。

本包准备的主片段仅含无参数的 `/usr/bin/zxs`，不含 `[Install]`、子进程代理、证书命令或数据库操作。安装主片段不等于有效配置已载入。

### 核验、启动与停止固定单元

仅适用于上面安装的固定原生 HTTP 单元。本机管理员单独核对并决定是否让 PID 1 重新加载系统配置；`daemon-reload` 会影响全机的单元配置，Helper 不代做。与其他系统维护人协调，以下核验至操作完成期间不要并发修改或操作该单元、发行文件、池和挂载。单独维护的外部代理或自写单元不由这些命令接管。

```sh
/usr/bin/helper web-unit status --system "${SYSTEM:?}" --pool "${POOL_ID:?}" --zxs-sha256 "${ZXS_SHA256:?}"
/usr/bin/helper web-unit start --system "${SYSTEM:?}" --pool "${POOL_ID:?}" \
  --init "${INIT_OP:?}" --setup-op "${SETUP_OP:?}" --approval "${START_APPROVAL:?}" --zxs-sha256 "${ZXS_SHA256:?}"
/usr/bin/helper web-unit stop --system "${SYSTEM:?}" --pool "${POOL_ID:?}" \
  --approval "${STOP_APPROVAL:?}" --zxs-sha256 "${ZXS_SHA256:?}"
```

上面不是要整段执行：在同一本机可信 root 前台分别选择动作。`status` 只查。启停先重新验证同一原片段、已核发行 ELF、实际 PID 1 已加载配置、池与非 root 身份；拒未知覆盖、外部单元联动、未载入或过渡状态。`start` 还由永久降权的工人查本池原 INIT 与原 Web 首设，并显式提示 8443 的全 IPv4 HTTP 监听。`stop` 不依赖能否读取正忙的数据库；它必须认出当前 PID 1 实际持有的原主进程，并核该进程确持有本次独立匹配池号的同一原 SYSTEM 标记，不只看路径、配置文字或进程能看到该目录。启动只提交固定单元名的一项 PID 1 作业，不替换其他正在进行的管理作业。只有正常 sysinit 已就绪、默认系统和根 slice 已经活动、没有关机作业时才请求；不代为启动抢修或引导服务。停止不是按名称提交 StopUnit：它仅向前台所核准的原主进程的持有 pidfd 发送一次 TERM，不强杀，也不追停同名重启后的另一个实例。启停各要求一次现场确认，不自动重试、重启或开机启用。丢失回执时先查实际状态；结束本程序不等于取消 PID 1 已接受的启动作业或撤销已发送的信号。

“已启动”只说明真实主进程、执行身份和它自己实际持有的原 SYSTEM 共享锁获得核对，不代表实际 HTTP 监听、访问或登录通过。停止前还须证明这组只有该原 Web 进程（正常线程不是额外进程）；“已停止”表示此固定单元回到非活动、原主进程已退出且原 cgroup 无残留或已由内核删除，不是全池停机证明。维护前仍须独立取得真实原 SYSTEM 独占锁；SMB 等持有者另核。暂未核妥单元时可继续用前面的原生前台方式。

## 待现场验收

确认原生 HTTP 能按准确地址从预先同意的 IPv4 网络到达、没有由本单位新增的 IPv6 listener。正常登录、退出、失效会话、错 Host/跨源、伪代理头、错误 CSRF 和超限请求须各自实测，且不能触达业务/磁盘正文。独立 HTTPS 终结、其站点认证、绕过与信任配置另验。本次只交付源码；没有构建、安装、启动、登录或修改现场网络。

参考：[systemd 单元执行环境](https://www.freedesktop.org/software/systemd/man/latest/systemd.exec.html)。
