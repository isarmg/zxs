# ZXS 业务管理页：本机 HTTPS 部署模板

**入口是 `https://zxs-admin.test:8443/`，不是 helper 的磁盘面板。** 注：本文的模板仅为待验收源码，没有安装、启动、签发证书或测试浏览器。helper 安装包只把配置材料放在 `/usr/share/local-ops-helper/web-management/`，不装入 systemd 的搜索目录，不触碰在用 nginx，不开监听，不运行安装脚本。只有管理员另行选好实际发行物并激活这套配置，才可能访问页面。

```text
浏览器 -- 独立可信证书、TLS 上的 HTTP/1.1，127.0.0.1:8443 --> 专用非 root nginx
       -- 受保护的本机 Unix 流套接字/SCGI --> 非 root ZXS，进程自己验密码和原池
```

## 先核这些既有条件

1. 源码对照 Debian 13 的 nginx 1.26.3 系列。独立验证实际采用的受维护 amd64 发行修订（需内建 SSL、SCGI、map、limit_conn 模块）、systemd、系统 Python 3、`/usr/bin/zxs` 及 helper 发行签名/摘要。该包不安装 nginx、不启 Debian 的默认站点，也不附证书或密钥。不要为试用而直接安装可能自动启动默认站点的发行包。若上游软件未准备好，由站点另行定不干扰现网的安装计划。ZXS 没有可验证的正式制品时不要启动。已存在并验证的同一活动池、固定 Web 管理员及其口令来自另外批准的首装；本页不开户，不允许 SMB 密码、匿名或访客代替 Web 认证。
2. 专用且稳定的本机非登录用户/主组 **zxs-web-proxy**；不给任何其他账号该组、不给它补充组、池文件或 sudo。存储服务为该池另行登记的非 root UID/GID，与代理均不同；不要借用 www-data、Samba 客户端身份或临时 UID。模板不增改任何账号、组或策略。只支持能从宿主普通 `/etc/passwd`、`/etc/group` 核证的本地账号；真正的 UID/组未被系统其他来源出借，由站点独立保证。
3. 站点独立提供能被**该浏览器**验证、DNS SAN 精确覆盖 `zxs-admin.test` 的 TLS 证书和对应私钥；这是[保留的私测名称](https://www.rfc-editor.org/rfc/rfc6761#section-6.2)，并非自动解析的 localhost；须由站点独立保证该浏览器经受信本地解析仅得到 127.0.0.1（例如由管理员另行配置本机名字映射）。没有此映射不启用，helper 不改 hosts 或 DNS。固定 `/etc/zxs-management-tls/` 为 root:代理组 0750，`chain.pem` 为 root:root 0444 或 0644、完整 PEM 链，`key.pem` 为 root:代理组 0440、无扩展访问 ACL 的普通私钥，均不接受符号/硬链接。此模板要求站点正面确认承载密钥的是可信的本地 Unix/POSIX 权限环境；只有已核原件回报确实没有扩展访问 ACL（ENODATA）才通过。不支持查询、无权或另有后端/服务器端访问规则不能当作已隔离。密钥不得给存储用户、浏览器或 helper；模板检查只看它的元数据，不读其内容。由 nginx 在真正启动时读取匹配密钥；客户端仍要核链、时效和主机名，不得绕过证书错误。没有可信证书就停在此处。
4. 此窄模板仅接受 `/srv/...` 或 `/mnt/...` 下 ASCII 规范 SYSTEM 路径（无空格、`%`、超长组件等），根归实际存储 UID/GID、0700，祖先 root 所有且其他人不可写。不能表示的原路径明确不支持，**不要复制或另建同名数据库**。下方数字只能取自原安装和独立站点批准，不是从网页登录名推算。
5. 在激活前确认三枚单位及所有别名、临时覆盖、drop-in、`/run/zxs-management.sock`、`/run/zxs-management-https/`、127.0.0.1:8443 均未被其他部署占用；只有这次批准的 PID 1 配置有权创建其中两项 /run 名字。两个新 /run 名字在相应单位开始前还各用否定的存在/符号链接断言，已有名（包括悬空软链）不接管；这不是与其他特权管理者并发安装的事务。不能把“试着启动”当作无害占用探测。现网共享 nginx 不参与。管理 cookie 的作用域是主机、**不是端口**：该专名所有端口上的 HTTPS 服务都必须可信，包括匹配其证书的通配站点。不要把该名字加入浏览器的“不安全来源视作可信”列表，或改用有明文可信特例的 *.localhost。不能保证这台主机始终满足此条件就不要启用，也不能把通用 localhost 的不同端口当成隔离。

## 生成并核对候选（不需要 root）

在普通操作者自己拥有、不可被其他人写的工作目录，使用已独立验签的发行物。参数顺序：SYSTEM、原 PoolId、存储 UID/GID、代理 UID/GID。以下是语法示意，请代入该站点已有的原值：

```sh
/usr/bin/python3 -I -B /usr/share/local-ops-helper/web-management/profile.py render \
  "$SYSTEM" "$POOL_ID" "$STORAGE_UID" "$STORAGE_GID" "$PROXY_UID" "$PROXY_GID"
```

只创建一个新的 `./zxs-management.candidate/` 和三份未启用的单位；已有同名目录则拒绝，不覆盖。输出失败所留候选不算完成。这一步只核参数和模板字节，不探测站点、不核凭据、不安装服务。逐字检查候选：socket 在 root 控制的 /run 下为 root:代理组 0660、`FileDescriptorName=zxs-management`；后端是存储身份、无参数 `/usr/bin/zxs`（借原 SYSTEM 目录 SH、由 SQLite 自证池）；HTTPS 终结器是代理身份、仅上游 nginx 的一个私有配置，不能读取选中的 SYSTEM。

**由管理员另行审核安装。** 使用站点自己的配置管理，独立核定上述参数和签名模板后，复核**准备实际安装的三个文件内容**（可在独立可信的受保护暂存处用同一发行输入重生成比较）。以 root:root 0644 在受保护的 `/etc/systemd/system/` 置入这三个新名字：`zxs-management.socket`、`zxs-management.service`、`zxs-management-https.service`。已有同名或来源不明时停止，不把可写工作目录软链接给 PID 1，不覆盖、重用或接管既有单元。模板没有 `[Install]`、`WantedBy` 或自动启用动作。

只有在管理员明确授权现在启动后，才由其人员执行：

```sh
systemctl daemon-reload
systemctl start zxs-management.socket zxs-management-https.service
```

后端仅由匹配的 Unix socket 激活。前端 nginx 是非 root 的独立前台服务；不与公网监听共用 UID、配置、日志或临时目录。两者在自己真实服务身份下做静态预检：当前 UID/GID/附加组、空能力集及 NoNewPrivileges、受保护实名、Unix socket 所有者/组/模式；前端再核发布的 nginx 配置全部字节和证书/密钥的类型与权限，后端核 SYSTEM 的保护和固定环境。**这不是认证或健康证明。** 未经独立批准不得用预检结果创建证书、生成口令或修改现网。配置、执行文件和原现场在启动、运行、排空期间都要由站点保护；不支持动态替换、并发升级、未知 drop-in、非预期目录别名或不可信同 UID 程序。

手动停止时应先关浏览器，停止入口和 socket，排空后端再离线维护；不要只看到网页不通就推断 SQLite 已关。可明确安排 `systemctl stop zxs-management-https.service zxs-management.socket zxs-management.service`，另验进程终结及真实离线锁；超时或强杀后不视作正常排空。私钥轮换和包升级都应另获批准、排空后复核，不自动 reload 或重放未确认的表单。

## 传输边界与实机验收

- 只保留既有管理路由（登录、页面/样式和单页分页、退出、共享登记/模式），不用 nginx 提供任何文件目录，无 SMB/业务正文。固定配置核 SNI、原 HTTP Host（含端口）和 POST 的原 Origin；拒错误 Host/Origin、重复 Host 或不匹配的合并 Origin、缺 Origin 的 POST、chunked、有歧义的帧、绝对形式请求及超限表单。专用 Unix 通路只显式列八个字段。SCGI 的第一个 CONTENT_LENGTH 来自 nginx 对实际已收正文的统计；不开整套默认 scgi_params、不信任客户端的 HTTP_ 用户/TLS/转发头。Cookie 行按 nginx 语义合并，ZXS 自身拒重复的唯一管理会话名，不声称拒每个重复 Cookie 行。
- 固定 4096 字节正文上限，内存缓冲高于上限。关闭访问/请求错误日志、缓存、存储、磁盘临时正文、自动切换上游和重试；无页面或密码转储。系统换页、系统级调试/审计、内核或浏览器转储不在 nginx 的保证内，宿主须另行限制。生成器/预检均不用私钥内容、SQLite、网络或 shell。
- 将来获准后，在专用 Debian 13 amd64 隔离宿主实际核：不同 UID、错误组和不当证书拒绝；socket fd3/真 SO_PEERCRED；浏览器从准确 HTTPS 同源正常登录并退出；明文/其他 Host、重 Origin、错误 CSRF、4097B 及 chunked 均不受理，伪转发头不改变认证且没有磁盘正文；令服务停止/重启后旧会话无效、SQLite 原 S 的独立 EX 只能在在线持有者释放后取得。**没有执行上述验收。**

参考：[Debian 13 nginx](https://packages.debian.org/trixie/nginx)、[nginx SCGI](https://nginx.org/en/docs/http/ngx_http_scgi_module.html)、[nginx HTTPS](https://nginx.org/en/docs/http/ngx_http_ssl_module.html)、[systemd socket activation](https://www.freedesktop.org/software/systemd/man/latest/systemd.socket.html)、[RFC 6265 §8.5 端口隔离限制](https://www.rfc-editor.org/rfc/rfc6265#section-8.5)。
