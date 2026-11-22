# 内置 Rust SMB 3.1.1：隔离客户端验收

这是手工选择的隔离客户端验收，不是构建、启动、安装或默认服务切换入口。不接受真实用户主机、现有池、真实业务凭据、开放网络、其他服务端或未获准的候选。外部 `smbclient` 只做标准客户端对照，不编入 `zxs`，也不提供任何服务端、构建或运行依赖。本次源码清理不执行用例，监听回执或历史服务的结果不能代替本次验证，更不能视为 Windows、Finder、GVfs 或 `mount.cifs` 通过。

## 先独立确认原服务和隔离夹具

隔离环境负责先建立全新、可丢弃的测试池、原 S 与经登记的可写普通 F、一个真实已配的 SMB 测试账户；可选一块独立已批准的**空**回收盘 R。检查实际运行的是此次经审验的同一个 `zxs` ELF 内部角色，不是其他服务器：发行许可、单 ELF 摘要、原受管启动、真实原低权对端和 START/READY、签名范围及原监听地址须有独立证据。启动所用真实 ACTIVE、原 S、原 F/R、同站点排他和批准前置条件不是本脚本能补的。尚未批准的 `release:candidate` 不得为了测试绕过；只有监听确认属于监听阶段，不是通过身份或文件操作。结束后由原监督器单独记录排空和实际退出，本客户端不停止其进程。

唯一可选终点为专用测试网的 `192.0.2.1:445`、安装时已批准的服务器名 `zxs-lab.test` 和共享 `ZXS`。测试用低权进程的独立网络命名空间只有 `lo`、客户端 `192.0.2.2/30` 的 `zxsaccept0` veth。独立监督者应验证另一端只接测试服务，两个命名空间都不桥接主机或外网；限制外连只允许该对端 TCP 445。客户端测试接口禁用 IPv6；不得有默认路由、网关、其他接口或 DNS 依赖。脚本在接触凭据前核客户端原 namespace 的设备／inode、有效 capabilities、veth、IPv4/IPv6 路由及实际选出的本地源地址；这是误用防护，不是对恶意特权进程或预检后重接网络的安全保证。执行环境是受审 Linux，不是用户的电脑或 WSL。

参数仅接收两个由隔离监督者事先打开的只读 Linux memfd；不接受任意目标或凭据路径。两个 memfd 均 `0400`、无硬链接、封阻写入/伸缩和修改 seal。公告由监督者 UID 0 拥有，最多 2 KiB；如下字段均来自本次独立确认，不从候选自证。`client_netns`、`outside_netns` 各为两个整数 `[st_dev, st_ino]`（后者为创建夹具前原环境的网络 namespace）。`data_label` 是真实获准 F 的逻辑根名，`recycle_label` 是实际 R 名；不测回收则用 JSON `null`。真实源提交为 40 位小写十六进制，单 ELF 为 64 位小写 SHA-256；这个输入不是生产启动格式，也不授予服务权限。

| 字段 | 约定 |
| --- | --- |
| `profile` | `ZXS-ISOLATED-SMB-ACCEPT-1` |
| `observed` | `same-elf-listen-confirmed`，独立监督者完成上述核实后填写 |
| `source_commit`, `zxs_sha256` | 本次实际所审源码与执行文件 |
| `client_netns`, `outside_netns` | 真持有的两个不同命名空间的设备／inode |
| `host`, `address` | `zxs-lab.test`, `192.0.2.1`，且与实际批准计划相符 |
| `data_label`, `recycle_label` | 本次实际被批准的精确 F/R 名，无路径分隔符 |

另一 memfd 仅供本次真实测试账户，由测试进程 UID 拥有，最多 256 B，内容为 Samba 认证文件的三行 `username = …`、`password = …`、`domain = ZXSLOCAL`（每行 LF，顺序固定）。只用测试专用小写 ASCII 登录和无分隔符的随机密码（允许字母、数字、`_+.!-`，24 至 64 字节）。该夹具约定不更改服务凭据规则。`stock` 只向 `smbclient` 继承该原凭据 FD；`operations` 只从所持 FD 读入本进程。密码不进 argv/env/报告，第三方的原始输出、异常正文、认证数据和签名都不记录。测试监督器也不得开启交互跟踪、环境或核心转储；不承诺清除解释器和第三方库的每一份瞬时副本。

## 两个独立运行项

在有上述两个原 FD、已审且预装依赖的夹具 Python 环境，分开调用（示例中的 FD 编号必须实存，不是证据）：

```text
python3 -I tests/acceptance/rust_smb_client.py stock --evidence-fd 7 --credentials-fd 8
python3 -I tests/acceptance/rust_smb_client.py operations --evidence-fd 7 --credentials-fd 8
```

