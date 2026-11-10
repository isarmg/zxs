# 辅助工具：按原摘要恢复固定配置安装

本流程补足固定 systemd/Samba 配置安装中断后的原请求恢复。通用 bundle 只读状态识别由 platform-io 提供，安装计划、原输入与恢复决定仍属于 helper，不引入服务端生产依赖。它不是配置替换/回滚、服务激活或完整一条龙 ready。实际定向验证见本节末尾及 TASKS H07；没有执行 helper 的真实主机安装，没有访问或修改实际 `/zxs` / `/etc`、部署程序或启动服务。

## 原输入不能靠当前表单猜回

跨 `/etc/systemd/system/zxs.service` 与可选 `/etc/samba/zxs.conf` 的安装可能在部分文件发布后中断。现在可以只提供原摘要，读取原来的服务账户、主组、监听地址与可选独立 SMB 参数，并重新绑定当时的程序、账户来源与目录身份；恢复请求本身不接收新输入、任意路径或上传配置。

```text
zxs-helper deployment install-recover-preview --request-digest <64 位原摘要>
zxs-helper deployment install-recover-files --request-digest <同一原摘要> \
  --confirm-original-configuration-files-only --external-administration-stopped
```

Web 在配置安装卡片提供“保存原配置恢复摘要”“读取原配置与恢复状态”和独立续作确认。重新连接后粘贴原摘要即可，不复制当前部署表单。更改摘要或相关表单会撤销旧恢复预览；已完成时不显示续作按钮。保存的客户端 JSON 只有格式、修订和摘要，不是安装成功、主机身份或恢复权威证据，服务端仍从自己的准确保留材料加载原请求。

对应严格 POST：

- `/api/deployment/install-recover-preview`：仅 `request_digest`，读取当前材料，不创建/补齐/同步文件。
- `/api/deployment/install-recover-files`：仅原摘要、`confirmation="resume-original-configuration-files-only"` 与 `external_administration_stopped=true`。

缺少准确确认或停止声明在主机访问前拒绝；非 64 位小写十六进制摘要也在主机访问前拒绝。沿用同源/CSRF/IP Host、请求上限、单阻塞工作 owner、浏览器断开后继续持有 owner 与 helper 退出排空。它们不等于网络认证、TLS 或外部服务停止证明。

## 当前格式与明确不兼容

当前配置计划和完成收据采用 revision 2。在任何配置安装写入前，先永久发布：

```text
/zxs/.zxs-helper-config-request-<摘要>/request.json
```

记录包括严格格式 `zxs-helper-original-configuration-installation`、revision 2、完整原摘要及无秘密的原输入。它使用既有 0700 操作目录、0600 单链接文件、确定 pending 名、no-clobber 发布和文件/目录持久化屏障。只有原请求持久化后才继续原来的 intent 和 `/etc` 写入。

恢复先持有 `/zxs` 租约读取这个最多 64 KiB 的记录，再用同一个 host 句柄建立其余资源，避免重复获取自己的租约。原输入经过当前 typed Settings 验证；重建的准确 Plan 摘要必须与请求摘要一致，且保留 request、plan、unit 与可选 Samba 原文必须逐字节匹配当前准确材料。载荷、passwd/group 来源、账户、配置父目录或输入发生变化都不会换成一个新计划继续。

不双读旧格式，不从当前 unit 反推输入，不把没有原请求的旧 intent 转换为新记录；旧 revision 1 摘要在新恢复入口明确拒绝。原先正常新建/匹配文件流程仍只允许准确当前字节，任何旧材料都不会因此被删除、覆盖或静默迁移。

## 只读阶段识别与单向续作

`platform_io::publication::BundleDestination::inspect` 只读核对调用者给定的固定文件集合，区分不存在、pending 与已发布。它不会调用 publish、写缺项或 fsync；published 目录缺文件、未知条目、错误字节/所有者/模式、链接或 final/pending 双名都保留并拒绝。pending 可以缺少尚未发布的预期文件，但不能含不明 staging。

恢复遵守以下顺序：

1. 原请求完整可读，但 intent 未开始：可续作同一原请求。
2. 原请求已发布，intent 在 pending：只补齐准确同一 intent，尚不把它当完成。
3. intent 已完整发布，完成记录未开始：允许重新核验已有准确配置，并以原有 no-clobber 原语补缺项。
4. 完成记录在 pending：所有准确配置必须还在，只继续同一完成发布。
5. 完成记录已发布：所有当前字节和输入重新核验后显示完成；它不是服务状态或整体 ready 证明。

intent/完成材料没有前置已发布记录时拒绝。**只要完成记录目录已经开始，缺失配置就不能由原安装重建。** 这个检查同时接入正常 `install-files` 的同计划重试，不只是 Web 恢复按钮；有缺失、不同或不安全材料时，不会先补文件再检查旧收据。未知结果保留原 request/intent/installed 和 staging，错误继续携带原摘要、意图位置及底层错误来源。

极早中断使原 request 内容未完整保存时，摘要恢复不能凭空重建输入。保留材料并明确报错，不扫描其它操作、不猜测“最新”、不换 operation identity、不清理暂存文件。完成回执只绑定准确配置字节与输入/父目录，不声称锁定外部 root 写者或提供永久文件 inode 不变保证。

## 不改变的边界

- 不修改任意已有不同配置，不备份/替换旧配置，不改默认 `/etc/samba/smb.conf`。
- 不读取/修改业务数据、配置之外的运行池、管理员凭据或 Samba passdb，不创建账户。
- 不执行 `systemd-analyze`、`testparm`、daemon-reload、start、enable 或安装依赖。
- 不声称现有 unit/drop-in 已经过目标环境验证，不替代首次存储、VFS/OS 依赖、干净启动交接和正式激活。
- 当前 daemon 的 host 租约仍只排斥参与者；外部 ZXS/Samba/管理员必须由操作者先停止。helper 自身不注册自启。

## 资源与验证状态

固定读取一个原请求、最多三个 intent 文件和一个完成收据，不扫描全部历史。原请求 64 KiB 上限；配置仍各 64 KiB，账户源与程序仍使用既有 4 MiB / 256 MiB 上限。额外原请求发布增加一组目录/文件持久化屏障；不减少已有跨目录同步。预览不写系统状态，续作的输入/程序核验为固定次数的线性读，不在请求热循环扫描全池。

测试分离在根 test/：7 项 helper 恢复用例覆盖严格原请求/摘要、pending 读取、超额/冲突记录保留、阶段顺序和完成后缺件；3 项 HTTP 覆盖同源/CSRF/单 owner、禁止新参数与主机访问前拒绝；1 项 CLI 覆盖独立参数。platform 的 3 项通用 bundle 回归覆盖只读缺项状态、冲突原件保留以及权限/硬链接/符号链接拒绝。上述 helper 11 项与 platform publication 7 项（含 3 项新增、4 项既有）定向通过，helper 生产目标 check 无 warning；helper/platform 格式、JS 语法、原架构与来源/消费者/依赖文档 verify 通过。未运行全套、Clippy 或新 GitHub CI；真实 root `/etc` 正向安装、浏览器交互及断电实验未执行，不将临时目录与拒绝路径等同完整安装验收。
