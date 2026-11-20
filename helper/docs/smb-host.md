# 专用 SMB：启动与确认

首次安装时，在终端选定原 INIT 登记的数据盘、每个盘在共享内的名字及只读/读写、服务器实际使用的 LAN 地址和客户网段。按[首装三步](samba-domain.md)完成布局、域和账户。成功的原账户回执同时确认这些普通目录的初始许可，不用登录 Web 再开启。只会有一个业务共享 **\\服务器\ZXS**；选中的普通盘是它的一级文件夹。不会共享系统盘、未选盘、物理路径或直接 UNC 别名。逻辑回收站、整盘删除与非空盘转换不在这次普通数据流程中。

## 安装材料

- 使用同一经独立验签的 Debian 13 amd64 正式版本，不混合旧版、系统 Samba 或开发构建。保存原域回执的 `bound_sha256`；不能用待验目录当前的 SHA 自行批准。切换数据库只针对新建池，原库不迁移、不重签。
- [发行配方](../packaging/debian13-amd64/online-smb.md)产生两个独立原 `elf-roles/smbd`、`elf-roles/storage` 根及对应 `ELF-ROLES-1`。独立发行者核准源码、两端实际 DDL、制品和许可证，随后将两棵完整树和清单安装到 `/usr/lib/zxs-smb-runtime/debian13-amd64/`。保持 root 属主、准确文件模式/硬链接，不用宿主库补齐。未签中间物不是获准制品。
- 管理员按**独立已签且已验的发行清单**把 `ELF-ROLES-1` 的 SHA-256 登记在 `/etc/local-ops-helper/online-smb.policy` 的唯一一行 `ZXS-SMB/ONLINE-1 <64 小写十六进制>`，root:root 0600、单链、尾 LF。承认的是确切发行闭包和已准站点执行身份，不从安装目录自荐材料。构建器不放置政策、不签名、不启服务。

## 启动

在受信本机 root 前台终端，选择同一次原回执：

~~~sh
/usr/bin/helper start-smb \
  --system "$SYSTEM" --pool "$POOL_ID" --init "$INIT_OP" --instance "$INSTANCE_ID" \
  --domain-op "$DOMAIN_OP" --setup-op "$SETUP_OP" --bound-sha256 "$BOUND_SHA256"
~~~

确认完整提示。不要在参数里传密码。程序不修改网卡、防火墙、DNS、既有 Samba 服务或宿主账号。实际站点必须是已批准 LAN，不与已有服务争用地址/端口；指定的本地 IPv4/IPv6、客户前缀、445、**SMB 3.1.1** 由正式程序核验，禁止访客、动态共享、NT 命名管道或 DFS。保持前台；Ctrl+C 请求停机。不用旧回环骨架、旧预检、systemd 的 active 或 TCP 探针声称已启动正确服务。

## 如何报告结果

- 原 SYSTEM 非阻塞 SH 先于实例 EX，两把贯穿真实两个原程序退出。低权 SQLite worker 借同一原 SH 只读确认已发的选定 F，不取得账户写权限、也不查询或投射回收盘。原共享历史不替代当前读写许可。
- 两个私有命名域按已核发行组装：非 root 存储端只见原 SYSTEM/选定 F；smbd 只见程序闭包、独立锁定的 files NSS、原 private/state、只读 accountdb、只读正式配置和 BOUND。两程序的标准流是真 `/dev/null`，不继承操作者终端或宿主全局库。
- smbd 和存储端各由自己的 `clone3` 原生 pidfd 判定，两端原 fd3 私有；存储另获原只读 BOUND fd4 与 smbd 原进程的 pidfd5。只有从真 smbd 的独立 fd4 收完整 **LISTEN1**，核同一原监听集合、BOUND、计划及真发送进程才报 `smb=listen-confirmed`。这不是客户可达、登录或实际 I/O。另从已批准 LAN 客户端用唯一 SMB 账户打开 `\\服务器\ZXS`，分别实证未选盘、匿名和旧方言被拒，并真正读写获准目录。
- 请求正常停止后只先通知已安装 TERM handler 的原存储进程，再回收确切两个原主进程。`smb=original-processes-exited` 仅表示两程序均正常退出，不证明每个会话自行排空或所有业务写已落稳。超时、失败或强制回收不报正常；内核等待可能延迟释放，不自动重启。
- 专实例临时状态只写每次独立的 `noswap` tmpfs。用真无持久流和进程配置禁转储，不修改全机 sysctl，也不承诺消除 Linux UID 切换瞬间、宿主管道 core 收集器、休眠或进程换页。站点按独立受管进程域管理异常生命周期。未做实机构建、启动、登录或传输验证。