- `stock` 用原发行者的 [Samba 4.22 `smbclient`](https://www.samba.org/samba/docs/4.22/man-html/smbclient.1.html)，限制最小和最大均为 SMB3_11、要求签名、用已配置 NTLM 账户，明确不调用 Kerberos；单独一次标准 `echo` 以原命令的退出状态作为该 CLI 实际完成连接和操作的证据。[其 4.22 源码](https://github.com/samba-team/samba/blob/samba-4.22.0/source3/client/client.c#L945-L965)不把 `echo` 的失败作成功。不从 CLI 成功推断其他业务。原版 [put/reput 路径](https://github.com/samba-team/samba/blob/samba-4.22.0/source3/client/client.c#L1776-L1790)有截断式打开／额外元数据查询，且 CLI 没有直接的普通文件 FLUSH 命令；`open` 还会退回只读，因此不把它的文本当作成功获得写者或单写锁。
- `operations` 用标准客户端库给真实 TCP 发送普通独立请求，不自造线上字节、握手、签名、认证材料或错误包。先核到 3.1.1、服务端要求签名和选定 AES-CMAC；第三方实现验证真实 SPNEGO、会话和最终签名。夹具没有 Kerberos 附加模块或票据，不使用 `ntlm` 裸封装回退、不修改或放宽内层 MIC。会话不能是 guest/null，且各阶段实际操作与预期拒绝都查原签名；不自动跟随重解析、重试未知请求或生成替代成功。按实读的 `FILE_CREATED`、实际全部写入字节数、独立 FLUSH、另一连接在首写者仍开时被原 FILE gate 拒绝写入但能读取完整确认数据、原 CLOSE 后同一竞争者真能开写者、第三条新会话重开和逐字节比对为依据。两个写者都提供兼容 SHARE 位，不把 share mask 拒绝当作 Core 独占。成功只证明这条普通链，不证明断电持久化、缓存／持久句柄或跨盘移动。
- R 只在公告中明确给出独立空 R 时测试：先实际扫描空 R，给新测试 FILE 的实际普通句柄执行显式 `DELETE` + 正常最后 CLOSE，确认旧普通名字不再可用；只读本次唯一回收轮及同一文件，核对全部字节；在真实 R FILE 的原句柄上用标准 typed `FileRenameInformation` 发同一 `ZXS` 树内的无覆盖还原，再开新会话确认旧 R 名不再选中、目标逻辑文件号相同且正文逐字节一致。R 不提供见证时显式 `skip`；实际失败或结果不明不能视为通过或跳过。不把目前这条同树恢复形状当成任意客户端自动支持。

`operations` 的独立客户端镜像从官方已验证材料提供 `smbprotocol` **1.17.0**（[官方轮子及来源](https://pypi.org/project/smbprotocol/1.17.0/#files)，源码 `55e643ae50f61dd270d6a4884f16ffa12dc29596`，轮子 SHA-256 `bd1abff5417f5af83ca516a64ab8e5acece3dbdcf58d4e5e23e47f5165a77349`）及 `pyspnego` **0.12.4**（[官方轮子及来源](https://pypi.org/project/pyspnego/0.12.4/#files)，源码 `133ec0005ed156b544ebf7bfdee93e08aefcc1b4`，轮子 SHA-256 `a29a34de1abe9e9b30d9aa1d363f8e49fc159eb7355b1f2ee86ccb3a2ce3bba9`）。镜像负责审核 Python、其传递依赖、`iproute2` 和 `smbclient` 的准确发行包；源脚本只核两个库版本，不下载／安装客户端，也不把任何依赖带入 `zxs`。

输出只含固定阶段、通过／失败／跳过、独立提供的公开源／执行文件摘要、确实收到的状态或测试数据摘要，不输出测试账号、密码、原路径、原包或文件内容。任一失败非零退出，保留夹具供查证；不替失败步骤做普通 CLOSE、补 FLUSH、重复删除／还原或清理原物。结束后由隔离操作员处理一次性夹具。

## 普通数据卷到普通数据卷

仅用于单独批准的 Linux 资格制品：独立核对本次待验的源修订和整份 ELF。服务与客户端在同一台隔离 Linux 虚拟机；服务监督器留在该虚拟机初始命名空间，客户端沿用上述独立网口。用同版 Helper 准备全新 v18 池，原 S、F1、F2、R 为四个独立文件系统，F1/F2 是同一 ZXS 树中已确认、可写的原普通共享。由监督者先核对真实挂载、原标记和权限；两条用例各用一份独立的一次性夹具。

沿用两个封印 FD。移动专用公告改用 `profile = ZXS-ISOLATED-SMB-MOVE-1`、
`observed = same-elf-listen-and-distinct-original-media`，新增 `target_data_label`
（F2 逻辑名）及 `move_case`（`stock` 或 `original`）。`data_label` 为 F1，
`recycle_label` 必须提供本次独立 R 的真实名。现有 `stock`、`operations` 继续用旧公告。

1. 以同样参数运行 `seed-move-a`，随后用现有 Web 提交 A。再运行
   `seed-move-b`，用 Web 提交 B。两步分别在 F1 的同一
   `zxs-normal-move.bin` 确认 4096 字节的数据；同一 FileId 各得一版。
   两次 Web 提交都从同一台虚拟机的初始（服务端）网络命名空间，用浏览器直连
   实际确认的数字地址，例如 `http://192.0.2.1:8443`。`zxs-lab.test` 只用于 SMB；
   独立 SMB 客户端的网络命名空间仍只放行 TCP 445。
   记录两版的节点、版本、父节点、SAVE 和实际内容证据。
   B 后取原只读基准，确认 F2 同名空位和这个 FILE 没有传输。不要通过复制准备目标。
2. `move-stock` 先用强签客户端记录源的真实逻辑文件号和 B，再调用固定发行版
   [smbclient rename](https://github.com/samba-team/samba/blob/samba-4.22.0/source3/libsmb/cli_smb2_fnum.c#L3049-L3149)
   （原 DELETE 打开、一次无覆盖改名和 CLOSE，不调用复制）。新强签会话核同一文件号、
   目标 B、原名不可选。stock 使用者已关闭原句柄。
3. 在另一份夹具执行 `move-original`。原强签 OPEN 确认 B；兼容的第二写者提交
   4096 字节的 C 并正常关闭，原 OPEN 仍读 B。标准客户端库向原 FILE 发送一次
   不覆盖的 SET_INFO 并保留同一原请求，实际看到一条 PENDING 和同 MID/SID/AsyncId
   的强签成功结尾（终局零授额）。同一原 OPEN 验同一两半 FileId、文件号和 B 后关闭。
   新会话只在 F2 打开同一文件号并读到 C。此用例不修改握手、原帧、签名或信用额度。

监督者对同一个原 S 独立只读执行 [回执查询](rust_smb_cross_f_receipt.sql)。原
`:file` 必须取自移动前选定 FILE，前八字节的大端值对应客户端报出的文件号；
不能到新名反推基准。只接受 SQLite v18 的原应用和 INIT、真实当代共同账户、
同一 SMB 主体及两端原 RW 设置。一条已确认 kind-12、direction=3 选中 F2，
原放置代加一，两份 kind-11 历史与正式版本不变、关联正确且保留原工作稿。
stock 用例的 SAVE 不变；原句柄用例只新增移动前已确认的 C 那一笔 kind-10，
选址行的 SAVE 必须等于该最新记录。旧源清退和两份正式版本字节由独立原 FILE
只读证据核对；客户端和 SQL 分别记录。
SQL 不运行在客户端网络命名空间，不打开写连接，不传凭据，也不清理失败夹具。

**丢答、F2 实际离线另测。** 只在监督者能扣住原改名应答、核对同一 kind-12
已确认，并能让服务的原 F2 实际不可用时运行。用另一份真实独立 F2；
不要用删名、卸载宿主上的一个别名、禁用共享或模拟错误代替。
原连接断开后不重送 MOVE。旧 F1 不可从新会话重开，F2 不可用时不得借旧源回读；
复原同一 F2 后核原 FileId、工作稿、两个正式版本及同一选址回执。
当前客户端两项不控制设备或失答。Linux 资格运行：未执行。

## 普通 FILE 非阻塞字节锁（未运行）

沿用本文件的全新一次性隔离池、同一制品证明和两个封印 FD；不调用原生用户主机或开放网络。由标准有类型的 SMB 客户端分别以两个签名会话打开同一真实普通 FILE，两者明确使用兼容的 ShareAccess，只开一个正文写者，不能用共享访问拒绝冒充字节锁效果。

1. 以独立单元素 LOCK 测精确共享和独占区间，分别核本人／另一原柄在重合区间、相邻区间和区间外的读写。共享锁要允许重合读而禁止同柄写；独占锁仍允许锁主的原已授权 I/O。重合失败检查原签名与确切状态，并复核没有修改工作。
2. 验证原 EOF 处追加与长短调整，分别核被锁实际改变区间、空范围及相邻界限。原柄只解准确区间，另一原柄、子范围和未锁区间不得解除；原柄正常 CLOSE 或实际断开后，新开柄可以取得原区间。
3. 除已有正常保存外，核对 FILE 身份、工作修订和正式历史未因锁、解锁或拒绝而改变。分别记录还未覆盖的阻塞／多元素／复合形状；不以这条窄用例代替 WPS 或任一其他客户端验收。
